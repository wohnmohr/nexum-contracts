#![no_std]

use soroban_sdk::{
    contract, contractimpl, contracttype, contracterror, symbol_short,
    Address, BytesN, Env, String, Vec, log,
};

// ============================================================================
// Data Types
// ============================================================================

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum ReceivableStatus {
    Active,
    Collateralized,
    Matured,
    Settled,
    Defaulted,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Receivable {
    pub id: u64,
    pub owner: Address,
    pub original_creditor: Address,
    pub debtor_hash: BytesN<32>,
    pub face_value: i128,
    pub currency: Address,
    pub issuance_date: u64,
    pub maturity_date: u64,
    pub zk_proof_hash: BytesN<32>,
    pub status: ReceivableStatus,
    pub risk_score: u32,
    pub metadata_uri: String,
}

#[contracttype]
pub enum DataKey {
    Admin,
    Verifier,
    BorrowContract,
    NextId,
    Receivable(u64),
    OwnerReceivables(Address),
    TotalMinted,
    TotalActive,
    Paused,
}

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u32)]
pub enum Error {
    NotAuthorized = 1,
    NotVerifier = 2,
    ReceivableNotFound = 3,
    InvalidStatus = 4,
    InvalidMaturityDate = 5,
    InvalidFaceValue = 6,
    AlreadyInitialized = 7,
    ContractPaused = 8,
    NotOwner = 9,
    NotBorrowContract = 10,
    TransferNotAllowed = 11,
}

#[contract]
pub struct ReceivableTokenContract;

#[contractimpl]
impl ReceivableTokenContract {

    pub fn initialize(
        env: Env,
        admin: Address,
        verifier: Address,
    ) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Verifier, &verifier);
        env.storage().instance().set(&DataKey::NextId, &1u64);
        env.storage().instance().set(&DataKey::TotalMinted, &0u64);
        env.storage().instance().set(&DataKey::TotalActive, &0u64);
        env.storage().instance().set(&DataKey::Paused, &false);
        Ok(())
    }

    pub fn set_borrow(env: Env, borrow_contract: Address) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::BorrowContract, &borrow_contract);
        Ok(())
    }

    /// Mint a tokenized receivable — only callable by the ZK verifier authority
    pub fn mint(
        env: Env,
        creditor: Address,
        debtor_hash: BytesN<32>,
        face_value: i128,
        currency: Address,
        maturity_date: u64,
        zk_proof_hash: BytesN<32>,
        risk_score: u32,
        metadata_uri: String,
    ) -> Result<u64, Error> {
        Self::require_not_paused(&env)?;

        let verifier: Address = env.storage().instance().get(&DataKey::Verifier).unwrap();
        verifier.require_auth();
        creditor.require_auth();

        if face_value <= 0 {
            return Err(Error::InvalidFaceValue);
        }
        if maturity_date <= env.ledger().timestamp() {
            return Err(Error::InvalidMaturityDate);
        }

        let id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap();
        env.storage().instance().set(&DataKey::NextId, &(id + 1));

        let receivable = Receivable {
            id,
            owner: creditor.clone(),
            original_creditor: creditor.clone(),
            debtor_hash,
            face_value,
            currency,
            issuance_date: env.ledger().timestamp(),
            maturity_date,
            zk_proof_hash,
            status: ReceivableStatus::Active,
            risk_score,
            metadata_uri,
        };

        env.storage().persistent().set(&DataKey::Receivable(id), &receivable);

        let mut list: Vec<u64> = env.storage().persistent()
            .get(&DataKey::OwnerReceivables(creditor.clone()))
            .unwrap_or(Vec::new(&env));
        list.push_back(id);
        env.storage().persistent().set(&DataKey::OwnerReceivables(creditor.clone()), &list);

        let total: u64 = env.storage().instance().get(&DataKey::TotalMinted).unwrap();
        env.storage().instance().set(&DataKey::TotalMinted, &(total + 1));
        let active: u64 = env.storage().instance().get(&DataKey::TotalActive).unwrap();
        env.storage().instance().set(&DataKey::TotalActive, &(active + 1));

        env.events().publish((symbol_short!("mint"), creditor), (id, face_value));
        Ok(id)
    }

    /// Lock receivable as collateral — only borrow contract
    pub fn lock(env: Env, receivable_id: u64) -> Result<(), Error> {
        Self::require_not_paused(&env)?;
        Self::require_borrow_contract(&env)?;

        let mut recv = Self::get_internal(&env, receivable_id)?;
        if recv.status != ReceivableStatus::Active {
            return Err(Error::InvalidStatus);
        }
        recv.status = ReceivableStatus::Collateralized;
        env.storage().persistent().set(&DataKey::Receivable(receivable_id), &recv);
        Ok(())
    }

    /// Unlock receivable from collateral — only borrow contract
    pub fn unlock(env: Env, receivable_id: u64) -> Result<(), Error> {
        Self::require_borrow_contract(&env)?;

        let mut recv = Self::get_internal(&env, receivable_id)?;
        if recv.status != ReceivableStatus::Collateralized {
            return Err(Error::InvalidStatus);
        }
        recv.status = ReceivableStatus::Active;
        env.storage().persistent().set(&DataKey::Receivable(receivable_id), &recv);
        Ok(())
    }

    /// Transfer receivable ownership (only Active ones)
    pub fn transfer(env: Env, receivable_id: u64, from: Address, to: Address) -> Result<(), Error> {
        Self::require_not_paused(&env)?;
        from.require_auth();

        let mut recv = Self::get_internal(&env, receivable_id)?;
        if recv.owner != from { return Err(Error::NotOwner); }
        if recv.status != ReceivableStatus::Active { return Err(Error::TransferNotAllowed); }

        // Update owner lists
        let mut from_list: Vec<u64> = env.storage().persistent()
            .get(&DataKey::OwnerReceivables(from.clone()))
            .unwrap_or(Vec::new(&env));
        let mut new_from = Vec::new(&env);
        for rid in from_list.iter() {
            if rid != receivable_id { new_from.push_back(rid); }
        }
        env.storage().persistent().set(&DataKey::OwnerReceivables(from.clone()), &new_from);

        let mut to_list: Vec<u64> = env.storage().persistent()
            .get(&DataKey::OwnerReceivables(to.clone()))
            .unwrap_or(Vec::new(&env));
        to_list.push_back(receivable_id);
        env.storage().persistent().set(&DataKey::OwnerReceivables(to.clone()), &to_list);

        recv.owner = to.clone();
        env.storage().persistent().set(&DataKey::Receivable(receivable_id), &recv);
        Ok(())
    }

    pub fn settle(env: Env, receivable_id: u64) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        let mut recv = Self::get_internal(&env, receivable_id)?;
        if recv.status != ReceivableStatus::Active && recv.status != ReceivableStatus::Matured {
            return Err(Error::InvalidStatus);
        }
        recv.status = ReceivableStatus::Settled;
        env.storage().persistent().set(&DataKey::Receivable(receivable_id), &recv);
        let active: u64 = env.storage().instance().get(&DataKey::TotalActive).unwrap();
        env.storage().instance().set(&DataKey::TotalActive, &active.saturating_sub(1));
        Ok(())
    }

    pub fn mark_default(env: Env, receivable_id: u64) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        let mut recv = Self::get_internal(&env, receivable_id)?;
        recv.status = ReceivableStatus::Defaulted;
        env.storage().persistent().set(&DataKey::Receivable(receivable_id), &recv);
        let active: u64 = env.storage().instance().get(&DataKey::TotalActive).unwrap();
        env.storage().instance().set(&DataKey::TotalActive, &active.saturating_sub(1));
        Ok(())
    }

    // ---- View ----
    pub fn get_recv(env: Env, receivable_id: u64) -> Result<Receivable, Error> {
        Self::get_internal(&env, receivable_id)
    }

    pub fn get_owner(env: Env, owner: Address) -> Vec<u64> {
        env.storage().persistent()
            .get(&DataKey::OwnerReceivables(owner))
            .unwrap_or(Vec::new(&env))
    }

    pub fn total_minted(env: Env) -> u64 {
        env.storage().instance().get(&DataKey::TotalMinted).unwrap_or(0)
    }

    pub fn total_active(env: Env) -> u64 {
        env.storage().instance().get(&DataKey::TotalActive).unwrap_or(0)
    }

    // ---- Admin ----
    pub fn pause(env: Env) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
        Ok(())
    }

    pub fn unpause(env: Env) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &false);
        Ok(())
    }

    // ---- Internal ----
    fn get_internal(env: &Env, id: u64) -> Result<Receivable, Error> {
        env.storage().persistent().get(&DataKey::Receivable(id)).ok_or(Error::ReceivableNotFound)
    }

    fn require_not_paused(env: &Env) -> Result<(), Error> {
        let paused: bool = env.storage().instance().get(&DataKey::Paused).unwrap_or(false);
        if paused { Err(Error::ContractPaused) } else { Ok(()) }
    }

    fn require_borrow_contract(env: &Env) -> Result<(), Error> {
        let bc: Address = env.storage().instance().get(&DataKey::BorrowContract)
            .ok_or(Error::NotBorrowContract)?;
        bc.require_auth();
        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================
#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};

    fn setup() -> (Env, ReceivableTokenContractClient<'static>, Address, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(LedgerInfo {
            timestamp: 1_000_000,
            protocol_version: 21,
            sequence_number: 100,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3_110_400,
        });

        let contract_id = env.register(ReceivableTokenContract, ());
        let client = ReceivableTokenContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let verifier = Address::generate(&env);
        let creditor = Address::generate(&env);

        client.initialize(&admin, &verifier);
        // Leak env for static lifetime in tests
        let client = unsafe { core::mem::transmute(client) };
        (env, client, admin, verifier, creditor)
    }

    fn mint_one(env: &Env, client: &ReceivableTokenContractClient, creditor: &Address) -> u64 {
        let currency = Address::generate(env);
        client.mint(
            creditor,
            &BytesN::from_array(env, &[1u8; 32]),
            &1_000_000_i128,
            &currency,
            &2_000_000_u64,
            &BytesN::from_array(env, &[2u8; 32]),
            &500_u32,
            &String::from_str(env, "ipfs://test"),
        )
    }

    #[test]
    fn test_init_and_mint() {
        let (env, client, _, _, creditor) = setup();
        assert_eq!(client.total_minted(), 0);

        let id = mint_one(&env, &client, &creditor);
        assert_eq!(id, 1);
        assert_eq!(client.total_minted(), 1);
        assert_eq!(client.total_active(), 1);

        let recv = client.get_recv(&1);
        assert_eq!(recv.face_value, 1_000_000);
        assert_eq!(recv.owner, creditor);
        assert_eq!(recv.status, ReceivableStatus::Active);
    }

    #[test]
    fn test_multiple_mints() {
        let (env, client, _, _, creditor) = setup();
        mint_one(&env, &client, &creditor);
        mint_one(&env, &client, &creditor);
        assert_eq!(client.total_minted(), 2);
        assert_eq!(client.get_owner(&creditor).len(), 2);
    }

    #[test]
    fn test_lock_unlock() {
        let (env, client, admin, _, creditor) = setup();
        let borrow_addr = Address::generate(&env);
        client.set_borrow(&borrow_addr);

        let id = mint_one(&env, &client, &creditor);
        client.lock(&id);
        assert_eq!(client.get_recv(&id).status, ReceivableStatus::Collateralized);

        client.unlock(&id);
        assert_eq!(client.get_recv(&id).status, ReceivableStatus::Active);
    }

    #[test]
    fn test_transfer() {
        let (env, client, _, _, creditor) = setup();
        let buyer = Address::generate(&env);
        let id = mint_one(&env, &client, &creditor);

        client.transfer(&id, &creditor, &buyer);
        assert_eq!(client.get_recv(&id).owner, buyer);
        assert_eq!(client.get_owner(&creditor).len(), 0);
        assert_eq!(client.get_owner(&buyer).len(), 1);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #11)")]
    fn test_transfer_collateralized_fails() {
        let (env, client, _, _, creditor) = setup();
        let borrow_addr = Address::generate(&env);
        client.set_borrow(&borrow_addr);
        let id = mint_one(&env, &client, &creditor);
        client.lock(&id);
        client.transfer(&id, &creditor, &Address::generate(&env));
    }

    #[test]
    fn test_settle() {
        let (env, client, _, _, creditor) = setup();
        let id = mint_one(&env, &client, &creditor);
        client.settle(&id);
        assert_eq!(client.get_recv(&id).status, ReceivableStatus::Settled);
        assert_eq!(client.total_active(), 0);
    }

    #[test]
    fn test_pause_blocks_mint() {
        let (env, client, _, _, creditor) = setup();
        client.pause();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            mint_one(&env, &client, &creditor);
        }));
        assert!(result.is_err());

        client.unpause();
        let id = mint_one(&env, &client, &creditor);
        assert_eq!(id, 1);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #6)")]
    fn test_zero_face_value_fails() {
        let (env, client, _, _, creditor) = setup();
        let currency = Address::generate(&env);
        client.mint(
            &creditor, &BytesN::from_array(&env, &[1u8; 32]), &0_i128,
            &currency, &2_000_000_u64, &BytesN::from_array(&env, &[2u8; 32]),
            &500_u32, &String::from_str(&env, "ipfs://test"),
        );
    }
}
