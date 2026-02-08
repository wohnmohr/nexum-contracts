#![no_std]

use soroban_sdk::{
    contract, contractimpl, contracttype, contracterror, symbol_short,
    Address, Env, IntoVal, Symbol, Vec, log,
};

// ============================================================================
// Types
// ============================================================================

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum LoanStatus {
    Active,
    Repaid,
    Liquidated,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Loan {
    pub id: u64,
    pub borrower: Address,
    pub receivable_ids: Vec<u64>,
    pub collateral_value: i128,
    pub principal: i128,
    pub interest_rate: i128,
    pub accrued_interest: i128,
    pub borrowed_at: u64,
    pub last_interest_update: u64,
    pub due_date: u64,
    pub status: LoanStatus,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct BorrowConfig {
    pub max_ltv: i128,
    pub liquidation_threshold: i128,
    pub liquidation_penalty: i128,
    pub base_interest_rate: i128,
    pub max_loan_duration: u64,
    pub risk_discount_factor: i128,
}

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
    pub debtor_hash: soroban_sdk::BytesN<32>,
    pub face_value: i128,
    pub currency: Address,
    pub issuance_date: u64,
    pub maturity_date: u64,
    pub zk_proof_hash: soroban_sdk::BytesN<32>,
    pub status: ReceivableStatus,
    pub risk_score: u32,
    pub metadata_uri: soroban_sdk::String,
}

#[contracttype]
pub enum DataKey {
    Admin,
    RecvContract,
    VaultContract,
    Config,
    NextLoanId,
    Loan(u64),
    BorrowerLoans(Address),
    TotalLoans,
    TotalBorrowed,
    Paused,
}

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u32)]
pub enum Error {
    NotAuthorized = 1,
    AlreadyInitialized = 2,
    LoanNotFound = 3,
    InvalidStatus = 4,
    LTVExceeded = 5,
    InsufficientCollateral = 6,
    NotLiquidatable = 7,
    ZeroAmount = 8,
    ContractPaused = 9,
    InvalidDuration = 10,
    RecvNotOwned = 11,
    RecvNotActive = 12,
    Overflow = 13,
    NotBorrower = 14,
}

const SECONDS_PER_YEAR: u64 = 31_557_600;

#[contract]
pub struct BorrowContract;

#[contractimpl]
impl BorrowContract {

    pub fn initialize(
        env: Env,
        admin: Address,
        recv_contract: Address,
        vault_contract: Address,
        config: BorrowConfig,
    ) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::RecvContract, &recv_contract);
        env.storage().instance().set(&DataKey::VaultContract, &vault_contract);
        env.storage().instance().set(&DataKey::Config, &config);
        env.storage().instance().set(&DataKey::NextLoanId, &1u64);
        env.storage().instance().set(&DataKey::TotalLoans, &0u64);
        env.storage().instance().set(&DataKey::TotalBorrowed, &0i128);
        env.storage().instance().set(&DataKey::Paused, &false);
        Ok(())
    }

    pub fn set_config(env: Env, config: BorrowConfig) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Config, &config);
        Ok(())
    }

    // ========================================================================
    // Borrow
    // ========================================================================

    pub fn borrow(
        env: Env,
        borrower: Address,
        receivable_ids: Vec<u64>,
        borrow_amount: i128,
        duration: u64,
    ) -> Result<u64, Error> {
        Self::require_not_paused(&env)?;
        borrower.require_auth();
        if borrow_amount <= 0 { return Err(Error::ZeroAmount); }

        let config: BorrowConfig = env.storage().instance().get(&DataKey::Config).unwrap();
        if duration == 0 || duration > config.max_loan_duration {
            return Err(Error::InvalidDuration);
        }

        let recv_addr: Address = env.storage().instance().get(&DataKey::RecvContract).unwrap();
        let vault_addr: Address = env.storage().instance().get(&DataKey::VaultContract).unwrap();

        // Validate receivables and compute discounted collateral
        let mut total_collateral: i128 = 0;
        for rid in receivable_ids.iter() {
            let recv: Receivable = env.invoke_contract(
                &recv_addr,
                &Symbol::new(&env, "get_recv"),
                soroban_sdk::vec![&env, rid.into_val(&env)],
            );
            if recv.owner != borrower { return Err(Error::RecvNotOwned); }
            if recv.status != ReceivableStatus::Active { return Err(Error::RecvNotActive); }

            let risk_disc = Self::mul_div(recv.risk_score as i128, config.risk_discount_factor, 10000)?;
            let eff = 10000i128.saturating_sub(risk_disc);
            let disc_val = Self::mul_div(recv.face_value, eff, 10000)?;
            total_collateral = total_collateral.checked_add(disc_val).ok_or(Error::Overflow)?;
        }

        // LTV check
        let max_borrow = Self::mul_div(total_collateral, config.max_ltv, 10000)?;
        if borrow_amount > max_borrow { return Err(Error::LTVExceeded); }

        // Lock receivables (pass our own address for multi-pool auth)
        let self_addr = env.current_contract_address();
        for rid in receivable_ids.iter() {
            let _: () = env.invoke_contract(
                &recv_addr,
                &Symbol::new(&env, "lock"),
                soroban_sdk::vec![&env, rid.into_val(&env), self_addr.clone().into_val(&env)],
            );
        }

        // Disburse from vault
        let _: () = env.invoke_contract(
            &vault_addr,
            &Symbol::new(&env, "disburse"),
            soroban_sdk::vec![&env, borrower.clone().into_val(&env), borrow_amount.into_val(&env)],
        );

        // Create loan
        let loan_id: u64 = env.storage().instance().get(&DataKey::NextLoanId).unwrap();
        env.storage().instance().set(&DataKey::NextLoanId, &(loan_id + 1));
        let now = env.ledger().timestamp();

        let loan = Loan {
            id: loan_id,
            borrower: borrower.clone(),
            receivable_ids: receivable_ids.clone(),
            collateral_value: total_collateral,
            principal: borrow_amount,
            interest_rate: config.base_interest_rate,
            accrued_interest: 0,
            borrowed_at: now,
            last_interest_update: now,
            due_date: now + duration,
            status: LoanStatus::Active,
        };
        env.storage().persistent().set(&DataKey::Loan(loan_id), &loan);

        let mut blist: Vec<u64> = env.storage().persistent()
            .get(&DataKey::BorrowerLoans(borrower.clone()))
            .unwrap_or(Vec::new(&env));
        blist.push_back(loan_id);
        env.storage().persistent().set(&DataKey::BorrowerLoans(borrower.clone()), &blist);

        let tl: u64 = env.storage().instance().get(&DataKey::TotalLoans).unwrap();
        env.storage().instance().set(&DataKey::TotalLoans, &(tl + 1));
        let tb: i128 = env.storage().instance().get(&DataKey::TotalBorrowed).unwrap();
        env.storage().instance().set(&DataKey::TotalBorrowed, &(tb + borrow_amount));

        env.events().publish((symbol_short!("borrow"), borrower), (loan_id, borrow_amount));
        Ok(loan_id)
    }

    // ========================================================================
    // Repayment
    // ========================================================================

    pub fn repay_loan(
        env: Env,
        borrower: Address,
        loan_id: u64,
        amount: i128,
    ) -> Result<i128, Error> {
        Self::require_not_paused(&env)?;
        borrower.require_auth();
        if amount <= 0 { return Err(Error::ZeroAmount); }

        let mut loan = Self::get_internal(&env, loan_id)?;
        if loan.status != LoanStatus::Active { return Err(Error::InvalidStatus); }
        if loan.borrower != borrower { return Err(Error::NotBorrower); }

        Self::accrue(&env, &mut loan)?;

        let total_owed = loan.principal.checked_add(loan.accrued_interest).ok_or(Error::Overflow)?;
        let payment = core::cmp::min(amount, total_owed);

        let interest_pay = core::cmp::min(payment, loan.accrued_interest);
        let principal_pay = payment.checked_sub(interest_pay).ok_or(Error::Overflow)?;

        // Forward to vault
        let vault_addr: Address = env.storage().instance().get(&DataKey::VaultContract).unwrap();
        let _: () = env.invoke_contract(
            &vault_addr,
            &Symbol::new(&env, "repay"),
            soroban_sdk::vec![
                &env,
                borrower.clone().into_val(&env),
                principal_pay.into_val(&env),
                interest_pay.into_val(&env),
            ],
        );

        loan.principal = loan.principal.checked_sub(principal_pay).ok_or(Error::Overflow)?;
        loan.accrued_interest = loan.accrued_interest.checked_sub(interest_pay).ok_or(Error::Overflow)?;

        let remaining = loan.principal.checked_add(loan.accrued_interest).ok_or(Error::Overflow)?;
        if remaining == 0 {
            loan.status = LoanStatus::Repaid;

            // Unlock receivables (pass our own address for multi-pool auth)
            let recv_addr: Address = env.storage().instance().get(&DataKey::RecvContract).unwrap();
            let self_addr = env.current_contract_address();
            for rid in loan.receivable_ids.iter() {
                let _: () = env.invoke_contract(
                    &recv_addr,
                    &Symbol::new(&env, "unlock"),
                    soroban_sdk::vec![&env, rid.into_val(&env), self_addr.clone().into_val(&env)],
                );
            }
        }

        env.storage().persistent().set(&DataKey::Loan(loan_id), &loan);
        env.events().publish((symbol_short!("repay"), borrower), (loan_id, payment, remaining));
        Ok(remaining)
    }

    // ========================================================================
    // Liquidation
    // ========================================================================

    pub fn liquidate(
        env: Env,
        liquidator: Address,
        loan_id: u64,
    ) -> Result<(), Error> {
        Self::require_not_paused(&env)?;
        liquidator.require_auth();

        let mut loan = Self::get_internal(&env, loan_id)?;
        if loan.status != LoanStatus::Active { return Err(Error::InvalidStatus); }

        Self::accrue(&env, &mut loan)?;

        let config: BorrowConfig = env.storage().instance().get(&DataKey::Config).unwrap();
        let now = env.ledger().timestamp();

        let total_debt = loan.principal.checked_add(loan.accrued_interest).ok_or(Error::Overflow)?;
        let current_ltv = Self::mul_div(total_debt, 10000, loan.collateral_value)?;

        let is_underwater = current_ltv > config.liquidation_threshold;
        let is_overdue = now > loan.due_date;

        if !is_underwater && !is_overdue { return Err(Error::NotLiquidatable); }

        let penalty = Self::mul_div(total_debt, config.liquidation_penalty, 10000)?;
        let liq_value = total_debt.checked_add(penalty).ok_or(Error::Overflow)?;
        let recovered = core::cmp::min(loan.collateral_value, liq_value);
        let shortfall = total_debt.saturating_sub(recovered);

        // Transfer receivables to liquidator
        let recv_addr: Address = env.storage().instance().get(&DataKey::RecvContract).unwrap();
        let self_addr = env.current_contract_address();
        for rid in loan.receivable_ids.iter() {
            let _: () = env.invoke_contract(
                &recv_addr,
                &Symbol::new(&env, "unlock"),
                soroban_sdk::vec![&env, rid.into_val(&env), self_addr.clone().into_val(&env)],
            );
            let _: () = env.invoke_contract(
                &recv_addr,
                &Symbol::new(&env, "transfer"),
                soroban_sdk::vec![
                    &env,
                    rid.into_val(&env),
                    loan.borrower.clone().into_val(&env),
                    liquidator.clone().into_val(&env),
                ],
            );
        }

        // Notify vault
        let vault_addr: Address = env.storage().instance().get(&DataKey::VaultContract).unwrap();
        let _: () = env.invoke_contract(
            &vault_addr,
            &Symbol::new(&env, "liq_recv"),
            soroban_sdk::vec![&env, recovered.into_val(&env), shortfall.into_val(&env)],
        );

        loan.status = LoanStatus::Liquidated;
        env.storage().persistent().set(&DataKey::Loan(loan_id), &loan);

        env.events().publish((symbol_short!("liq"), liquidator), (loan_id, recovered, shortfall));
        Ok(())
    }

    // ========================================================================
    // Interest
    // ========================================================================

    pub fn accrue_interest(env: Env, loan_id: u64) -> Result<i128, Error> {
        let mut loan = Self::get_internal(&env, loan_id)?;
        if loan.status != LoanStatus::Active { return Err(Error::InvalidStatus); }
        Self::accrue(&env, &mut loan)?;
        let interest = loan.accrued_interest;
        env.storage().persistent().set(&DataKey::Loan(loan_id), &loan);
        Ok(interest)
    }

    fn accrue(env: &Env, loan: &mut Loan) -> Result<(), Error> {
        let now = env.ledger().timestamp();
        let elapsed = now.saturating_sub(loan.last_interest_update);
        if elapsed == 0 { return Ok(()); }

        // Simple interest: principal * rate_bps * elapsed / (YEAR * 10000)
        let num = (loan.principal as u128)
            .checked_mul(loan.interest_rate as u128).ok_or(Error::Overflow)?
            .checked_mul(elapsed as u128).ok_or(Error::Overflow)?;
        let den = (SECONDS_PER_YEAR as u128) * 10000u128;
        let new_interest = (num / den) as i128;

        loan.accrued_interest = loan.accrued_interest.checked_add(new_interest).ok_or(Error::Overflow)?;
        loan.last_interest_update = now;
        Ok(())
    }

    // ========================================================================
    // View
    // ========================================================================

    pub fn get_loan(env: Env, loan_id: u64) -> Result<Loan, Error> {
        Self::get_internal(&env, loan_id)
    }

    pub fn get_borrower_loans(env: Env, borrower: Address) -> Vec<u64> {
        env.storage().persistent()
            .get(&DataKey::BorrowerLoans(borrower))
            .unwrap_or(Vec::new(&env))
    }

    pub fn get_ltv(env: Env, loan_id: u64) -> Result<i128, Error> {
        let loan = Self::get_internal(&env, loan_id)?;
        let now = env.ledger().timestamp();
        let elapsed = now.saturating_sub(loan.last_interest_update);
        let mut interest = loan.accrued_interest;
        if elapsed > 0 {
            let num = (loan.principal as u128) * (loan.interest_rate as u128) * (elapsed as u128);
            let den = (SECONDS_PER_YEAR as u128) * 10000u128;
            interest += (num / den) as i128;
        }
        let total = loan.principal + interest;
        Self::mul_div(total, 10000, loan.collateral_value)
    }

    pub fn is_liquidatable(env: Env, loan_id: u64) -> Result<bool, Error> {
        let loan = Self::get_internal(&env, loan_id)?;
        if loan.status != LoanStatus::Active { return Ok(false); }
        let config: BorrowConfig = env.storage().instance().get(&DataKey::Config).unwrap();
        if env.ledger().timestamp() > loan.due_date { return Ok(true); }
        let ltv = Self::get_ltv(env, loan_id)?;
        Ok(ltv > config.liquidation_threshold)
    }

    pub fn get_config(env: Env) -> BorrowConfig {
        env.storage().instance().get(&DataKey::Config).unwrap()
    }

    pub fn total_loans(env: Env) -> u64 {
        env.storage().instance().get(&DataKey::TotalLoans).unwrap_or(0)
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

    // ========================================================================
    // Internal
    // ========================================================================

    fn get_internal(env: &Env, id: u64) -> Result<Loan, Error> {
        env.storage().persistent().get(&DataKey::Loan(id)).ok_or(Error::LoanNotFound)
    }

    fn require_not_paused(env: &Env) -> Result<(), Error> {
        let p: bool = env.storage().instance().get(&DataKey::Paused).unwrap_or(false);
        if p { Err(Error::ContractPaused) } else { Ok(()) }
    }

    fn mul_div(a: i128, b: i128, c: i128) -> Result<i128, Error> {
        if c == 0 { return Err(Error::Overflow); }
        Ok(((a as u128).checked_mul(b as u128).ok_or(Error::Overflow)?
            .checked_div(c as u128).ok_or(Error::Overflow)?) as i128)
    }
}