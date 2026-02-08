#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "============================================"
echo "  Soroban Receivables Protocol - Setup"
echo "============================================"

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

print_step() { echo -e "\n${GREEN}[STEP]${NC} $1"; }
print_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
print_err()  { echo -e "${RED}[ERROR]${NC} $1"; }

# ============================================================
# 1. Install Rust (if not installed)
# ============================================================
print_step "Checking Rust installation..."
if ! command -v rustc &> /dev/null; then
    echo "Installing Rust..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
else
    echo "Rust already installed: $(rustc --version)"
fi

# ============================================================
# 2. Add wasm32 target
# ============================================================
print_step "Adding wasm32-unknown-unknown target..."
rustup target add wasm32-unknown-unknown

# ============================================================
# 3. Install Stellar CLI (includes Soroban)
# ============================================================
print_step "Installing Stellar CLI..."
if ! command -v stellar &> /dev/null; then
    cargo install --locked stellar-cli
    echo "Stellar CLI installed: $(stellar --version)"
else
    echo "Stellar CLI already installed: $(stellar --version)"
fi

# ============================================================
# 4. Configure Testnet
# ============================================================
print_step "Configuring Stellar Testnet..."
stellar network add \
    --global testnet \
    --rpc-url https://soroban-testnet.stellar.org:443 \
    --network-passphrase "Test SDF Network ; September 2015" \
    2>/dev/null || echo "Testnet network already configured"

# ============================================================
# 5. Generate test identities
# ============================================================
print_step "Generating test identities..."

generate_identity() {
    local name=$1
    if stellar keys address "$name" &>/dev/null 2>&1; then
        echo "  Identity '$name' already exists: $(stellar keys address $name)"
    else
        stellar keys generate "$name" --network testnet --fund 2>/dev/null
        echo "  Created & funded '$name': $(stellar keys address $name)"
    fi
}

generate_identity "admin"
generate_identity "verifier"
generate_identity "creditor"
generate_identity "borrower"
generate_identity "lp1"
generate_identity "lp2"
generate_identity "liquidator"

# ============================================================
# 6. Verify setup
# ============================================================
print_step "Verifying setup..."
echo ""
echo "  Rust:        $(rustc --version)"
echo "  Cargo:       $(cargo --version)"
echo "  Stellar CLI: $(stellar --version)"
echo "  WASM target: $(rustup target list --installed | grep wasm)"
echo ""

echo -e "${GREEN}============================================${NC}"
echo -e "${GREEN}  Setup complete! Next steps:${NC}"
echo -e "${GREEN}============================================${NC}"
echo ""
echo "  1. Build contracts:    ./build.sh"
echo "  2. Run tests:          ./test.sh"
echo "  3. Deploy to testnet:  ./deploy.sh"
echo ""
