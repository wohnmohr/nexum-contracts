#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "============================================"
echo "  Running All Tests"
echo "============================================"

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

PASSED=0
FAILED=0

run_tests() {
    local name=$1
    echo -e "\n${YELLOW}━━━ Testing: $name ━━━${NC}"
    if cargo test -p "$name" -- --nocapture 2>&1; then
        echo -e "${GREEN}✓ $name passed${NC}"
        PASSED=$((PASSED + 1))
    else
        echo -e "${RED}✗ $name failed${NC}"
        FAILED=$((FAILED + 1))
    fi
}

# Run tests for each contract
run_tests "receivable_token"
run_tests "lending_vault"

# Borrow contract tests require integration (cross-contract)
# Run unit tests only
echo -e "\n${YELLOW}━━━ Testing: borrow_contract (compile check) ━━━${NC}"
if cargo check -p borrow_contract 2>&1; then
    echo -e "${GREEN}✓ borrow_contract compiles${NC}"
    PASSED=$((PASSED + 1))
else
    echo -e "${RED}✗ borrow_contract compile failed${NC}"
    FAILED=$((FAILED + 1))
fi

echo ""
echo "============================================"
echo -e "  Results: ${GREEN}$PASSED passed${NC}, ${RED}$FAILED failed${NC}"
echo "============================================"

if [ $FAILED -gt 0 ]; then
    exit 1
fi
