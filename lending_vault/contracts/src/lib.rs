#![no_std]

use soroban_sdk::{
    contract, contractimpl, contracttype, contracterror, symbol_short,
    token, Address, Env, log,
};

// ============================================================================
// Types
// ============================================================================

#[contracttype]
#[derive(Clone, Debug)]
pub struct VaultState {
    pub total_deposits: i128,
    pub total_shares: i128,
    pub total_borrowed: i128,
    pub total_interest_earned: i128,
    pub reserve_factor: i128,      // bps (1000 = 10%)
    pub protocol_reserves: i128,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct LPPosition {
    pub shares: i128,
    pub deposit_timestamp: u64,
}

#[contracttype]
pub enum DataKey {
    Admin,
    BaseAsset,
    BorrowContract,
    VaultState,
    LPPosition(Address),
    MinDeposit,
    MaxUtilization,
    Paused,
}

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u32)]
pub enum Error {
    NotAuthorized = 1,
    AlreadyInitialized = 2,
    InsufficientDeposit = 3,
    InsufficientShares = 4,
    InsufficientLiquidity = 5,
    MaxUtilizationExceeded = 6,
    ContractPaused = 7,
    ZeroAmount = 8,
    NotBorrowContract = 9,
    Overflow = 10,
}

#[contract]
pub struct LendingVaultContract;

#[contractimpl]
impl LendingVaultContract {

    pub fn initialize(
        env: Env,
        admin: Address,
        base_asset: Address,
        reserve_factor: i128,
        max_utilization: i128,
        min_deposit: i128,
    ) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::BaseAsset, &base_asset);
        env.storage().instance().set(&DataKey::MinDeposit, &min_deposit);
        env.storage().instance().set(&DataKey::MaxUtilization, &max_utilization);
        env.storage().instance().set(&DataKey::Paused, &false);
        env.storage().instance().set(&DataKey::VaultState, &VaultState {
            total_deposits: 0,
            total_shares: 0,
            total_borrowed: 0,
            total_interest_earned: 0,
            reserve_factor,
            protocol_reserves: 0,
        });
        Ok(())
    }

    pub fn set_borrow(env: Env, borrow_contract: Address) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::BorrowContract, &borrow_contract);
        Ok(())
    }

    // ========================================================================
    // LP Actions
    // ========================================================================

    /// Deposit base asset, receive LP shares
    pub fn deposit(env: Env, depositor: Address, amount: i128) -> Result<i128, Error> {
        Self::require_not_paused(&env)?;
        depositor.require_auth();

        if amount <= 0 { return Err(Error::ZeroAmount); }

        let min_dep: i128 = env.storage().instance().get(&DataKey::MinDeposit).unwrap_or(0);
        if amount < min_dep { return Err(Error::InsufficientDeposit); }

        let base_asset: Address = env.storage().instance().get(&DataKey::BaseAsset).unwrap();
        let mut state: VaultState = env.storage().instance().get(&DataKey::VaultState).unwrap();

        // Calculate shares
        let shares = if state.total_shares == 0 {
            amount
        } else {
            let total_assets = Self::calc_total_assets(&state);
            if total_assets == 0 { amount }
            else { Self::mul_div(amount, state.total_shares, total_assets)? }
        };
        if shares <= 0 { return Err(Error::ZeroAmount); }

        // Transfer tokens
        let tc = token::Client::new(&env, &base_asset);
        tc.transfer(&depositor, &env.current_contract_address(), &amount);

        // Update state
        state.total_deposits = state.total_deposits.checked_add(amount).ok_or(Error::Overflow)?;
        state.total_shares = state.total_shares.checked_add(shares).ok_or(Error::Overflow)?;
        env.storage().instance().set(&DataKey::VaultState, &state);

        // Update LP position
        let mut pos: LPPosition = env.storage().persistent()
            .get(&DataKey::LPPosition(depositor.clone()))
            .unwrap_or(LPPosition { shares: 0, deposit_timestamp: env.ledger().timestamp() });
        pos.shares = pos.shares.checked_add(shares).ok_or(Error::Overflow)?;
        env.storage().persistent().set(&DataKey::LPPosition(depositor.clone()), &pos);

        env.events().publish((symbol_short!("deposit"), depositor), (amount, shares));
        Ok(shares)
    }

    /// Withdraw by burning shares
    pub fn withdraw(env: Env, depositor: Address, shares_to_burn: i128) -> Result<i128, Error> {
        Self::require_not_paused(&env)?;
        depositor.require_auth();

        if shares_to_burn <= 0 { return Err(Error::ZeroAmount); }

        let mut pos: LPPosition = env.storage().persistent()
            .get(&DataKey::LPPosition(depositor.clone()))
            .ok_or(Error::InsufficientShares)?;
        if pos.shares < shares_to_burn { return Err(Error::InsufficientShares); }

        let base_asset: Address = env.storage().instance().get(&DataKey::BaseAsset).unwrap();
        let mut state: VaultState = env.storage().instance().get(&DataKey::VaultState).unwrap();

        let total_assets = Self::calc_total_assets(&state);
        let withdraw_amt = Self::mul_div(shares_to_burn, total_assets, state.total_shares)?;

        let available = state.total_deposits.saturating_sub(state.total_borrowed);
        if withdraw_amt > available { return Err(Error::InsufficientLiquidity); }

        let tc = token::Client::new(&env, &base_asset);
        tc.transfer(&env.current_contract_address(), &depositor, &withdraw_amt);

        state.total_deposits = state.total_deposits.checked_sub(withdraw_amt).ok_or(Error::Overflow)?;
        state.total_shares = state.total_shares.checked_sub(shares_to_burn).ok_or(Error::Overflow)?;
        env.storage().instance().set(&DataKey::VaultState, &state);

        pos.shares = pos.shares.checked_sub(shares_to_burn).ok_or(Error::Overflow)?;
        env.storage().persistent().set(&DataKey::LPPosition(depositor.clone()), &pos);

        env.events().publish((symbol_short!("withdraw"), depositor), (withdraw_amt, shares_to_burn));
        Ok(withdraw_amt)
    }

    // ========================================================================
    // Borrow Contract Interface
    // ========================================================================

    /// Disburse a loan to borrower — only borrow contract
    pub fn disburse(env: Env, borrower: Address, amount: i128) -> Result<(), Error> {
        Self::require_not_paused(&env)?;
        Self::require_borrow_contract(&env)?;

        if amount <= 0 { return Err(Error::ZeroAmount); }

        let base_asset: Address = env.storage().instance().get(&DataKey::BaseAsset).unwrap();
        let mut state: VaultState = env.storage().instance().get(&DataKey::VaultState).unwrap();

        let available = state.total_deposits.saturating_sub(state.total_borrowed);
        if amount > available { return Err(Error::InsufficientLiquidity); }

        // Utilization check
        let max_util: i128 = env.storage().instance().get(&DataKey::MaxUtilization).unwrap_or(9000);
        let new_borrowed = state.total_borrowed.checked_add(amount).ok_or(Error::Overflow)?;
        if state.total_deposits > 0 {
            let util = Self::mul_div(new_borrowed, 10000, state.total_deposits)?;
            if util > max_util { return Err(Error::MaxUtilizationExceeded); }
        }

        let tc = token::Client::new(&env, &base_asset);
        tc.transfer(&env.current_contract_address(), &borrower, &amount);

        state.total_borrowed = new_borrowed;
        env.storage().instance().set(&DataKey::VaultState, &state);

        env.events().publish((symbol_short!("disburse"), borrower), amount);
        Ok(())
    }

    /// Receive repayment — only borrow contract
    pub fn repay(env: Env, borrower: Address, principal: i128, interest: i128) -> Result<(), Error> {
        Self::require_borrow_contract(&env)?;

        let base_asset: Address = env.storage().instance().get(&DataKey::BaseAsset).unwrap();
        let mut state: VaultState = env.storage().instance().get(&DataKey::VaultState).unwrap();

        let total_payment = principal.checked_add(interest).ok_or(Error::Overflow)?;
        let tc = token::Client::new(&env, &base_asset);
        tc.transfer(&borrower, &env.current_contract_address(), &total_payment);

        // Split interest
        let protocol_share = Self::mul_div(interest, state.reserve_factor, 10000)?;
        let lp_share = interest.checked_sub(protocol_share).ok_or(Error::Overflow)?;

        state.total_borrowed = state.total_borrowed.checked_sub(principal).ok_or(Error::Overflow)?;
        state.total_deposits = state.total_deposits.checked_add(lp_share).ok_or(Error::Overflow)?;
        state.total_interest_earned = state.total_interest_earned.checked_add(interest).ok_or(Error::Overflow)?;
        state.protocol_reserves = state.protocol_reserves.checked_add(protocol_share).ok_or(Error::Overflow)?;
        env.storage().instance().set(&DataKey::VaultState, &state);

        env.events().publish((symbol_short!("repay"), borrower), (principal, interest));
        Ok(())
    }

    /// Receive liquidation proceeds — only borrow contract
    pub fn liq_recv(env: Env, recovered: i128, shortfall: i128) -> Result<(), Error> {
        Self::require_borrow_contract(&env)?;

        let mut state: VaultState = env.storage().instance().get(&DataKey::VaultState).unwrap();
        let total_cleared = recovered.checked_add(shortfall).ok_or(Error::Overflow)?;
        state.total_borrowed = state.total_borrowed.saturating_sub(total_cleared);
        if recovered > 0 {
            state.total_deposits = state.total_deposits.checked_add(recovered).ok_or(Error::Overflow)?;
        }
        env.storage().instance().set(&DataKey::VaultState, &state);
        Ok(())
    }

    // ========================================================================
    // View
    // ========================================================================

    pub fn total_assets(env: Env) -> i128 {
        let state: VaultState = env.storage().instance().get(&DataKey::VaultState).unwrap();
        Self::calc_total_assets(&state)
    }

    pub fn available(env: Env) -> i128 {
        let state: VaultState = env.storage().instance().get(&DataKey::VaultState).unwrap();
        state.total_deposits.saturating_sub(state.total_borrowed)
    }

    pub fn utilization(env: Env) -> i128 {
        let state: VaultState = env.storage().instance().get(&DataKey::VaultState).unwrap();
        if state.total_deposits == 0 { return 0; }
        Self::mul_div(state.total_borrowed, 10000, state.total_deposits).unwrap_or(0)
    }

    pub fn get_state(env: Env) -> VaultState {
        env.storage().instance().get(&DataKey::VaultState).unwrap()
    }

    pub fn get_lp(env: Env, depositor: Address) -> Option<LPPosition> {
        env.storage().persistent().get(&DataKey::LPPosition(depositor))
    }

    pub fn shares_value(env: Env, shares: i128) -> i128 {
        let state: VaultState = env.storage().instance().get(&DataKey::VaultState).unwrap();
        if state.total_shares == 0 { return shares; }
        Self::mul_div(shares, Self::calc_total_assets(&state), state.total_shares).unwrap_or(0)
    }

    // ========================================================================
    // Admin
    // ========================================================================

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

    pub fn withdraw_reserves(env: Env, recipient: Address, amount: i128) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        let base_asset: Address = env.storage().instance().get(&DataKey::BaseAsset).unwrap();
        let mut state: VaultState = env.storage().instance().get(&DataKey::VaultState).unwrap();
        if amount > state.protocol_reserves { return Err(Error::InsufficientLiquidity); }

        let tc = token::Client::new(&env, &base_asset);
        tc.transfer(&env.current_contract_address(), &recipient, &amount);
        state.protocol_reserves -= amount;
        env.storage().instance().set(&DataKey::VaultState, &state);
        Ok(())
    }

    // ========================================================================
    // Internal
    // ========================================================================

    fn calc_total_assets(state: &VaultState) -> i128 {
        state.total_deposits
            .saturating_add(state.total_interest_earned)
            .saturating_sub(state.protocol_reserves)
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

    fn mul_div(a: i128, b: i128, c: i128) -> Result<i128, Error> {
        if c == 0 { return Err(Error::Overflow); }
        Ok(((a as u128).checked_mul(b as u128).ok_or(Error::Overflow)?
            .checked_div(c as u128).ok_or(Error::Overflow)?) as i128)
    }
}

// ============================================================================
// Tests
// ============================================================================
#[cfg(test)]
mod test {
    extern crate std;
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Env,
    };
    use soroban_sdk::token::{StellarAssetClient, TokenClient};

    struct TestContext<'a> {
        env: Env,
        client: LendingVaultContractClient<'a>,
        token: TokenClient<'a>,
        token_admin: StellarAssetClient<'a>,
        admin: Address,
        lp1: Address,
        lp2: Address,
        borrow_contract: Address,
    }

    fn setup<'a>() -> TestContext<'a> {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
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

        let admin = Address::generate(&env);
        let lp1 = Address::generate(&env);
        let lp2 = Address::generate(&env);
        let borrow_contract = Address::generate(&env);

        // Create test token (simulates USDC)
        let token_admin_addr = Address::generate(&env);
        let token_id = env.register_stellar_asset_contract_v2(token_admin_addr.clone());
        let token = TokenClient::new(&env, &token_id.address());
        let token_admin = StellarAssetClient::new(&env, &token_id.address());

        // Fund LPs
        token_admin.mint(&lp1, &10_000_000);
        token_admin.mint(&lp2, &10_000_000);

        // Deploy vault
        let vault_id = env.register_contract(None, LendingVaultContract);
        let client = LendingVaultContractClient::new(&env, &vault_id);

        client.initialize(
            &admin,
            &token_id.address(),
            &1000_i128,        // 10% reserve factor
            &9000_i128,        // 90% max utilization
            &1000_i128,        // min deposit 1000
        );
        client.set_borrow(&borrow_contract);

        // Fund borrow_contract for repayment tests
        token_admin.mint(&borrow_contract, &5_000_000);

        // Transmute for static lifetime
        let client = unsafe { core::mem::transmute(client) };
        let token = unsafe { core::mem::transmute(token) };
        let token_admin = unsafe { core::mem::transmute(token_admin) };

        TestContext { env, client, token, token_admin, admin, lp1, lp2, borrow_contract }
    }

    #[test]
    fn test_deposit_and_shares() {
        let ctx = setup();

        let shares = ctx.client.deposit(&ctx.lp1, &1_000_000);
        assert_eq!(shares, 1_000_000); // First deposit is 1:1

        let pos = ctx.client.get_lp(&ctx.lp1).unwrap();
        assert_eq!(pos.shares, 1_000_000);

        let state = ctx.client.get_state();
        assert_eq!(state.total_deposits, 1_000_000);
        assert_eq!(state.total_shares, 1_000_000);
    }

    #[test]
    fn test_multiple_deposits() {
        let ctx = setup();

        ctx.client.deposit(&ctx.lp1, &1_000_000);
        let shares2 = ctx.client.deposit(&ctx.lp2, &2_000_000);

        // LP2 should get 2x shares since vault is 1:1 still
        assert_eq!(shares2, 2_000_000);

        let state = ctx.client.get_state();
        assert_eq!(state.total_deposits, 3_000_000);
        assert_eq!(state.total_shares, 3_000_000);
    }

    #[test]
    fn test_withdraw() {
        let ctx = setup();

        ctx.client.deposit(&ctx.lp1, &1_000_000);
        let withdrawn = ctx.client.withdraw(&ctx.lp1, &500_000);
        assert_eq!(withdrawn, 500_000);

        let pos = ctx.client.get_lp(&ctx.lp1).unwrap();
        assert_eq!(pos.shares, 500_000);

        let state = ctx.client.get_state();
        assert_eq!(state.total_deposits, 500_000);
    }

    #[test]
    fn test_full_withdraw() {
        let ctx = setup();

        ctx.client.deposit(&ctx.lp1, &1_000_000);
        let withdrawn = ctx.client.withdraw(&ctx.lp1, &1_000_000);
        assert_eq!(withdrawn, 1_000_000);
        assert_eq!(ctx.client.get_state().total_shares, 0);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #4)")]
    fn test_withdraw_too_many_shares() {
        let ctx = setup();
        ctx.client.deposit(&ctx.lp1, &1_000_000);
        ctx.client.withdraw(&ctx.lp1, &2_000_000);
    }

    #[test]
    fn test_disburse_loan() {
        let ctx = setup();

        ctx.client.deposit(&ctx.lp1, &5_000_000);

        let borrower = Address::generate(&ctx.env);
        ctx.client.disburse(&borrower, &3_000_000);

        let state = ctx.client.get_state();
        assert_eq!(state.total_borrowed, 3_000_000);
        assert_eq!(ctx.client.available(), 2_000_000);

        // Borrower should have received tokens
        assert_eq!(ctx.token.balance(&borrower), 3_000_000);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #5)")]
    fn test_disburse_exceeds_liquidity() {
        let ctx = setup();
        ctx.client.deposit(&ctx.lp1, &1_000_000);
        let borrower = Address::generate(&ctx.env);
        ctx.client.disburse(&borrower, &2_000_000);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #6)")]
    fn test_disburse_exceeds_max_utilization() {
        let ctx = setup();
        ctx.client.deposit(&ctx.lp1, &1_000_000);
        let borrower = Address::generate(&ctx.env);
        // 95% utilization > 90% max
        ctx.client.disburse(&borrower, &950_000);
    }

    #[test]
    fn test_repayment_splits_interest() {
        let ctx = setup();

        ctx.client.deposit(&ctx.lp1, &5_000_000);
        let borrower = Address::generate(&ctx.env);
        ctx.client.disburse(&borrower, &2_000_000);

        // Fund borrower for repayment
        ctx.token_admin.mint(&borrower, &2_200_000);

        // Repay: 2M principal + 200K interest
        ctx.client.repay(&borrower, &2_000_000, &200_000);

        let state = ctx.client.get_state();
        assert_eq!(state.total_borrowed, 0);
        assert_eq!(state.total_interest_earned, 200_000);
        // 10% reserve = 20K protocol, 180K to LPs
        assert_eq!(state.protocol_reserves, 20_000);
        // deposits should have increased by LP share of interest
        assert_eq!(state.total_deposits, 5_180_000);
    }

    #[test]
    fn test_share_value_increases_with_interest() {
        let ctx = setup();

        ctx.client.deposit(&ctx.lp1, &1_000_000);
        assert_eq!(ctx.client.shares_value(&1_000_000), 1_000_000);

        // Simulate interest by depositing more via repayment
        let borrower = Address::generate(&ctx.env);
        ctx.client.disburse(&borrower, &500_000);
        ctx.token_admin.mint(&borrower, &600_000);
        ctx.client.repay(&borrower, &500_000, &100_000);

        // Shares should now be worth more
        let value = ctx.client.shares_value(&1_000_000);
        assert!(value > 1_000_000);
    }

    #[test]
    fn test_utilization_rate() {
        let ctx = setup();
        ctx.client.deposit(&ctx.lp1, &10_000_000);

        assert_eq!(ctx.client.utilization(), 0);

        let borrower = Address::generate(&ctx.env);
        ctx.client.disburse(&borrower, &5_000_000);
        assert_eq!(ctx.client.utilization(), 5000); // 50%
    }

    #[test]
    fn test_pause_blocks_operations() {
        let ctx = setup();
        ctx.client.pause();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ctx.client.deposit(&ctx.lp1, &1_000_000);
        }));
        assert!(result.is_err());

        ctx.client.unpause();
        ctx.client.deposit(&ctx.lp1, &1_000_000);
    }

    #[test]
    fn test_liquidation_proceeds() {
        let ctx = setup();
        ctx.client.deposit(&ctx.lp1, &5_000_000);

        let borrower = Address::generate(&ctx.env);
        ctx.client.disburse(&borrower, &2_000_000);

        // Simulate liquidation: recovered 1.5M, shortfall 500K
        ctx.client.liq_recv(&1_500_000, &500_000);

        let state = ctx.client.get_state();
        assert_eq!(state.total_borrowed, 0);
        assert_eq!(state.total_deposits, 5_000_000 + 1_500_000);
    }
}