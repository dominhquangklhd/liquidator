#!/bin/bash

# ============================================================================
# CRASH ETH PRICE - Make Position Liquidatable (Bash Version)
# ============================================================================
#
# Script nay crash gia ETH de position tro nen liquidatable:
#   1. Get WETH price source from Aave Oracle
#   2. Set crashed price via storage slot manipulation
#   3. Mine new block to ensure state is reflected
#   4. Trigger Aave event (repay) to seed user into Risk Engine
#   5. Verify HF < 1.0 (position liquidatable)
#
# Yeu cau: 
#   - Anvil dang chay (./scripts-ubuntu/start_anvil.sh)
#   - Da chay setup_liquidation_scenario.sh
#
# Cach dung:
#   ./scripts-ubuntu/crash_price.sh                    # Drop 25% (default)
#   ./scripts-ubuntu/crash_price.sh -d 30              # Drop 30%
#   ./scripts-ubuntu/crash_price.sh -r "http://..." -d 40
# ============================================================================

set -euo pipefail

# Auto-load project .env (if present)
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [ -f "$PROJECT_ROOT/.env" ]; then
    set -a
    . "$PROJECT_ROOT/.env"
    set +a
fi

RPC_URL="http://127.0.0.1:8545"
NETWORK="auto"
PRICE_DROP=25

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -r|--rpc-url)
            RPC_URL="$2"
            shift 2
            ;;
        -n|--network)
            NETWORK="$2"
            shift 2
            ;;
        -d|--drop)
            PRICE_DROP="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# ============================================================================
# NETWORK CONFIGURATION
# ============================================================================

declare -A MAINNET_CONFIG=(
    [AAVE_POOL]="0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2"
    [AAVE_ORACLE]="0x54586bE62E3c3580375aE3723C145253060Ca0C2"
    [WETH]="0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"
    [USDC]="0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"
    [ETH_USD_FEED]="0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419"
    [NetworkName]="Ethereum Mainnet"
)

declare -A SEPOLIA_CONFIG=(
    [AAVE_POOL]="0x6Ae43d3271ff6888e7Fc43Fd7321a503ff738951"
    [AAVE_ORACLE]="0x2da88497588bf89281816106C7259e31AF45a663"
    [WETH]="0xC558DBdd856501FCd9aaF1E62eae57A9F0629a3c"
    [USDC]="0x94a9D9AC8a22534E3FaCa9F4e7F2E2cf85d5E4C8"
    [ETH_USD_FEED]="0x694AA1769357215DE4FAC081bf1f309aDC325306"
    [NetworkName]="Sepolia Testnet"
)

# Test accounts
BORROWER="0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"
BORROWER_KEY="0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"
DEPLOYER_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"

# ============================================================================
# HELPER FUNCTIONS
# ============================================================================

cast_call() {
    cast call "$@" --rpc-url "$RPC_URL" 2>&1
}

cast_send() {
    local key="$1"
    shift
    cast send "$@" --private-key "$key" --rpc-url "$RPC_URL" 2>&1
}

cast_rpc() {
    cast rpc "$@" --rpc-url "$RPC_URL" 2>&1
}

strip_cast_annotation() {
    local v="$1"
    echo "$v" | sed -E 's/\[.*\]//g' | tr -d '\r' | xargs
}

first_int() {
    local v="$1"
    strip_cast_annotation "$v" | grep -Eo -- '-?[0-9]+' | head -n 1
}

decimal_to_hex() {
    local num=$1
    printf "0x%x" "$num"
}

# ============================================================================
# DETECT NETWORK
# ============================================================================

if [ "$NETWORK" = "auto" ]; then
    CHAIN_ID=$(cast chain-id --rpc-url "$RPC_URL" 2>/dev/null || echo "1")
    
    if [ "$CHAIN_ID" = "11155111" ]; then
        NETWORK="sepolia"
    else
        NETWORK="mainnet"
    fi
fi

# Load config
if [ "$NETWORK" = "sepolia" ]; then
    declare -n CONFIG=SEPOLIA_CONFIG
else
    declare -n CONFIG=MAINNET_CONFIG
fi

AAVE_POOL="${CONFIG[AAVE_POOL]}"
AAVE_ORACLE="${CONFIG[AAVE_ORACLE]}"
WETH="${CONFIG[WETH]}"
USDC="${CONFIG[USDC]}"
ETH_USD_FEED="${CONFIG[ETH_USD_FEED]}"
NETWORK_NAME="${CONFIG[NetworkName]}"

echo "============================================"
echo "  CRASH ETH PRICE - LIQUIDATION TRIGGER"
echo "============================================"
echo ""
echo "  Price Drop: ${PRICE_DROP}%"
echo "  [OK] Connected to $NETWORK_NAME (Chain ID: $CHAIN_ID)"
echo "  [i] ETH/USD Feed: $ETH_USD_FEED"
echo ""

# ============================================================================
# STEP 1: Get current ETH price
# ============================================================================
echo "--------------------------------------"
echo "  STEP 1/5: Get current ETH price"
echo "--------------------------------------"
echo ""

ETH_PRICE_RAW=$(cast_call "$ETH_USD_FEED" "latestAnswer()(int256)")
ETH_PRICE_RAW=$(first_int "$ETH_PRICE_RAW")
ETH_PRICE=$((ETH_PRICE_RAW / 100000000))
echo "  [\$] ETH/USD hien tai: \$$ETH_PRICE"

# Calculate new price after crash
NEW_ETH_PRICE=$((ETH_PRICE * (100 - PRICE_DROP) / 100))
echo "  [CRASH] Gia moi sau khi crash ${PRICE_DROP}%: \$$NEW_ETH_PRICE"
echo ""

# Convert to 8-decimal format (Chainlink uses 8 decimals)
NEW_PRICE_WEI=$((NEW_ETH_PRICE * 100000000))
NEW_PRICE_HEX=$(decimal_to_hex "$NEW_PRICE_WEI")

# ============================================================================
# STEP 2: Check Health Factor before crash
# ============================================================================
echo "--------------------------------------"
echo "  STEP 2/5: Check Health Factor truoc crash"
echo "--------------------------------------"
echo ""

ACCOUNT_DATA=$(cast_call "$AAVE_POOL" "getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)" "$BORROWER")
echo "  [i] Borrower Account before crash:"
echo "      $ACCOUNT_DATA"
echo ""

# Parse HF (from cast output - last value is HF)
HF_RAW=$(strip_cast_annotation "$ACCOUNT_DATA" | tr '(),\n\t' '    ' | awk '{for(i=NF;i>=1;i--) if($i ~ /^-?[0-9]+$/){print $i; break}}')
HF_BEFORE=$(echo "scale=4; $HF_RAW / 1000000000000000000" | bc 2>/dev/null || echo "N/A")
echo "     Health Factor: $HF_BEFORE"
echo ""

# ============================================================================
# STEP 3: Get WETH price source & Replace with Mock
# ============================================================================
echo "--------------------------------------"
echo "  STEP 3/5: Get WETH Price Source from Aave Oracle"
echo "--------------------------------------"
echo ""

WETH_PRICE_SOURCE=$(cast_call "$AAVE_ORACLE" "getSourceOfAsset(address)(address)" "$WETH")
WETH_PRICE_SOURCE=$(strip_cast_annotation "$WETH_PRICE_SOURCE")
echo "  [i] Aave WETH Price Source: $WETH_PRICE_SOURCE"
echo ""

# Option: If you want bytecode replacement (complex), uncomment below:
# echo "  [>] Replacing bytecode with MockPriceFeed..."
# This requires compiled MockPriceFeed bytecode, not shown for brevity

# ============================================================================
# STEP 4: Set Crashed Price via Storage Slots
# ============================================================================
echo "--------------------------------------"
echo "  STEP 4/5: Set Crashed Price via Storage"
echo "--------------------------------------"
echo ""

echo "  [>] Setting storage slots on $WETH_PRICE_SOURCE..."
echo "      MockPriceFeed storage layout:"
echo "      - slot 0: _answer (int256)"
echo "      - slot 1: _decimals (uint8)"
echo "      - slot 4: _roundId (uint80)"
echo "      - slot 5: _updatedAt (uint256)"
echo ""

# Set slot 0: _answer = newPrice
cast_rpc anvil_setStorageAt "$WETH_PRICE_SOURCE" 0x0000000000000000000000000000000000000000000000000000000000000000 "$NEW_PRICE_HEX" > /dev/null
echo "  [OK] Slot 0 (_answer): $NEW_ETH_PRICE"

# Set slot 1: _decimals = 8
cast_rpc anvil_setStorageAt "$WETH_PRICE_SOURCE" 0x0000000000000000000000000000000000000000000000000000000000000001 0x08 > /dev/null
echo "  [OK] Slot 1 (_decimals): 8"

# Set slot 4: _roundId = 1
cast_rpc anvil_setStorageAt "$WETH_PRICE_SOURCE" 0x0000000000000000000000000000000000000000000000000000000000000004 0x01 > /dev/null
echo "  [OK] Slot 4 (_roundId): 1"

# Set slot 5: _updatedAt = current block timestamp
BLOCK_NUM=$(cast block-number --rpc-url "$RPC_URL")
BLOCK_DATA=$(cast block "$BLOCK_NUM" --json --rpc-url "$RPC_URL")
TIMESTAMP=$(echo "$BLOCK_DATA" | grep -oP '"timestamp":"0x\K[^"]+')
TIMESTAMP_DEC=$((16#$TIMESTAMP))
TIMESTAMP_HEX=$(decimal_to_hex "$TIMESTAMP_DEC")

cast_rpc anvil_setStorageAt "$WETH_PRICE_SOURCE" 0x0000000000000000000000000000000000000000000000000000000000000005 "$TIMESTAMP_HEX" > /dev/null
echo "  [OK] Slot 5 (_updatedAt): $TIMESTAMP_DEC"

# Mine new block to ensure state is reflected
echo "  [>] Mining new block..."
cast_rpc anvil_mine 1 > /dev/null
echo "  [OK] Block mined"
echo ""

# ============================================================================
# STEP 4.5: Verify price was set correctly
# ============================================================================

NEW_PRICE_CHECK=$(cast_call "$WETH_PRICE_SOURCE" "latestAnswer()(int256)")
NEW_PRICE_CHECK=$(first_int "$NEW_PRICE_CHECK")
NEW_PRICE_CHECK_USD=$((NEW_PRICE_CHECK / 100000000))

echo "  [\$] ETH/USD SAU CRASH: \$$NEW_PRICE_CHECK_USD"
if [ "$NEW_PRICE_CHECK_USD" = "$NEW_ETH_PRICE" ]; then
    echo "  [OK] Gia da duoc cap nhat thanh cong!"
else
    echo "  [!] Gia khac expected: expected=\$$NEW_ETH_PRICE, actual=\$$NEW_PRICE_CHECK_USD"
fi
echo ""

# ============================================================================
# STEP 5: Check Health Factor after crash & Trigger Aave Event
# ============================================================================
echo "--------------------------------------"
echo "  STEP 5/5: Check HF & Trigger Event"
echo "--------------------------------------"
echo ""

ACCOUNT_DATA=$(cast_call "$AAVE_POOL" "getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)" "$BORROWER")
echo "  [i] Borrower Account AFTER crash:"
echo "      $ACCOUNT_DATA"
echo ""

HF_RAW=$(strip_cast_annotation "$ACCOUNT_DATA" | tr '(),\n\t' '    ' | awk '{for(i=NF;i>=1;i--) if($i ~ /^-?[0-9]+$/){print $i; break}}')
HF_AFTER=$(echo "scale=4; $HF_RAW / 1000000000000000000" | bc 2>/dev/null || echo "N/A")
echo "     Health Factor: $HF_AFTER"
echo ""

if [ "$HF_AFTER" != "N/A" ]; then
    # Check if liquidatable (HF < 1.0)
    HF_NUM=$(echo "$HF_AFTER" | cut -d. -f1)
    if [ "$HF_NUM" -lt 1 ]; then
        echo "  !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!"
        echo "  !  POSITION IS NOW LIQUIDATABLE   !"
        echo "  !  Health Factor: $HF_AFTER"
        echo "  !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!"
    else
        echo "  [!] Health Factor van >= 1.0: $HF_AFTER"
        echo "      Try increasing PRICE_DROP: -d 40"
    fi
fi
echo ""

# Trigger Aave event to seed user into Risk Engine
echo "  [>] Triggering Aave event to notify liquidator..."
echo "      (System needs events to discover users to monitor)"

USDC_ALLOWANCE=$(cast_call "$USDC" "allowance(address,address)(uint256)" "$BORROWER" "$AAVE_POOL")
USDC_ALLOWANCE=$(first_int "$USDC_ALLOWANCE")

if [ "$USDC_ALLOWANCE" -lt 1 ]; then
    # Approve USDC
    cast_send "$DEPLOYER_KEY" "$USDC" "approve(address,uint256)" "$AAVE_POOL" 1 > /dev/null 2>&1
fi

# Do small repay to trigger event
if cast_send "$DEPLOYER_KEY" "$AAVE_POOL" "repay(address,uint256,uint256,address)" "$USDC" 1 2 "$BORROWER" > /dev/null 2>&1; then
    echo "  [OK] Repay event emitted"
else
    echo "  [!] Repay trigger failed (non-critical)"
fi

sleep 1

# ============================================================================
# SUMMARY
# ============================================================================
echo ""
echo "============================================"
echo "  PRICE CRASH COMPLETE"
echo "============================================"
echo ""
echo "  [*] Ket qua:"
echo "     ETH Before:  \$$ETH_PRICE"
echo "     ETH After:   \$$NEW_ETH_PRICE (target)"
echo "     Drop:        ${PRICE_DROP}%"
echo ""
echo "     HF Before:   $HF_BEFORE"
echo "     HF After:    $HF_AFTER"
echo ""
echo "  [>] Next step:"
echo "     cargo run"
echo ""
echo "  [i] To reset price:"
echo "     cast send $ETH_USD_FEED 'setAnswer(int256)' $ETH_PRICE_RAW --private-key $DEPLOYER_KEY --rpc-url $RPC_URL"
echo ""
