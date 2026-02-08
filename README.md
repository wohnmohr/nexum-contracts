# Soroban Receivable Tokenization & Lending Protocol

Tokenize ZK-verified receivables on Stellar/Soroban and use them as collateral to borrow from a lending vault.

## Deployed Contracts (Testnet)

| Contract | Address |
|----------|---------|
| Receivable Token | `CC2DRZCX6GB3SKVD6QOKJ4INTCUMOITZAJ4HLVO7JRIFRZDL6LJN3KWK` |
| Lending Vault | `CASROYI6HGFXGAEABTMYIYDE3EUK7I7LLC6ZP5NZFB5ZAVY2EQJT5WTS` |
| Borrow Contract | `CDYRJVQLCX5TGFATTDJV63NNM4N2OADZ3KTRRRQZG6TJY6ZOT4MFW37X` |

**Network:** Stellar Testnet
**RPC URL:** `https://soroban-testnet.stellar.org:443`

## Architecture

```
                           ┌─────────────────────┐
   ZK Verifier ──────────▶ │   Receivable Token   │  NFT-like tokenized invoices
   (external)              │                      │  with face value, risk score,
                           │  CC2DRZ...N3KWK      │  debtor hash, maturity date
                           └──────────┬───────────┘
                                      │
                         lock/unlock  │  get_recv (cross-contract)
                                      ▼
                           ┌─────────────────────┐
                           │   Borrow Contract    │  LTV checks, interest accrual,
                           │                      │  liquidation engine
                           │  CDYRJV...W37X       │
                           └──────────┬───────────┘
                                      │
                       disburse/repay │  liq_recv
                                      ▼
                           ┌─────────────────────┐
                           │   Lending Vault      │  LP deposits, share-based
                           │                      │  accounting, yield from loans
                           │  CASROY...T5WTS      │
                           └─────────────────────┘
                                      ▲
                             deposit / withdraw
                                      │
                                 LP Providers
```

### Multi-Pool Architecture

The protocol supports multiple lending pools. A single shared **Receivable Token** contract manages all tokenized receivables, while each **pool** consists of its own Lending Vault + Borrow Contract pair with independent configuration (different assets, LTV ratios, interest rates, etc.).

```
                    ┌──────────────────────┐
                    │  Receivable Token    │  (shared - 1 per protocol)
                    │  Manages all NFTs    │
                    └────┬───────┬────┬────┘
                         │       │    │
              ┌──────────┘       │    └──────────┐
              ▼                  ▼               ▼
     ┌────────────────┐ ┌────────────────┐ ┌────────────────┐
     │  Pool 1 (XLM)  │ │  Pool 2 (USDC) │ │  Pool 3 (XLM)  │
     │  Vault+Borrow  │ │  Vault+Borrow  │ │  Vault+Borrow  │
     │  LTV: 50%      │ │  LTV: 70%      │ │  LTV: 80%      │
     │  APR: 8%       │ │  APR: 12%      │ │  APR: 20%      │
     └────────────────┘ └────────────────┘ └────────────────┘
```

To deploy additional pools, use the `deploy_pool.sh` script:

```bash
./deploy_pool.sh xlm-conservative native --ltv 5000 --interest 800
./deploy_pool.sh usdc-pool USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5
./deploy_pool.sh xlm-high-risk native --ltv 8000 --liq 9000 --interest 2000
```

---

## Contracts

### 1. Receivable Token (`receivable_token`)

NFT-like contract for tokenizing ZK-verified invoices/receivables. Each receivable has a face value, risk score, debtor hash, maturity date, and ZK proof hash.

#### Data Types

**`Receivable`** - On-chain representation of a tokenized invoice:

| Field | Type | Description |
|-------|------|-------------|
| `id` | `u64` | Auto-incrementing unique identifier |
| `owner` | `Address` | Current owner of the receivable |
| `original_creditor` | `Address` | The creditor who originally minted it |
| `debtor_hash` | `BytesN<32>` | SHA-256 hash of the debtor identity (privacy-preserving) |
| `face_value` | `i128` | Nominal value of the invoice (in stroops) |
| `currency` | `Address` | Token contract address for the invoice currency |
| `issuance_date` | `u64` | Ledger timestamp when minted |
| `maturity_date` | `u64` | When the invoice is due for payment |
| `zk_proof_hash` | `BytesN<32>` | Hash of the ZK proof that validated this receivable |
| `status` | `ReceivableStatus` | Current lifecycle state |
| `risk_score` | `u32` | Risk rating in basis points (0-10000) |
| `metadata_uri` | `String` | IPFS or off-chain metadata link |

**`ReceivableStatus`** - Lifecycle states:

| Status | Description |
|--------|-------------|
| `Active` | Minted and available for use as collateral or transfer |
| `Collateralized` | Locked as collateral for an active loan |
| `Matured` | Past its maturity date |
| `Settled` | Invoice has been paid by the debtor |
| `Defaulted` | Invoice debtor failed to pay |

#### Functions

##### Initialization

| Function | Auth | Description |
|----------|------|-------------|
| `initialize(admin, verifier)` | `admin` | One-time setup. Sets admin and verifier addresses, initializes counters. Fails with `AlreadyInitialized` if called again. |

##### Borrow Contract Authorization

| Function | Auth | Description |
|----------|------|-------------|
| `add_borrow(borrow_contract)` | `admin` | Authorize a borrow contract to lock/unlock receivables. Supports multiple pools. |
| `remove_borrow(borrow_contract)` | `admin` | Revoke a borrow contract's authorization. |
| `set_borrow(borrow_contract)` | `admin` | Backward-compatible alias for `add_borrow`. |

##### Core Operations

| Function | Auth | Description |
|----------|------|-------------|
| `mint(creditor, debtor_hash, face_value, currency, maturity_date, zk_proof_hash, risk_score, metadata_uri)` | `verifier` + `creditor` | Mint a new tokenized receivable. Verifier must sign (proves ZK validity). Creditor must sign (consents to tokenization). Returns the receivable `id`. |
| `lock(receivable_id, caller)` | `caller` (authorized borrow contract) | Lock a receivable as collateral. Changes status from `Active` to `Collateralized`. Only callable by authorized borrow contracts. |
| `unlock(receivable_id, caller)` | `caller` (authorized borrow contract) | Unlock a receivable from collateral. Changes status from `Collateralized` back to `Active`. Only callable by authorized borrow contracts. |
| `transfer(receivable_id, from, to)` | `from` | Transfer ownership of an `Active` receivable. Cannot transfer `Collateralized` receivables. Updates owner lists for both parties. |
| `settle(receivable_id)` | `admin` | Mark a receivable as `Settled` (debtor paid). Only works on `Active` or `Matured` receivables. Decrements active count. |
| `mark_default(receivable_id)` | `admin` | Mark a receivable as `Defaulted`. Decrements active count. |

##### View Functions

| Function | Description |
|----------|-------------|
| `get_recv(receivable_id) -> Receivable` | Get full receivable details by ID. |
| `get_owner(owner) -> Vec<u64>` | Get all receivable IDs owned by an address. |
| `total_minted() -> u64` | Total receivables ever minted. |
| `total_active() -> u64` | Currently active (non-settled, non-defaulted) receivables. |

##### Admin Functions

| Function | Auth | Description |
|----------|------|-------------|
| `pause()` | `admin` | Pause the contract. Blocks `mint`, `lock`, and `transfer`. |
| `unpause()` | `admin` | Unpause the contract. |

#### Errors

| Code | Name | Description |
|------|------|-------------|
| 1 | `NotAuthorized` | Caller lacks required authorization |
| 2 | `NotVerifier` | Caller is not the registered verifier |
| 3 | `ReceivableNotFound` | No receivable exists with the given ID |
| 4 | `InvalidStatus` | Receivable is not in the expected status for this operation |
| 5 | `InvalidMaturityDate` | Maturity date is in the past |
| 6 | `InvalidFaceValue` | Face value is zero or negative |
| 7 | `AlreadyInitialized` | Contract has already been initialized |
| 8 | `ContractPaused` | Contract is paused by admin |
| 9 | `NotOwner` | Caller does not own the receivable |
| 10 | `NotBorrowContract` | Caller is not an authorized borrow contract |
| 11 | `TransferNotAllowed` | Cannot transfer a collateralized receivable |

---

### 2. Lending Vault (`lending_vault`)

Liquidity pool that accepts deposits from LPs (liquidity providers) and issues share tokens. The borrow contract draws funds from the vault to disburse loans and returns repayments with interest split between LPs and the protocol.

#### Data Types

**`VaultState`** - Aggregate pool state:

| Field | Type | Description |
|-------|------|-------------|
| `total_deposits` | `i128` | Total deposited base asset (stroops) |
| `total_shares` | `i128` | Total outstanding LP shares |
| `total_borrowed` | `i128` | Currently lent out to borrowers |
| `total_interest_earned` | `i128` | Cumulative interest received from loans |
| `reserve_factor` | `i128` | Protocol's cut of interest (basis points, e.g. 1000 = 10%) |
| `protocol_reserves` | `i128` | Accumulated protocol fee revenue |

**`LPPosition`** - Per-depositor tracking:

| Field | Type | Description |
|-------|------|-------------|
| `shares` | `i128` | Number of LP shares held |
| `deposit_timestamp` | `u64` | When the first deposit was made |

#### Functions

##### Initialization

| Function | Auth | Description |
|----------|------|-------------|
| `initialize(admin, base_asset, reserve_factor, max_utilization, min_deposit)` | `admin` | One-time setup. `base_asset` is the token contract address (e.g. native XLM SAC). `reserve_factor` and `max_utilization` in basis points. `min_deposit` in stroops. |
| `set_borrow(borrow_contract)` | `admin` | Authorize a borrow contract to call `disburse`, `repay`, and `liq_recv`. |

##### LP Actions

| Function | Auth | Description |
|----------|------|-------------|
| `deposit(depositor, amount) -> i128` | `depositor` | Deposit base asset into the vault. Returns shares minted. First deposit is 1:1, subsequent deposits are proportional to `amount / total_assets * total_shares`. Transfers tokens from depositor to vault. |
| `withdraw(depositor, shares_to_burn) -> i128` | `depositor` | Burn LP shares and withdraw proportional base asset. Returns amount withdrawn. Fails if insufficient available liquidity (funds lent out). |

##### Borrow Contract Interface (cross-contract only)

| Function | Auth | Description |
|----------|------|-------------|
| `disburse(borrower, amount)` | `borrow_contract` | Transfer `amount` from vault to `borrower`. Checks available liquidity and utilization cap. Called by borrow contract when a loan is created. |
| `repay(borrower, principal, interest)` | `borrow_contract` | Receive repayment from borrower. Transfers `principal + interest` from borrower to vault. Interest is split: `reserve_factor` % to protocol reserves, remainder to LP deposits (increases share value). |
| `liq_recv(recovered, shortfall)` | `borrow_contract` | Handle liquidation accounting. Reduces `total_borrowed` by `recovered + shortfall`. Adds `recovered` to `total_deposits`. Shortfall represents a loss absorbed by LPs. |

##### View Functions

| Function | Description |
|----------|-------------|
| `get_state() -> VaultState` | Full vault state including deposits, borrows, interest, reserves. |
| `get_lp(depositor) -> Option<LPPosition>` | Get an LP's position (shares + deposit timestamp). |
| `total_assets() -> i128` | Total assets under management: `deposits + interest_earned - protocol_reserves`. |
| `available() -> i128` | Available liquidity: `total_deposits - total_borrowed`. |
| `utilization() -> i128` | Current utilization rate in basis points: `total_borrowed / total_deposits * 10000`. |
| `shares_value(shares) -> i128` | Calculate the base asset value of a given number of shares. |

##### Admin Functions

| Function | Auth | Description |
|----------|------|-------------|
| `pause()` | `admin` | Pause the contract. Blocks `deposit`, `withdraw`, and `disburse`. |
| `unpause()` | `admin` | Unpause the contract. |
| `withdraw_reserves(recipient, amount)` | `admin` | Withdraw accumulated protocol reserves to a recipient address. |

#### Errors

| Code | Name | Description |
|------|------|-------------|
| 1 | `NotAuthorized` | Caller lacks required authorization |
| 2 | `AlreadyInitialized` | Contract has already been initialized |
| 3 | `InsufficientDeposit` | Deposit amount below minimum |
| 4 | `InsufficientShares` | Trying to burn more shares than owned |
| 5 | `InsufficientLiquidity` | Not enough available funds (too much lent out) |
| 6 | `MaxUtilizationExceeded` | Loan would push utilization above the cap |
| 7 | `ContractPaused` | Contract is paused by admin |
| 8 | `ZeroAmount` | Amount must be greater than zero |
| 9 | `NotBorrowContract` | Caller is not the authorized borrow contract |
| 10 | `Overflow` | Arithmetic overflow |

---

### 3. Borrow Contract (`borrow_contract`)

Manages the full loan lifecycle: collateral validation, LTV checks, interest accrual, repayment, and liquidation. Cross-contract calls lock/unlock receivables and disburse/repay through the vault.

#### Data Types

**`Loan`** - On-chain loan record:

| Field | Type | Description |
|-------|------|-------------|
| `id` | `u64` | Auto-incrementing loan ID |
| `borrower` | `Address` | Borrower's Stellar address |
| `receivable_ids` | `Vec<u64>` | IDs of receivables pledged as collateral |
| `collateral_value` | `i128` | Risk-discounted collateral value at time of borrowing |
| `principal` | `i128` | Outstanding principal (decreases with repayments) |
| `interest_rate` | `i128` | Annual interest rate in basis points |
| `accrued_interest` | `i128` | Accumulated unpaid interest |
| `borrowed_at` | `u64` | Timestamp when loan was created |
| `last_interest_update` | `u64` | Last time interest was accrued |
| `due_date` | `u64` | Loan maturity timestamp |
| `status` | `LoanStatus` | Current loan state |

**`LoanStatus`** - Loan lifecycle:

| Status | Description |
|--------|-------------|
| `Active` | Loan is outstanding, interest accruing |
| `Repaid` | Fully repaid, collateral unlocked |
| `Liquidated` | Liquidated due to LTV breach or overdue |

**`BorrowConfig`** - Pool-level risk parameters:

| Field | Type | Description |
|-------|------|-------------|
| `max_ltv` | `i128` | Maximum loan-to-value ratio (bps). E.g. 7000 = 70% |
| `liquidation_threshold` | `i128` | LTV at which liquidation is allowed (bps). E.g. 8500 = 85% |
| `liquidation_penalty` | `i128` | Extra penalty applied during liquidation (bps). E.g. 500 = 5% |
| `base_interest_rate` | `i128` | Annual interest rate (bps). E.g. 1200 = 12% APR |
| `max_loan_duration` | `u64` | Maximum loan term in seconds. E.g. 7776000 = 90 days |
| `risk_discount_factor` | `i128` | Multiplier for risk score discount. E.g. 5000 |

#### Functions

##### Initialization

| Function | Auth | Description |
|----------|------|-------------|
| `initialize(admin, recv_contract, vault_contract, config)` | `admin` | One-time setup. Links the receivable token and vault contracts. Sets the borrow config. |
| `set_config(config)` | `admin` | Update the borrow configuration (LTV, rates, etc.) without redeployment. |

##### Borrowing

| Function | Auth | Description |
|----------|------|-------------|
| `borrow(borrower, receivable_ids, borrow_amount, duration) -> u64` | `borrower` | Create a new loan. Validates receivable ownership and status, calculates risk-discounted collateral value, checks LTV, locks receivables, disburses funds from vault. Returns `loan_id`. |

**Borrow flow:**
1. Validates each receivable is `Active` and owned by borrower
2. Calculates risk-discounted collateral: `face_value * (10000 - risk_score * risk_discount_factor / 10000) / 10000`
3. Checks `borrow_amount <= collateral * max_ltv / 10000`
4. Locks all receivables via `receivable_token.lock()`
5. Disburses funds via `vault.disburse()`
6. Creates loan record with interest rate and due date

##### Repayment

| Function | Auth | Description |
|----------|------|-------------|
| `repay_loan(borrower, loan_id, amount) -> i128` | `borrower` | Make a payment toward a loan. Accrues interest first. Pays interest before principal. Forwards payment to vault via `vault.repay()`. If fully repaid, unlocks all collateral. Returns remaining balance (0 = fully repaid). |

**Repay flow:**
1. Accrues interest to current timestamp
2. Caps payment at total owed (`principal + accrued_interest`)
3. Interest paid first, then principal
4. Calls `vault.repay(borrower, principal_pay, interest_pay)` which transfers tokens from borrower to vault
5. If remaining = 0, sets status to `Repaid` and unlocks all receivables

##### Liquidation

| Function | Auth | Description |
|----------|------|-------------|
| `liquidate(liquidator, loan_id)` | `liquidator` | Liquidate an unhealthy loan. Triggers if LTV exceeds `liquidation_threshold` or the loan is past its `due_date`. Transfers collateral receivables to the liquidator. Notifies vault of recovered/shortfall amounts. |

**Liquidation flow:**
1. Accrues interest to current timestamp
2. Checks if loan is liquidatable (LTV > threshold OR past due date)
3. Calculates: `penalty = total_debt * liquidation_penalty / 10000`
4. `recovered = min(collateral_value, total_debt + penalty)`
5. `shortfall = total_debt - recovered` (loss absorbed by LPs)
6. Unlocks and transfers all receivables to the liquidator
7. Calls `vault.liq_recv(recovered, shortfall)` to update vault accounting

##### Interest

| Function | Description |
|----------|-------------|
| `accrue_interest(loan_id) -> i128` | Manually trigger interest accrual for a loan. Returns accrued interest amount. Uses simple interest: `principal * rate_bps * elapsed_seconds / (seconds_per_year * 10000)`. |

**Interest model:** Simple interest, accrued per-second. `SECONDS_PER_YEAR = 31,557,600` (365.25 days).

##### View Functions

| Function | Description |
|----------|-------------|
| `get_loan(loan_id) -> Loan` | Get full loan details by ID. |
| `get_borrower_loans(borrower) -> Vec<u64>` | Get all loan IDs for a borrower. |
| `get_ltv(loan_id) -> i128` | Calculate current LTV in basis points, including pending (unaccrued) interest. |
| `is_liquidatable(loan_id) -> bool` | Check if a loan can be liquidated (LTV > threshold or overdue). |
| `get_config() -> BorrowConfig` | Get current borrow configuration. |
| `total_loans() -> u64` | Total number of loans ever created. |

##### Admin Functions

| Function | Auth | Description |
|----------|------|-------------|
| `pause()` | `admin` | Pause the contract. Blocks `borrow`, `repay_loan`, and `liquidate`. |
| `unpause()` | `admin` | Unpause the contract. |

#### Errors

| Code | Name | Description |
|------|------|-------------|
| 1 | `NotAuthorized` | Caller lacks required authorization |
| 2 | `AlreadyInitialized` | Contract has already been initialized |
| 3 | `LoanNotFound` | No loan exists with the given ID |
| 4 | `InvalidStatus` | Loan is not in the expected status |
| 5 | `LTVExceeded` | Borrow amount exceeds maximum LTV |
| 6 | `InsufficientCollateral` | Not enough collateral value |
| 7 | `NotLiquidatable` | Loan is healthy and not overdue |
| 8 | `ZeroAmount` | Amount must be greater than zero |
| 9 | `ContractPaused` | Contract is paused by admin |
| 10 | `InvalidDuration` | Duration is 0 or exceeds max loan duration |
| 11 | `RecvNotOwned` | Borrower does not own the receivable |
| 12 | `RecvNotActive` | Receivable is not in Active status |
| 13 | `Overflow` | Arithmetic overflow |
| 14 | `NotBorrower` | Caller is not the loan's borrower |

---

## Configuration

### Default Borrow Config

| Parameter | Default | Description |
|-----------|---------|-------------|
| `max_ltv` | 7000 (70%) | Maximum loan-to-value ratio |
| `liquidation_threshold` | 8500 (85%) | LTV at which liquidation triggers |
| `liquidation_penalty` | 500 (5%) | Extra penalty on liquidation |
| `base_interest_rate` | 1200 (12% APR) | Annual interest rate |
| `max_loan_duration` | 7776000 (90 days) | Max loan term in seconds |
| `risk_discount_factor` | 5000 | Risk score to collateral discount multiplier |

### Default Vault Config

| Parameter | Default | Description |
|-----------|---------|-------------|
| `reserve_factor` | 1000 (10%) | Protocol's share of interest income |
| `max_utilization` | 9000 (90%) | Maximum pool utilization before disbursal is blocked |
| `min_deposit` | 10000000 (1 XLM) | Minimum deposit in stroops |

### Risk Discount Formula

Receivable collateral value is discounted based on the risk score:

```
discount_bps = risk_score * risk_discount_factor / 10000
effective_value = face_value * (10000 - discount_bps) / 10000

Example:
  face_value = 1,000,000 stroops
  risk_score = 500 (5%)
  risk_discount_factor = 5000

  discount = 500 * 5000 / 10000 = 250 bps (2.5%)
  effective = 1,000,000 * (10000 - 250) / 10000 = 975,000
  max_borrow = 975,000 * 70% (max_ltv) = 682,500
```

### Interest Calculation

Simple interest, accrued per-second:

```
interest = principal * rate_bps * elapsed_seconds / (31,557,600 * 10000)

Example:
  principal = 500,000 stroops
  rate = 1200 bps (12% APR)
  elapsed = 2,592,000 seconds (30 days)

  interest = 500,000 * 1200 * 2,592,000 / (31,557,600 * 10000)
           = 49,281 stroops
```

---

## Quick Start

### 1. Setup (one time)

```bash
chmod +x *.sh
./setup.sh
```

Installs Rust, wasm32 target, Stellar CLI, testnet config, and 7 funded test identities:
- `admin` - Contract administrator
- `verifier` - ZK proof verification authority
- `creditor` - Invoice creditor (mints receivables)
- `borrower` - Takes loans against receivables
- `lp1`, `lp2` - Liquidity providers
- `liquidator` - Liquidates unhealthy loans

### 2. Build

```bash
./build.sh
```

Compiles all 3 contracts to WASM, copies to `target/wasm/`, and optimizes.

### 3. Run Unit Tests

```bash
./test.sh
```

Runs 21 unit tests across all 3 contracts (8 receivable_token + 13 lending_vault).

### 4. Deploy to Testnet

```bash
./deploy.sh
```

Deploys all 3 contracts, initializes them, and sets cross-contract authorizations. Saves addresses to `deployed_contracts.json`.

### 5. Deploy Additional Pools

```bash
./deploy_pool.sh <pool_name> <asset> [options]
```

Options:

| Flag | Default | Description |
|------|---------|-------------|
| `--ltv` | 7000 | Max LTV (basis points) |
| `--liq` | 8500 | Liquidation threshold (basis points) |
| `--penalty` | 500 | Liquidation penalty (basis points) |
| `--interest` | 1200 | Interest rate (basis points) |
| `--duration` | 7776000 | Max loan duration (seconds) |
| `--risk` | 5000 | Risk discount factor |
| `--reserve` | 1000 | Reserve factor (basis points) |
| `--max-util` | 9000 | Max utilization (basis points) |
| `--min-deposit` | 10000000 | Minimum deposit (stroops) |

Examples:

```bash
# Conservative XLM pool: 50% LTV, 8% APR
./deploy_pool.sh xlm-conservative native --ltv 5000 --interest 800

# USDC pool with default settings
./deploy_pool.sh usdc-pool USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5

# High-risk XLM pool: 80% LTV, 20% APR
./deploy_pool.sh xlm-high-risk native --ltv 8000 --liq 9000 --interest 2000
```

Pool config is saved to `pool_<name>.json`.

### 6. Integration Test (on testnet)

```bash
./test_integration.sh
```

Runs: mint receivable -> LP deposit -> borrow -> check LTV -> repay -> LP withdraw.

---

## Cross-Contract Authorization

The contracts require explicit authorization to interact with each other:

```
              add_borrow()
Receivable Token ◄───────────── Admin
       │
       │ lock/unlock (only authorized borrow contracts)
       ▼
Borrow Contract ──────────────► Lending Vault
              disburse/repay        │
              liq_recv              │
                                    │ set_borrow()
                              Admin ─┘
```

1. **Receivable Token** must authorize each borrow contract via `add_borrow(borrow_contract)`. Multiple borrow contracts can be authorized simultaneously (multi-pool support).
2. **Lending Vault** must authorize its borrow contract via `set_borrow(borrow_contract)`. Each vault only has one authorized borrow contract.

---

## Connecting Your ZK Verifier

`receivable_token.mint()` requires `verifier.require_auth()`:

1. User submits invoice + ZK proof to your verifier
2. Verifier validates proof off-chain
3. Verifier calls `mint()` on-chain (signed by verifier key)
4. Receivable minted with `zk_proof_hash` stored on-chain

---

## End-to-End Flow

### Minting a Receivable

```
Creditor + Verifier ──► receivable_token.mint(
                            creditor,
                            debtor_hash,       // SHA-256 of debtor identity
                            face_value,        // e.g. 10_000_0000000 (1000 XLM)
                            currency,          // XLM SAC address
                            maturity_date,     // future timestamp
                            zk_proof_hash,     // proof hash
                            risk_score,        // e.g. 500 (5%)
                            metadata_uri       // "ipfs://..."
                        ) -> receivable_id
```

### Depositing Liquidity

```
LP ──► lending_vault.deposit(
           depositor,  // LP address
           amount      // e.g. 20_000_0000000 (2000 XLM in stroops)
       ) -> shares
```

### Taking a Loan

```
Borrower ──► borrow_contract.borrow(
                 borrower,
                 receivable_ids,   // [1, 2, 3] - receivables to pledge
                 borrow_amount,    // e.g. 5_000_0000000 (500 XLM)
                 duration          // e.g. 2592000 (30 days)
             ) -> loan_id
```

### Repaying a Loan

```
Borrower ──► borrow_contract.repay_loan(
                 borrower,
                 loan_id,
                 amount    // partial or full repayment
             ) -> remaining_balance  (0 = fully repaid)
```

### Liquidating a Loan

```
Liquidator ──► borrow_contract.liquidate(
                   liquidator,
                   loan_id
               )
               // Liquidator receives the collateral receivables
               // Vault accounting is updated
```

---

## Tech Stack

- **Language:** Rust (no_std)
- **Framework:** Soroban SDK v21.7.4
- **Target:** `wasm32v1-none` (Soroban WASM)
- **Network:** Stellar Testnet (Soroban RPC)
- **CLI:** `stellar-cli`
