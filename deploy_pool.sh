#!/bin/bash
set -e

# ============================================================
# Deploy an additional Lending Pool (Vault + Borrow Contract)
#
# Usage:
#   ./deploy_pool.sh <pool_name> <asset> [options]
#
# Examples:
#   ./deploy_pool.sh xlm-conservative  native  --ltv 5000 --interest 800
#   ./deploy_pool.sh usdc-pool         USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5
#   ./deploy_pool.sh xlm-high-risk     native  --ltv 8000 --liq 9000 --interest 2000
# ============================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

NETWORK="testnet"

print_step()  { echo -e "\n${GREEN}[STEP $1]${NC} $2"; }
print_info()  { echo -e "${YELLOW}  → $1${NC}"; }
print_addr()  { echo -e "  $1: ${GREEN}$2${NC}"; }

get_address() {
    stellar keys address "$1" 2>/dev/null
}

# ============================================================
# Parse arguments
# ============================================================

POOL_NAME="${1:?Usage: ./deploy_pool.sh <pool_name> <asset> [options]}"
ASSET="${2:?Usage: ./deploy_pool.sh <pool_name> <asset> [options]}"
shift 2

# Defaults (basis points)
MAX_LTV=7000
LIQ_THRESHOLD=8500
LIQ_PENALTY=500
INTEREST_RATE=1200
MAX_DURATION=7776000
RISK_DISCOUNT=5000
RESERVE_FACTOR=1000
MAX_UTIL=9000
MIN_DEPOSIT=10000000

# Parse optional flags
while [[ $# -gt 0 ]]; do
    case "$1" in
        --ltv)          MAX_LTV="$2"; shift 2 ;;
        --liq)          LIQ_THRESHOLD="$2"; shift 2 ;;
        --penalty)      LIQ_PENALTY="$2"; shift 2 ;;
        --interest)     INTEREST_RATE="$2"; shift 2 ;;
        --duration)     MAX_DURATION="$2"; shift 2 ;;
        --risk)         RISK_DISCOUNT="$2"; shift 2 ;;
        --reserve)      RESERVE_FACTOR="$2"; shift 2 ;;
        --max-util)     MAX_UTIL="$2"; shift 2 ;;
        --min-deposit)  MIN_DEPOSIT="$2"; shift 2 ;;
        *) echo -e "${RED}Unknown option: $1${NC}"; exit 1 ;;
    esac
done

echo "============================================"
echo "  Deploy Pool: $POOL_NAME"
echo "============================================"

# ============================================================
# Pre-checks
# ============================================================

if ! command -v stellar &>/dev/null; then
    echo -e "${RED}Error: stellar CLI not found.${NC}"
    exit 1
fi

if [ ! -f "target/wasm/lending_vault.wasm" ] || [ ! -f "target/wasm/borrow_contract.wasm" ]; then
    echo -e "${RED}Error: WASM files not found. Run ./build.sh first.${NC}"
    exit 1
fi

# Need the existing receivable token contract
if [ ! -f "deployed_contracts.json" ]; then
    echo -e "${RED}Error: deployed_contracts.json not found. Run ./deploy.sh first for the base deployment.${NC}"
    exit 1
fi

RECV_CONTRACT=$(python3 -c "import json; print(json.load(open('deployed_contracts.json'))['contracts']['receivable_token'])")
if [ -z "$RECV_CONTRACT" ]; then
    echo -e "${RED}Error: Could not read receivable_token contract from deployed_contracts.json${NC}"
    exit 1
fi

ADMIN=$(get_address "admin")
if [ -z "$ADMIN" ]; then
    echo -e "${RED}Error: admin identity not found. Run ./setup.sh first.${NC}"
    exit 1
fi

# ============================================================
# Resolve asset contract address
# ============================================================

print_step 1 "Resolving asset address..."

if [ "$ASSET" = "native" ]; then
    ASSET_CONTRACT=$(stellar contract id asset --asset native --network $NETWORK)
    ASSET_LABEL="Native XLM"
else
    ASSET_CONTRACT=$(stellar contract id asset --asset "$ASSET" --network $NETWORK)
    ASSET_LABEL="$ASSET"
fi

print_info "Asset: $ASSET_LABEL"
print_info "Asset contract: $ASSET_CONTRACT"

# ============================================================
# Deploy Lending Vault
# ============================================================

print_step 2 "Deploying Lending Vault ($POOL_NAME)..."

VAULT_WASM_HASH=$(stellar contract install \
    --wasm target/wasm/lending_vault.wasm \
    --source admin \
    --network $NETWORK)
print_info "WASM hash: $VAULT_WASM_HASH"

VAULT_CONTRACT=$(stellar contract deploy \
    --wasm-hash $VAULT_WASM_HASH \
    --source admin \
    --network $NETWORK)
print_info "Vault contract: $VAULT_CONTRACT"

stellar contract invoke \
    --id $VAULT_CONTRACT \
    --source admin \
    --network $NETWORK \
    -- \
    initialize \
    --admin $ADMIN \
    --base_asset $ASSET_CONTRACT \
    --reserve_factor $RESERVE_FACTOR \
    --max_utilization $MAX_UTIL \
    --min_deposit $MIN_DEPOSIT

print_info "✓ Vault initialized (${RESERVE_FACTOR}bps reserve, ${MAX_UTIL}bps max util)"

# ============================================================
# Deploy Borrow Contract
# ============================================================

print_step 3 "Deploying Borrow Contract ($POOL_NAME)..."

BORROW_WASM_HASH=$(stellar contract install \
    --wasm target/wasm/borrow_contract.wasm \
    --source admin \
    --network $NETWORK)
print_info "WASM hash: $BORROW_WASM_HASH"

BORROW_CONTRACT=$(stellar contract deploy \
    --wasm-hash $BORROW_WASM_HASH \
    --source admin \
    --network $NETWORK)
print_info "Borrow contract: $BORROW_CONTRACT"

stellar contract invoke \
    --id $BORROW_CONTRACT \
    --source admin \
    --network $NETWORK \
    -- \
    initialize \
    --admin $ADMIN \
    --recv_contract $RECV_CONTRACT \
    --vault_contract $VAULT_CONTRACT \
    --config "{\"max_ltv\":\"$MAX_LTV\",\"liquidation_threshold\":\"$LIQ_THRESHOLD\",\"liquidation_penalty\":\"$LIQ_PENALTY\",\"base_interest_rate\":\"$INTEREST_RATE\",\"max_loan_duration\":$MAX_DURATION,\"risk_discount_factor\":\"$RISK_DISCOUNT\"}"

print_info "✓ Borrow Contract initialized (${MAX_LTV}bps LTV, ${LIQ_THRESHOLD}bps liq, ${INTEREST_RATE}bps APR)"

# ============================================================
# Cross-authorize contracts
# ============================================================

print_step 4 "Setting cross-contract authorizations..."

# Receivable Token → authorize this borrow contract (add_borrow supports multiple)
stellar contract invoke \
    --id $RECV_CONTRACT \
    --source admin \
    --network $NETWORK \
    -- \
    add_borrow \
    --borrow_contract $BORROW_CONTRACT

print_info "✓ Receivable Token: authorized $POOL_NAME borrow contract"

# Vault → allow this borrow contract to disburse/repay
stellar contract invoke \
    --id $VAULT_CONTRACT \
    --source admin \
    --network $NETWORK \
    -- \
    set_borrow \
    --borrow_contract $BORROW_CONTRACT

print_info "✓ Vault: authorized borrow contract"

# ============================================================
# Save pool config
# ============================================================

POOL_FILE="pool_${POOL_NAME}.json"
cat > "$POOL_FILE" << EOF
{
    "pool_name": "$POOL_NAME",
    "network": "testnet",
    "rpc_url": "https://soroban-testnet.stellar.org:443",
    "contracts": {
        "receivable_token": "$RECV_CONTRACT",
        "lending_vault": "$VAULT_CONTRACT",
        "borrow_contract": "$BORROW_CONTRACT"
    },
    "asset": {
        "label": "$ASSET_LABEL",
        "contract": "$ASSET_CONTRACT"
    },
    "config": {
        "max_ltv_bps": $MAX_LTV,
        "liquidation_threshold_bps": $LIQ_THRESHOLD,
        "liquidation_penalty_bps": $LIQ_PENALTY,
        "interest_rate_bps": $INTEREST_RATE,
        "reserve_factor_bps": $RESERVE_FACTOR,
        "max_utilization_bps": $MAX_UTIL,
        "max_loan_duration_secs": $MAX_DURATION,
        "min_deposit_stroops": $MIN_DEPOSIT
    }
}
EOF

# ============================================================
# Summary
# ============================================================

echo ""
echo "============================================"
echo -e "${GREEN}  Pool '$POOL_NAME' Deployed!${NC}"
echo "============================================"
echo ""
echo "Contract Addresses:"
print_addr "Receivable Token (shared)" "$RECV_CONTRACT"
print_addr "Lending Vault            " "$VAULT_CONTRACT"
print_addr "Borrow Contract          " "$BORROW_CONTRACT"
echo ""
echo "Config:"
echo "  Asset:        $ASSET_LABEL ($ASSET_CONTRACT)"
echo "  Max LTV:      $(echo "scale=1; $MAX_LTV / 100" | bc)%"
echo "  Liq Threshold:$(echo "scale=1; $LIQ_THRESHOLD / 100" | bc)%"
echo "  Interest:     $(echo "scale=1; $INTEREST_RATE / 100" | bc)%"
echo "  Reserve:      $(echo "scale=1; $RESERVE_FACTOR / 100" | bc)%"
echo ""
echo "Pool config saved to: $POOL_FILE"
echo ""
