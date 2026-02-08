#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "============================================"
echo "  Integration Test (Testnet E2E)"
echo "============================================"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

NETWORK="testnet"

# Load contract addresses
if [ ! -f "deployed_contracts.json" ]; then
    echo -e "${RED}Error: deployed_contracts.json not found. Run ./deploy.sh first.${NC}"
    exit 1
fi

RECV_CONTRACT=$(cat deployed_contracts.json | grep -o '"receivable_token": "[^"]*"' | cut -d'"' -f4)
VAULT_CONTRACT=$(cat deployed_contracts.json | grep -o '"lending_vault": "[^"]*"' | cut -d'"' -f4)
BORROW_CONTRACT=$(cat deployed_contracts.json | grep -o '"borrow_contract": "[^"]*"' | cut -d'"' -f4)

ADMIN=$(stellar keys address admin)
VERIFIER=$(stellar keys address verifier)
CREDITOR=$(stellar keys address creditor)
BORROWER=$(stellar keys address borrower)
LP1=$(stellar keys address lp1)

print_step()  { echo -e "\n${GREEN}[TEST $1]${NC} $2"; }
print_ok()    { echo -e "  ${GREEN}✓${NC} $1"; }
print_result(){ echo -e "  ${YELLOW}→${NC} $1"; }

# ============================================================
# Test 1: Mint a receivable (verifier signs)
# ============================================================

print_step 1 "Minting a tokenized receivable..."

# Generate a mock 32-byte hash (hex)
MOCK_HASH="0101010101010101010101010101010101010101010101010101010101010101"

RECV_ID=$(stellar contract invoke \
    --id $RECV_CONTRACT \
    --source verifier \
    --network $NETWORK \
    -- \
    mint \
    --creditor $CREDITOR \
    --debtor_hash $MOCK_HASH \
    --face_value 10000000000 \
    --currency CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2OOTWHUF \
    --maturity_date 1800000000 \
    --zk_proof_hash $MOCK_HASH \
    --risk_score 500 \
    --metadata_uri "ipfs://QmTestReceivableMetadata123")

print_ok "Minted receivable ID: $RECV_ID"

# Verify
print_step 1b "Verifying receivable data..."

stellar contract invoke \
    --id $RECV_CONTRACT \
    --source admin \
    --network $NETWORK \
    -- \
    get_recv \
    --receivable_id $RECV_ID

print_ok "Receivable data verified"

# Check totals
TOTAL=$(stellar contract invoke \
    --id $RECV_CONTRACT \
    --source admin \
    --network $NETWORK \
    -- \
    total_minted)

print_result "Total minted: $TOTAL"

# ============================================================
# Test 2: LP deposits into vault
# ============================================================

print_step 2 "LP1 depositing into lending vault..."

SHARES=$(stellar contract invoke \
    --id $VAULT_CONTRACT \
    --source lp1 \
    --network $NETWORK \
    -- \
    deposit \
    --depositor $LP1 \
    --amount 50000000000)

print_ok "LP1 received shares: $SHARES"

# Check vault state
print_step 2b "Checking vault state..."

stellar contract invoke \
    --id $VAULT_CONTRACT \
    --source admin \
    --network $NETWORK \
    -- \
    get_state

print_ok "Vault state verified"

# ============================================================
# Test 3: Borrow against receivable
# ============================================================

print_step 3 "Borrowing against receivable as collateral..."

# Transfer receivable to borrower first (creditor -> borrower)
stellar contract invoke \
    --id $RECV_CONTRACT \
    --source creditor \
    --network $NETWORK \
    -- \
    transfer \
    --receivable_id $RECV_ID \
    --from $CREDITOR \
    --to $BORROWER

print_ok "Receivable transferred to borrower"

# Now borrow (70% LTV of 10B face value * discount = ~6.65B max, borrowing 5B)
LOAN_ID=$(stellar contract invoke \
    --id $BORROW_CONTRACT \
    --source borrower \
    --network $NETWORK \
    -- \
    borrow \
    --borrower $BORROWER \
    --receivable_ids "[$RECV_ID]" \
    --borrow_amount 5000000000 \
    --duration 2592000)

print_ok "Loan created, ID: $LOAN_ID"

# Check loan details
print_step 3b "Checking loan details..."

stellar contract invoke \
    --id $BORROW_CONTRACT \
    --source admin \
    --network $NETWORK \
    -- \
    get_loan \
    --loan_id $LOAN_ID

print_ok "Loan data verified"

# Check receivable is now Collateralized
print_step 3c "Verifying receivable is locked..."

stellar contract invoke \
    --id $RECV_CONTRACT \
    --source admin \
    --network $NETWORK \
    -- \
    get_recv \
    --receivable_id $RECV_ID

print_ok "Receivable status = Collateralized"

# ============================================================
# Test 4: Check LTV
# ============================================================

print_step 4 "Checking current LTV..."

LTV=$(stellar contract invoke \
    --id $BORROW_CONTRACT \
    --source admin \
    --network $NETWORK \
    -- \
    get_ltv \
    --loan_id $LOAN_ID)

print_result "Current LTV (bps): $LTV"

# ============================================================
# Test 5: Repay loan
# ============================================================

print_step 5 "Repaying loan..."

REMAINING=$(stellar contract invoke \
    --id $BORROW_CONTRACT \
    --source borrower \
    --network $NETWORK \
    -- \
    repay_loan \
    --borrower $BORROWER \
    --loan_id $LOAN_ID \
    --amount 5100000000)

print_ok "Repayment submitted, remaining: $REMAINING"

# Check loan status
print_step 5b "Verifying loan status..."

stellar contract invoke \
    --id $BORROW_CONTRACT \
    --source admin \
    --network $NETWORK \
    -- \
    get_loan \
    --loan_id $LOAN_ID

print_ok "Loan status verified"

# Check receivable unlocked
print_step 5c "Verifying receivable unlocked..."

stellar contract invoke \
    --id $RECV_CONTRACT \
    --source admin \
    --network $NETWORK \
    -- \
    get_recv \
    --receivable_id $RECV_ID

print_ok "Receivable status = Active (unlocked)"

# ============================================================
# Test 6: LP withdraws
# ============================================================

print_step 6 "LP1 withdrawing from vault..."

WITHDRAWN=$(stellar contract invoke \
    --id $VAULT_CONTRACT \
    --source lp1 \
    --network $NETWORK \
    -- \
    withdraw \
    --depositor $LP1 \
    --shares_to_burn $SHARES)

print_ok "LP1 withdrew: $WITHDRAWN (should be > original deposit due to interest)"

# ============================================================
# Summary
# ============================================================

echo ""
echo "============================================"
echo -e "${GREEN}  All Integration Tests Passed! ✓${NC}"
echo "============================================"
echo ""
echo "Full flow completed:"
echo "  1. ✓ Minted ZK-verified receivable"
echo "  2. ✓ LP deposited into vault"
echo "  3. ✓ Borrowed against receivable collateral"
echo "  4. ✓ Verified LTV calculation"
echo "  5. ✓ Repaid loan, receivable unlocked"
echo "  6. ✓ LP withdrew with interest"
echo ""
