#!/bin/bash
set -e

# Always run from the project root (where this script lives)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Ensure cargo/rustup binaries are on PATH (prefer rustup over Homebrew)
export PATH="$HOME/.cargo/bin:$PATH"

echo "============================================"
echo "  Building Soroban Contracts"
echo "============================================"
echo "  Working dir: $(pwd)"

GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

# Pre-check: stellar CLI required
if ! command -v stellar &>/dev/null; then
    echo -e "${RED}Error: stellar CLI not found. Run ./setup.sh first.${NC}"
    exit 1
fi

# Build and optimize all contracts
CONTRACTS=("receivable_token" "lending_vault" "borrow_contract")
TOTAL=${#CONTRACTS[@]}

for i in "${!CONTRACTS[@]}"; do
    name="${CONTRACTS[$i]}"
    step=$((i + 1))
    echo -e "\n${GREEN}[$step/$TOTAL]${NC} Building $name..."
    stellar contract build --package "$name"
done

# Create output directory and copy WASM files
mkdir -p target/wasm

echo -e "\n${GREEN}[COPY]${NC} Copying WASM artifacts..."
for name in "${CONTRACTS[@]}"; do
    cp "target/wasm32v1-none/release/${name}.wasm" target/wasm/
done

# Optimize
echo -e "\n${GREEN}[OPTIMIZE]${NC} Optimizing WASM files..."
for name in "${CONTRACTS[@]}"; do
    stellar contract optimize --wasm "target/wasm/${name}.wasm"
done

# Show sizes
echo -e "\n${GREEN}[DONE]${NC} Build artifacts:"
ls -lh target/wasm/*.wasm

echo -e "\n${GREEN}Build complete!${NC}"
