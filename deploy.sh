#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "============================================"
echo "  Deploy to Stellar Testnet"
echo "============================================"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

NETWORK="testnet"

# ============================================================
# Helpers
# ============================================================

print_step()  { echo -e "\n${GREEN}[STEP $1]${NC} $2"; }
print_info()  { echo -e "${YELLOW}  → $1${NC}"; }
print_addr()  { echo -e "  $1: ${GREEN}$2${NC}"; }

get_address() {
    stellar keys address "$1" 2>/dev/null
}

# ============================================================
# Pre-checks
# ============================================================

echo "Checking prerequisites..."

if ! command -v stellar &>/dev/null; then
    echo -e "${RED}Error: stellar CLI not found. Run ./setup.sh first.${NC}"
    exit 1
fi

if [ ! -f "target/wasm/receivable_token.wasm" ]; then
    echo -e "${RED}Error: WASM files not found. Run ./build.sh first.${NC}"
    exit 1
fi

ADMIN=$(get_address "admin")
if [ -z "$ADMIN" ]; then
    echo -e "${RED}Error: admin identity not found. Run ./setup.sh first.${NC}"
    exit 1
fi

VERIFIER=$(get_address "verifier")
CREDITOR=$(get_address "creditor")
BORROWER=$(get_address "borrower")
LP1=$(get_address "lp1")

echo ""
print_addr "Admin" "$ADMIN"
print_addr "Verifier" "$VERIFIER"
print_addr "Creditor" "$CREDITOR"
print_addr "Borrower" "$BORROWER"
print_addr "LP1" "$LP1"

# ============================================================
# Step 1: Deploy Receivable Token Contract
# ============================================================

print_step 1 "Deploying Receivable Token Contract..."

RECV_WASM_HASH=$(stellar contract install \
    --wasm target/wasm/receivable_token.wasm \
    --source admin \
    --network $NETWORK)
print_info "WASM hash: $RECV_WASM_HASH"

RECV_CONTRACT=$(stellar contract deploy \
    --wasm-hash $RECV_WASM_HASH \
    --source admin \
    --network $NETWORK)
print_info "Contract ID: $RECV_CONTRACT"

# Initialize
stellar contract invoke \
    --id $RECV_CONTRACT \
    --source admin \
    --network $NETWORK \
    -- \
    initialize \
    --admin $ADMIN \
    --verifier $VERIFIER

print_info "✓ Receivable Token initialized"

# ============================================================
# Step 2: Deploy Lending Vault Contract
# ============================================================

print_step 2 "Deploying Lending Vault Contract..."

VAULT_WASM_HASH=$(stellar contract install \
    --wasm target/wasm/lending_vault.wasm \
    --source admin \
    --network $NETWORK)
print_info "WASM hash: $VAULT_WASM_HASH"

VAULT_CONTRACT=$(stellar contract deploy \
    --wasm-hash $VAULT_WASM_HASH \
    --source admin \
    --network $NETWORK)
print_info "Contract ID: $VAULT_CONTRACT"

# For testnet, we use the native XLM as base asset
# In production, you'd use USDC: CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA
# Dynamically resolve the native asset contract address:
NATIVE_ASSET=$(stellar contract id asset --asset native --network $NETWORK)
print_info "Native asset contract: $NATIVE_ASSET"

stellar contract invoke \
    --id $VAULT_CONTRACT \
    --source admin \
    --network $NETWORK \
    -- \
    initialize \
    --admin $ADMIN \
    --base_asset $NATIVE_ASSET \
    --reserve_factor 1000 \
    --max_utilization 9000 \
    --min_deposit 10000000

print_info "✓ Lending Vault initialized (10% reserve, 90% max util)"

# ============================================================
# Step 3: Deploy Borrow Contract
# ============================================================

print_step 3 "Deploying Borrow Contract..."

BORROW_WASM_HASH=$(stellar contract install \
    --wasm target/wasm/borrow_contract.wasm \
    --source admin \
    --network $NETWORK)
print_info "WASM hash: $BORROW_WASM_HASH"

BORROW_CONTRACT=$(stellar contract deploy \
    --wasm-hash $BORROW_WASM_HASH \
    --source admin \
    --network $NETWORK)
print_info "Contract ID: $BORROW_CONTRACT"

# Initialize with config
stellar contract invoke \
    --id $BORROW_CONTRACT \
    --source admin \
    --network $NETWORK \
    -- \
    initialize \
    --admin $ADMIN \
    --recv_contract $RECV_CONTRACT \
    --vault_contract $VAULT_CONTRACT \
    --config '{"max_ltv":"7000","liquidation_threshold":"8500","liquidation_penalty":"500","base_interest_rate":"1200","max_loan_duration":7776000,"risk_discount_factor":"5000"}'

print_info "✓ Borrow Contract initialized (70% LTV, 85% liq threshold, 12% APR)"

# ============================================================
# Step 4: Cross-authorize contracts
# ============================================================

print_step 4 "Setting cross-contract authorizations..."

# Receivable Token → authorize borrow contract to lock/unlock (multi-pool safe)
stellar contract invoke \
    --id $RECV_CONTRACT \
    --source admin \
    --network $NETWORK \
    -- \
    add_borrow \
    --borrow_contract $BORROW_CONTRACT

print_info "✓ Receivable Token: authorized borrow contract"

# Vault → allow borrow contract to disburse/repay
stellar contract invoke \
    --id $VAULT_CONTRACT \
    --source admin \
    --network $NETWORK \
    -- \
    set_borrow \
    --borrow_contract $BORROW_CONTRACT

print_info "✓ Lending Vault: authorized borrow contract"

# ============================================================
# Summary
# ============================================================

echo ""
echo "============================================"
echo -e "${GREEN}  Deployment Complete!${NC}"
echo "============================================"
echo ""
echo "Contract Addresses:"
print_addr "Receivable Token" "$RECV_CONTRACT"
print_addr "Lending Vault   " "$VAULT_CONTRACT"
print_addr "Borrow Contract " "$BORROW_CONTRACT"
echo ""

# Save addresses to file
cat > deployed_contracts.json << EOF
{
    "network": "testnet",
    "rpc_url": "https://soroban-testnet.stellar.org:443",
    "contracts": {
        "receivable_token": "$RECV_CONTRACT",
        "lending_vault": "$VAULT_CONTRACT",
        "borrow_contract": "$BORROW_CONTRACT"
    },
    "identities": {
        "admin": "$ADMIN",
        "verifier": "$VERIFIER",
        "creditor": "$CREDITOR",
        "borrower": "$BORROWER",
        "lp1": "$LP1"
    },
    "config": {
        "base_asset": "$NATIVE_ASSET",
        "max_ltv_bps": 7000,
        "liquidation_threshold_bps": 8500,
        "interest_rate_bps": 1200,
        "reserve_factor_bps": 1000,
        "max_utilization_bps": 9000,
        "max_loan_duration_secs": 7776000
    }
}
EOF

echo "Addresses saved to: deployed_contracts.json"
echo ""
echo "Next: Run the integration test with ./test_integration.sh"
