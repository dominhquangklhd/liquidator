#!/usr/bin/env bash

# ============================================================================
# SETUP LIQUIDATION SCENARIO (Full Bash Version - no Python)
# ============================================================================
#
# Script tao kich ban liquidation tren Anvil fork:
#   0) Kiem tra pool USDC liquidity + tinh toan collateral/debt
#   1) Wrap ETH -> WETH cho borrower
#   2) Approve WETH cho Aave Pool
#   3) Supply WETH lam collateral
#   4) Borrow USDC + day HF sat 1.0
#   4b) Vay them de day HF sat 1.0
#   4c) Rut bot collateral de tinh chinh HF
#   5) Setup liquidator wallet (USDC + approval)
#   6) Kiem tra trang thai cuoi
#   7) Tao snapshot
#
# Yeu cau:
#   - Anvil dang chay (scripts-ubuntu/start_anvil.sh)
#   - Foundry cast da duoc cai
#   - awk/sed/sort (co san tren Git Bash)
# ============================================================================

set -euo pipefail

# Auto-load project .env (if present)
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ -f "$PROJECT_ROOT/.env" ]]; then
    set -a
    # shellcheck disable=SC1091
    . "$PROJECT_ROOT/.env"
    set +a
fi

RPC_URL="http://127.0.0.1:8545"
NETWORK="auto"

while [[ $# -gt 0 ]]; do
    case "$1" in
        -r|--rpc-url)
            RPC_URL="$2"
            shift 2
            ;;
        -n|--network)
            NETWORK="$2"
            shift 2
            ;;
        -h|--help)
            echo "Usage: ./scripts-ubuntu/setup_liquidation_scenario.sh [-r RPC_URL] [-n mainnet|sepolia]"
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

# Mainnet config
AAVE_POOL_MAINNET="0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2"
AAVE_ORACLE_MAINNET="0x54586bE62E3c3580375aE3723C145253060Ca0C2"
POOL_ADDR_PROVIDER_MAINNET="0x2f39d218133AFaB8F2B819B1066c7E434Ad94E9e"
ACL_MANAGER_MAINNET="0xc2aaCf6553D20d1e9571216f576571920c0FBB3d"
WETH_MAINNET="0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"
USDC_MAINNET="0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"
WBTC_MAINNET="0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599"
aWETH_MAINNET="0x4d5F47FA6A74757f35C14fD3a6Ef8E3C9BC514E8"
aUSDC_MAINNET="0x98C23E9d8f34FEFb1B7BD6a91B7FF122F4e16F5c"
ETH_USD_FEED_MAINNET="0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419"
USDC_BALANCE_SLOT_MAINNET="9"
NETWORK_NAME_MAINNET="Ethereum Mainnet"

# Sepolia config
AAVE_POOL_SEPOLIA="0x6Ae43d3271ff6888e7Fc43Fd7321a503ff738951"
AAVE_ORACLE_SEPOLIA="0x2da88497588bf89281816106C7259e31AF45a663"
POOL_ADDR_PROVIDER_SEPOLIA="0x012bAC54348C0E635dCAc9D5FB99f06F24136C9A"
ACL_MANAGER_SEPOLIA="0x7F2bE3b178deeFF716CD6Ff03Ef79A1dFf360ddD"
WETH_SEPOLIA="0xC558DBdd856501FCd9aaF1E62eae57A9F0629a3c"
USDC_SEPOLIA="0x94a9D9AC8a22534E3FaCa9F4e7F2E2cf85d5E4C8"
WBTC_SEPOLIA="0x29f2D40B0605204364af54EC677bD022dA425d03"
aWETH_SEPOLIA="0x5b071b590a59395fE4025A0Ccc1FcC931AAc1830"
aUSDC_SEPOLIA="0x16da4541aD1807f4443d92D26044C1147406EB80"
ETH_USD_FEED_SEPOLIA="0x694AA1769357215DE4FAC081bf1f309aDC325306"
USDC_BALANCE_SLOT_SEPOLIA="0"
NETWORK_NAME_SEPOLIA="Sepolia Testnet"

# Anvil default accounts
BORROWER="0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"
BORROWER_KEY="0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"
LIQUIDATOR="0x90F79bf6EB2c4f870365E785982E1f101E93b906"
LIQUIDATOR_KEY="0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6"

# ----------------------------- helpers ---------------------------------------

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || { echo "[X] Missing command: $1" >&2; exit 1; }
}

strip_cast_annotation() {
    local v="$1"
    echo "$v" | sed -E 's/\[.*\]//g' | tr -d '\r' | xargs
}

parse_cast_values() {
    local raw="$1"
    strip_cast_annotation "$raw" \
        | tr '(),\n\t' '    ' \
        | awk '{for(i=1;i<=NF;i++) if($i ~ /^-?[0-9]+$/) print $i}'
}

fmt_money_2() {
    local v="$1"
    local d="$2"
    awk -v v="$v" -v d="$d" 'BEGIN{printf "%.2f", v/(10^d)}'
}

fmt_hf_4() {
    local v="$1"
    awk -v v="$v" 'BEGIN{printf "%.4f", v/1e18}'
}

float_lt() {
    awk -v a="$1" -v b="$2" 'BEGIN{exit (a < b) ? 0 : 1}'
}

float_le() {
    awk -v a="$1" -v b="$2" 'BEGIN{exit (a <= b) ? 0 : 1}'
}

float_gt() {
    awk -v a="$1" -v b="$2" 'BEGIN{exit (a > b) ? 0 : 1}'
}

int_min() {
    awk -v a="$1" -v b="$2" 'BEGIN{if (a<b) printf "%.0f", a; else printf "%.0f", b}'
}

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

write_step() {
    echo
    echo "----------------------------------------"
    echo "  STEP $1 : $2"
    echo "----------------------------------------"
}

write_account_data() {
    local raw="$1"
    mapfile -t vals < <(parse_cast_values "$raw")
    if [[ ${#vals[@]} -lt 6 ]]; then
        echo "     $raw"
        return
    fi

    local total_collateral_usd total_debt_usd available_borrow_usd liq_threshold ltv hf_raw hf_display
    total_collateral_usd=$(fmt_money_2 "${vals[0]}" 8)
    total_debt_usd=$(fmt_money_2 "${vals[1]}" 8)
    available_borrow_usd=$(fmt_money_2 "${vals[2]}" 8)
    liq_threshold=$(awk -v v="${vals[3]}" 'BEGIN{printf "%.2f", v/100}')
    ltv=$(awk -v v="${vals[4]}" 'BEGIN{printf "%.2f", v/100}')
    hf_raw="${vals[5]}"

    if [[ ${#hf_raw} -gt 30 ]]; then
        hf_display="Infinity (no debt)"
    else
        hf_display=$(fmt_hf_4 "$hf_raw")
    fi

    echo "     Total Collateral:   \$$total_collateral_usd"
    echo "     Total Debt:         \$$total_debt_usd"
    echo "     Available Borrow:   \$$available_borrow_usd"
    echo "     Liq. Threshold:     ${liq_threshold}%"
    echo "     LTV:                ${ltv}%"
    echo "     Health Factor:      $hf_display"
}

get_health_factor() {
    local raw="$1"
    mapfile -t vals < <(parse_cast_values "$raw")
    if [[ ${#vals[@]} -lt 6 ]]; then
        echo "999999.0"
        return
    fi
    local hf_raw="${vals[5]}"
    if [[ ${#hf_raw} -gt 30 ]]; then
        echo "999999.0"
    else
        fmt_hf_4 "$hf_raw"
    fi
}

# ----------------------------- precheck --------------------------------------

echo "============================================"
echo "  SETUP LIQUIDATION SCENARIO"
echo "============================================"

require_cmd cast
require_cmd awk
require_cmd sed
require_cmd sort

chain_id=$(cast chain-id --rpc-url "$RPC_URL" 2>/dev/null || echo "1")

if [[ "$NETWORK" == "auto" ]]; then
    if [[ "$chain_id" == "11155111" ]]; then
        NETWORK="sepolia"
    else
        NETWORK="mainnet"
    fi
fi

if [[ "$NETWORK" == "sepolia" ]]; then
    AAVE_POOL="$AAVE_POOL_SEPOLIA"
    AAVE_ORACLE="$AAVE_ORACLE_SEPOLIA"
    POOL_ADDRESSES_PROVIDER="$POOL_ADDR_PROVIDER_SEPOLIA"
    ACL_MANAGER="$ACL_MANAGER_SEPOLIA"
    WETH="$WETH_SEPOLIA"
    USDC="$USDC_SEPOLIA"
    WBTC="$WBTC_SEPOLIA"
    aWETH="$aWETH_SEPOLIA"
    aUSDC="$aUSDC_SEPOLIA"
    ETH_USD_FEED="$ETH_USD_FEED_SEPOLIA"
    USDC_BALANCE_SLOT="$USDC_BALANCE_SLOT_SEPOLIA"
    NETWORK_NAME="$NETWORK_NAME_SEPOLIA"
else
    AAVE_POOL="$AAVE_POOL_MAINNET"
    AAVE_ORACLE="$AAVE_ORACLE_MAINNET"
    POOL_ADDRESSES_PROVIDER="$POOL_ADDR_PROVIDER_MAINNET"
    ACL_MANAGER="$ACL_MANAGER_MAINNET"
    WETH="$WETH_MAINNET"
    USDC="$USDC_MAINNET"
    WBTC="$WBTC_MAINNET"
    aWETH="$aWETH_MAINNET"
    aUSDC="$aUSDC_MAINNET"
    ETH_USD_FEED="$ETH_USD_FEED_MAINNET"
    USDC_BALANCE_SLOT="$USDC_BALANCE_SLOT_MAINNET"
    NETWORK_NAME="$NETWORK_NAME_MAINNET"
fi

echo "[OK] Connected to $NETWORK_NAME (Chain ID: $chain_id)"
echo "[i] Aave Pool: $AAVE_POOL"

# ----------------------------- step 0 ----------------------------------------

write_step "0/8" "Kiem tra USDC liquidity & tinh toan"

pool_usdc_raw=$(cast_call "$USDC" "balanceOf(address)(uint256)" "$aUSDC")
pool_usdc_amount=$(parse_cast_values "$pool_usdc_raw" | head -n 1)
pool_usdc_usd=$(fmt_money_2 "$pool_usdc_amount" 6)
echo "  [i] USDC kha dung trong Pool: $pool_usdc_usd USDC"

if ! awk -v v="$pool_usdc_amount" 'BEGIN{exit (v >= 1000000) ? 0 : 1}'; then
    echo "  [X] Pool khong co du USDC liquidity!"
    exit 1
fi

eth_price_raw=$(cast_call "$AAVE_ORACLE" "getAssetPrice(address)(uint256)" "$WETH")
eth_price_base=$(parse_cast_values "$eth_price_raw" | head -n 1)
eth_price_usd=$(fmt_money_2 "$eth_price_base" 8)
echo "  [i] ETH price (Aave Oracle): \$$eth_price_usd"

ltv_ratio="0.80"
max_supply_eth="50"

max_collateral_usd=$(awk -v m="$max_supply_eth" -v e="$eth_price_usd" 'BEGIN{printf "%.8f", m*e}')
max_borrow_from_collateral=$(awk -v c="$max_collateral_usd" -v l="$ltv_ratio" 'BEGIN{printf "%.0f", c*l*1e6}')
pool_target=$(awk -v p="$pool_usdc_amount" 'BEGIN{printf "%.0f", p*0.90}')
borrow_target_usdc6=$(int_min "$pool_target" "$max_borrow_from_collateral")
borrow_target_usd=$(fmt_money_2 "$borrow_target_usdc6" 6)
echo "  [i] Borrow target: $borrow_target_usd USDC"

needed_collateral_usd=$(awk -v b="$borrow_target_usd" -v l="$ltv_ratio" 'BEGIN{printf "%.8f", b/l}')
needed_weth_eth=$(awk -v c="$needed_collateral_usd" -v e="$eth_price_usd" 'BEGIN{printf "%.8f", (c/e)*1.05}')

if float_lt "$needed_weth_eth" "1"; then needed_weth_eth="1"; fi
if float_gt "$needed_weth_eth" "$max_supply_eth"; then needed_weth_eth="$max_supply_eth"; fi
supply_weth_eth=$(awk -v v="$needed_weth_eth" 'BEGIN{printf "%.4f", v}')

# Use cast for wei conversion to avoid integer overflow in shell arithmetic
needed_weth_wei=$(cast to-wei "${supply_weth_eth}ether" 2>/dev/null | xargs)

echo "  [i] Supply: ~$supply_weth_eth WETH (du de vay $borrow_target_usd USDC)"

# ----------------------------- step 1 ----------------------------------------

write_step "1/8" "Wrap ETH -> WETH cho Borrower"

wrap_amount=$(awk -v v="$supply_weth_eth" 'BEGIN{iv=int(v); if(v>iv) iv++; print iv}')

if cast_send "$BORROWER_KEY" "$WETH" "deposit()" --value "${wrap_amount}ether" >/dev/null; then
    echo "  [OK] Wrapped $wrap_amount ETH -> WETH"
else
    echo "  [X] Wrap ETH that bai!"
    exit 1
fi

weth_balance=$(cast_call "$WETH" "balanceOf(address)(uint256)" "$BORROWER")
echo "  [i] Borrower WETH balance: $(strip_cast_annotation "$weth_balance")"

# ----------------------------- step 2 ----------------------------------------

write_step "2/8" "Approve WETH cho Aave Pool"
max_approval="0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"

if cast_send "$BORROWER_KEY" "$WETH" "approve(address,uint256)" "$AAVE_POOL" "$max_approval" >/dev/null; then
    echo "  [OK] WETH approved"
else
    echo "  [X] Approve that bai!"
    exit 1
fi

# ----------------------------- step 3 ----------------------------------------

write_step "3/8" "Supply WETH vao Aave lam Collateral"

echo "  [>] Supplying $supply_weth_eth WETH..."
if cast_send "$BORROWER_KEY" "$AAVE_POOL" "supply(address,uint256,address,uint16)" "$WETH" "$needed_weth_wei" "$BORROWER" 0 >/dev/null; then
    echo "  [OK] Supplied $supply_weth_eth WETH"
else
    echo "  [X] Supply that bai!"
    exit 1
fi

echo "  [>] Enabling WETH as collateral..."
if cast_send "$BORROWER_KEY" "$AAVE_POOL" "setUserUseReserveAsCollateral(address,bool)" "$WETH" true >/dev/null; then
    echo "  [OK] WETH enabled as collateral"
else
    echo "  [!] setCollateral failed (co the da enable)"
fi

account_data=$(cast_call "$AAVE_POOL" "getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)" "$BORROWER")
echo "  [i] Account Data sau khi supply:"
write_account_data "$account_data"

# ----------------------------- step 4 ----------------------------------------

write_step "4/8" "Borrow USDC (gioi han boi pool liquidity)"

pool_usdc_now_raw=$(cast_call "$USDC" "balanceOf(address)(uint256)" "$aUSDC")
pool_usdc_now=$(parse_cast_values "$pool_usdc_now_raw" | head -n 1)

mapfile -t acct_vals < <(parse_cast_values "$account_data")
if [[ ${#acct_vals[@]} -ge 3 ]]; then
    available_borrows_base="${acct_vals[2]}"
    max_borrow_usdc=$(awk -v v="$available_borrows_base" 'BEGIN{printf "%.0f", v/100}')
    borrow_from_capacity=$(awk -v v="$max_borrow_usdc" 'BEGIN{printf "%.0f", v*0.99}')
    borrow_from_pool=$(awk -v v="$pool_usdc_now" 'BEGIN{printf "%.0f", v*0.90}')
    borrow_amount=$(int_min "$borrow_from_capacity" "$borrow_from_pool")

    borrow_amount_usd=$(fmt_money_2 "$borrow_amount" 6)
    max_borrow_usd=$(fmt_money_2 "$max_borrow_usdc" 6)
    pool_now_usd=$(fmt_money_2 "$pool_usdc_now" 6)
    echo "  [i] Max borrow capacity: \$$max_borrow_usd"
    echo "  [i] Pool USDC available:  $pool_now_usd"
    echo "  [>] Borrowing $borrow_amount_usd USDC..."
else
    borrow_amount=$(awk -v v="$pool_usdc_now" 'BEGIN{b=v*0.90; if(b>1000000000) b=1000000000; printf "%.0f", b}')
    borrow_amount_usd=$(fmt_money_2 "$borrow_amount" 6)
    echo "  [!] Fallback: vay $borrow_amount_usd USDC"
fi

if ! cast_send "$BORROWER_KEY" "$AAVE_POOL" "borrow(address,uint256,uint256,uint16,address)" "$USDC" "$borrow_amount" 2 0 "$BORROWER" >/dev/null; then
    echo "  [!] Borrow $borrow_amount_usd that bai, thu 50% pool..."
    borrow_amount=$(awk -v v="$pool_usdc_now" 'BEGIN{printf "%.0f", v*0.50}')
    borrow_amount_usd=$(fmt_money_2 "$borrow_amount" 6)
    cast_send "$BORROWER_KEY" "$AAVE_POOL" "borrow(address,uint256,uint256,uint16,address)" "$USDC" "$borrow_amount" 2 0 "$BORROWER" >/dev/null || {
        echo "  [X] Borrow van that bai!"
        exit 1
    }
fi

echo "  [OK] Borrowed $borrow_amount_usd USDC"

account_data=$(cast_call "$AAVE_POOL" "getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)" "$BORROWER")
echo "  [i] Account Data sau borrow:"
write_account_data "$account_data"

# ---------------------------- step 4b ----------------------------------------

write_step "4b/8" "Vay them USDC de day HF sat 1.0"

total_borrowed_usd="$borrow_amount_usd"
for i in 1 2 3 4 5; do
    pool_usdc_check=$(cast_call "$USDC" "balanceOf(address)(uint256)" "$aUSDC")
    pool_usdc_check_val=$(parse_cast_values "$pool_usdc_check" | head -n 1)

    mapfile -t acct_vals2 < <(parse_cast_values "$account_data")
    if [[ ${#acct_vals2[@]} -lt 6 ]]; then
        break
    fi

    avail_left="${acct_vals2[2]}"
    avail_left_usdc=$(awk -v v="$avail_left" 'BEGIN{printf "%.0f", v/100}')
    pool_cap=$(awk -v v="$pool_usdc_check_val" 'BEGIN{printf "%.0f", v*0.90}')
    extra_by_cap=$(awk -v v="$avail_left_usdc" 'BEGIN{printf "%.0f", v*0.99}')
    extra_borrow=$(int_min "$extra_by_cap" "$pool_cap")

    if ! awk -v v="$extra_borrow" 'BEGIN{exit (v >= 100000) ? 0 : 1}'; then
        echo "  [i] Khong con du de vay them, dung."
        break
    fi

    extra_borrow_usd=$(fmt_money_2 "$extra_borrow" 6)
    echo "  [>] Vay them #$i : $extra_borrow_usd USDC ..."

    if ! cast_send "$BORROWER_KEY" "$AAVE_POOL" "borrow(address,uint256,uint256,uint16,address)" "$USDC" "$extra_borrow" 2 0 "$BORROWER" >/dev/null; then
        echo "  [!] Vay them that bai, dung."
        break
    fi

    total_borrowed_usd=$(awk -v a="$total_borrowed_usd" -v b="$extra_borrow_usd" 'BEGIN{printf "%.2f", a+b}')

    account_data=$(cast_call "$AAVE_POOL" "getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)" "$BORROWER")
    current_hf=$(get_health_factor "$account_data")
    echo "  [i] HF hien tai: $current_hf"

    if float_lt "$current_hf" "1.05"; then
        echo "  [OK] HF da sat 1.0!"
        break
    fi
done

borrow_amount_usd="$total_borrowed_usd"
echo
echo "  [i] Tong no: ~\$$borrow_amount_usd USDC"
echo "  [i] Account Data cuoi:"
write_account_data "$account_data"

final_hf=$(get_health_factor "$account_data")

# ---------------------------- step 4c ----------------------------------------

if float_gt "$final_hf" "1.10"; then
    write_step "4c/8" "Rut bot collateral de day HF xuong ~1.03"

    target_hf="1.03"
    for w in 1 2 3 4 5 6 7 8; do
        mapfile -t wvals < <(parse_cast_values "$account_data")
        if [[ ${#wvals[@]} -lt 6 ]]; then
            break
        fi

        cur_collateral8="${wvals[0]}"
        cur_debt8="${wvals[1]}"
        cur_liq_threshold="${wvals[3]}"

        if ! awk -v v="$cur_debt8" 'BEGIN{exit (v >= 1000000) ? 0 : 1}'; then
            echo "  [!] Debt qua nho, khong can rut collateral."
            break
        fi

        cur_hf=$(get_health_factor "$account_data")
        if float_le "$cur_hf" "1.08"; then
            echo "  [OK] HF = $cur_hf da gan 1.0!"
            break
        fi

        # targetCollateral = targetHF * debt / (liqThreshold / 10000)
        withdraw_amount8=$(awk -v c="$cur_collateral8" -v d="$cur_debt8" -v t="$target_hf" -v lt="$cur_liq_threshold" 'BEGIN{liq=lt/10000; target=t*d/liq; w=c-target; if(w<0) w=0; printf "%.0f", w}')

        if ! awk -v v="$withdraw_amount8" 'BEGIN{exit (v >= 1000000) ? 0 : 1}'; then
            echo "  [i] Khong can rut them."
            break
        fi

        eth_price_now=$(cast_call "$AAVE_ORACLE" "getAssetPrice(address)(uint256)" "$WETH")
        eth_price_now_val=$(parse_cast_values "$eth_price_now" | head -n 1)

        withdraw_weth_wei=$(awk -v w="$withdraw_amount8" -v p="$eth_price_now_val" 'BEGIN{v=(w/p)*1e18; v=v*0.95; if(v<0) v=0; printf "%.0f", v}')

        if ! awk -v v="$withdraw_weth_wei" 'BEGIN{exit (v >= 100000000000000) ? 0 : 1}'; then
            echo "  [i] Withdraw amount qua nho, dung."
            break
        fi

        withdraw_weth_eth=$(awk -v v="$withdraw_weth_wei" 'BEGIN{printf "%.6f", v/1e18}')
        withdraw_usd=$(awk -v v="$withdraw_amount8" 'BEGIN{printf "%.2f", (v/1e8)*0.95}')
        echo "  [>] Rut #$w : $withdraw_weth_eth WETH (~\$$withdraw_usd) ..."

        if ! cast_send "$BORROWER_KEY" "$AAVE_POOL" "withdraw(address,uint256,address)" "$WETH" "$withdraw_weth_wei" "$BORROWER" >/dev/null; then
            echo "  [!] Withdraw that bai (HF qua sat 1.0), dung."
            break
        fi

        account_data=$(cast_call "$AAVE_POOL" "getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)" "$BORROWER")
        cur_hf=$(get_health_factor "$account_data")
        echo "  [i] HF sau rut: $cur_hf"
    done

    account_data=$(cast_call "$AAVE_POOL" "getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)" "$BORROWER")
    echo "  [i] Account Data sau khi rut collateral:"
    write_account_data "$account_data"
    final_hf=$(get_health_factor "$account_data")
fi

needed_drop=$(awk -v hf="$final_hf" 'BEGIN{if(hf<=0){print 0}else{printf "%.0f", (1 - 1/hf)*100}}')

if float_gt "$final_hf" "1.20"; then
    echo
    echo "  [!] HF = $final_hf van cao."
    echo "  [!] Can crash gia ~${needed_drop}% de HF < 1.0"
else
    echo
    echo "  [OK] HF = $final_hf - chi can crash ~${needed_drop}% la liquidatable!"
fi

# ----------------------------- step 5 ----------------------------------------

write_step "5/8" "Setup Liquidator Wallet"

echo "  [>] Setting USDC balance (storage slot $USDC_BALANCE_SLOT)..."
balance_slot=$(cast index address "$LIQUIDATOR" "$USDC_BALANCE_SLOT" 2>&1 | tail -n 1 | xargs)
usdc_hex="0x$(printf '%064s' '746A528800' | tr ' ' '0')"
cast_rpc anvil_setStorageAt "$USDC" "$balance_slot" "$usdc_hex" >/dev/null

liquidator_usdc_raw=$(cast_call "$USDC" "balanceOf(address)(uint256)" "$LIQUIDATOR")
liquidator_usdc_clean=$(parse_cast_values "$liquidator_usdc_raw" | head -n 1)
liquidator_usdc_val=$(fmt_money_2 "$liquidator_usdc_clean" 6)

if float_gt "$liquidator_usdc_val" "0"; then
    echo "  [OK] Liquidator USDC: $liquidator_usdc_val"
else
    echo "  [X] Storage slot $USDC_BALANCE_SLOT incorrect!"
    echo "  [>] Fallback: impersonate aUSDC de transfer..."

    cast_rpc anvil_impersonateAccount "$aUSDC" >/dev/null
    cast_rpc anvil_setBalance "$aUSDC" 0x56BC75E2D63100000 >/dev/null

    transfer_amt=$(awk -v p="$pool_usdc_amount" 'BEGIN{t=p*0.5; if(t>500000000000) t=500000000000; printf "%.0f", t}')

    cast send "$USDC" "transfer(address,uint256)" "$LIQUIDATOR" "$transfer_amt" --from "$aUSDC" --rpc-url "$RPC_URL" >/dev/null 2>&1 || true
    cast_rpc anvil_stopImpersonatingAccount "$aUSDC" >/dev/null

    liquidator_usdc_raw=$(cast_call "$USDC" "balanceOf(address)(uint256)" "$LIQUIDATOR")
    liquidator_usdc_clean=$(parse_cast_values "$liquidator_usdc_raw" | head -n 1)
    liquidator_usdc_val=$(fmt_money_2 "$liquidator_usdc_clean" 6)

    if float_gt "$liquidator_usdc_val" "0"; then
        echo "  [OK] Liquidator USDC (impersonate): $liquidator_usdc_val"
    else
        echo "  [X] Khong set duoc USDC cho Liquidator!"
    fi
fi

if cast_send "$LIQUIDATOR_KEY" "$USDC" "approve(address,uint256)" "$AAVE_POOL" "$max_approval" >/dev/null; then
    echo "  [OK] Liquidator approved USDC"
fi

# ----------------------------- step 6 ----------------------------------------

write_step "6/8" "Kiem tra trang thai cuoi cung"

eth_price_chainlink=$(cast_call "$ETH_USD_FEED" "latestAnswer()(int256)")
eth_price_chainlink_clean=$(parse_cast_values "$eth_price_chainlink" | head -n 1)
eth_price_chainlink_usd=$(fmt_money_2 "$eth_price_chainlink_clean" 8)
echo "  [\$] ETH/USD (Chainlink): \$$eth_price_chainlink_usd"

account_data=$(cast_call "$AAVE_POOL" "getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)" "$BORROWER")
echo "  [i] Borrower Account:"
write_account_data "$account_data"

echo
echo "  [OK] Scenario san sang!"

# ----------------------------- step 7 ----------------------------------------

write_step "7/8" "Tao Snapshot"

snapshot_id=$(cast_rpc anvil_snapshot | tail -n 1 | xargs)
echo "  [*] Snapshot ID: $snapshot_id"
echo "  [i] Rollback: cast rpc anvil_revert $snapshot_id --rpc-url $RPC_URL"

# ----------------------------- summary ---------------------------------------

echo
echo "============================================"
echo "  SCENARIO SETUP COMPLETE"
echo "============================================"
echo
echo "  [i] Tom tat:"
echo "     Borrower:   $BORROWER"
echo "     Collateral: $supply_weth_eth WETH"
echo "     Debt:       ~\$$borrow_amount_usd USDC"
echo "     ETH/USD:    \$$eth_price_chainlink_usd"
echo "     HF:         $final_hf"
echo
echo "     Liquidator: $LIQUIDATOR"
echo "     USDC:       $liquidator_usdc_val"
echo
echo "  --> Buoc tiep theo:"
if float_gt "$final_hf" "1.20"; then
    suggested_drop=$(awk -v n="$needed_drop" 'BEGIN{s=n+5; if(s>95)s=95; printf "%.0f", s}')
    echo "     1. ./scripts-ubuntu/crash_price.sh -d $suggested_drop"
else
    echo "     1. ./scripts-ubuntu/crash_price.sh"
fi
echo "     2. cargo test --test executor_integration -- --nocapture"
echo "     3. cargo run"
