#!/bin/bash

# ============================================================================
# START ANVIL - Local Ethereum Fork (Bash Version)
# ============================================================================
#
# Script nay khoi dong Anvil (Foundry) de fork mang Ethereum
# Cho phep test liquidation bot tren mot ban sao thuc te cua blockchain
#
# Yeu cau: 
#   - Foundry da cai dat (https://getfoundry.sh)
#   - RPC URL (Alchemy/Infura key)
#
# Cach dung:
#   ./scripts-ubuntu/start_anvil.sh                    # Mainnet (default)
#   ./scripts-ubuntu/start_anvil.sh -n sepolia         # Sepolia testnet
#   ./scripts-ubuntu/start_anvil.sh -r "YOUR_RPC_URL"
# ============================================================================

set -euo pipefail

# Auto-load project .env (if present)
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [ -f "$PROJECT_ROOT/.env" ]; then
    set -a
    . "$PROJECT_ROOT/.env"
    set +a
fi

# Default values
RPC_URL=""
NETWORK="mainnet"
FORK_BLOCK=0
PORT=8545
ACCOUNTS=10
BALANCE=10000

print_usage() {
    echo "Usage:"
    echo "  ./scripts-ubuntu/start_anvil.sh"
    echo "  ./scripts-ubuntu/start_anvil.sh -n sepolia"
    echo "  ./scripts-ubuntu/start_anvil.sh -r https://..."
    echo "  ./scripts-ubuntu/start_anvil.sh -b 24700000 -p 8545 -a 10 --balance 10000"
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "[X] $1 chua duoc cai dat!" >&2
        exit 1
    }
}

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
        -b|--fork-block)
            FORK_BLOCK="$2"
            shift 2
            ;;
        -p|--port)
            PORT="$2"
            shift 2
            ;;
        -a|--accounts)
            ACCOUNTS="$2"
            shift 2
            ;;
        --balance)
            BALANCE="$2"
            shift 2
            ;;
        -h|--help)
            print_usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            print_usage
            exit 1
            ;;
    esac
done

if [[ "$NETWORK" != "mainnet" && "$NETWORK" != "sepolia" ]]; then
    echo "[X] Network khong hop le: $NETWORK" >&2
    echo "    Chi ho tro: mainnet | sepolia" >&2
    exit 1
fi

for num in "$FORK_BLOCK" "$PORT" "$ACCOUNTS" "$BALANCE"; do
    if ! [[ "$num" =~ ^[0-9]+$ ]]; then
        echo "[X] Gia tri so khong hop le: $num" >&2
        exit 1
    fi
done

if [[ "$PORT" -lt 1 || "$PORT" -gt 65535 ]]; then
    echo "[X] Port khong hop le: $PORT" >&2
    exit 1
fi

# Check if anvil is installed
if ! command -v anvil &> /dev/null; then
    echo "[X] Anvil chua duoc cai dat!" >&2
    echo ""
    echo "Cai dat Foundry:"
    echo "  curl -L https://foundry.paradigm.xyz | bash"
    echo "  foundryup"
    echo ""
    echo "Hoac tren Windows (PowerShell):"
    echo "  Invoke-WebRequest -Uri https://foundry.paradigm.xyz -OutFile foundryup.sh"
    echo "  # Chay trong WSL hoac Git Bash"
    exit 1
fi

require_cmd anvil

# Determine network name and RPC URL
if [ -z "$RPC_URL" ]; then
    if [ "$NETWORK" = "sepolia" ]; then
        RPC_URL="${SEPOLIA_RPC_URL:-}"
        NETWORK_NAME="Sepolia Testnet"
    else
        RPC_URL="${ETH_RPC_URL:-}"
        NETWORK_NAME="Ethereum Mainnet"
    fi
fi

if [ -n "$RPC_URL" ] && ! [[ "$RPC_URL" =~ ^https?:// ]]; then
    echo "[X] RPC URL khong hop le: $RPC_URL" >&2
    echo "    URL phai bat dau bang http:// hoac https://" >&2
    exit 1
fi

# Check RPC URL
if [ -z "$RPC_URL" ]; then
    echo "[!] Khong co RPC URL!" >&2
    echo ""
    echo "Ban can mot RPC URL de fork $NETWORK_NAME. Cach lay:"
    echo "  1. Dang ky tai https://www.alchemy.com (mien phi)"
    echo "  2. Tao app chon $NETWORK_NAME"
    echo "  3. Copy API Key"
    echo ""
    
    if [ "$NETWORK" = "sepolia" ]; then
        echo "Sau do chay:"
        echo "  export SEPOLIA_RPC_URL='https://eth-sepolia.g.alchemy.com/v2/YOUR_KEY'"
        echo "  ./scripts-ubuntu/start_anvil.sh -n sepolia"
    else
        echo "Sau do chay:"
        echo "  export ETH_RPC_URL='https://eth-mainnet.g.alchemy.com/v2/YOUR_KEY'"
        echo "  ./scripts-ubuntu/start_anvil.sh"
    fi
    echo ""
    echo "Hoac chay truc tiep:"
    echo "  ./scripts-ubuntu/start_anvil.sh -r 'YOUR_RPC_URL'"
    exit 1
fi

echo "============================================"
echo "  ANVIL - $NETWORK_NAME Fork"
echo "============================================"
echo ""

# Build command
ANVIL_CMD="anvil --fork-url \"$RPC_URL\" --port $PORT --accounts $ACCOUNTS --balance $BALANCE --steps-tracing"

if [ "$FORK_BLOCK" -gt 0 ]; then
    ANVIL_CMD="$ANVIL_CMD --fork-block-number $FORK_BLOCK"
    echo "[*] Fork tai block: $FORK_BLOCK"
fi

# Preview RPC URL (truncate if too long)
URL_PREVIEW="$RPC_URL"
if [ ${#RPC_URL} -gt 50 ]; then
    URL_PREVIEW="${RPC_URL:0:50}..."
fi

echo "[+] RPC URL: $URL_PREVIEW"
echo "[+] Local RPC: http://127.0.0.1:$PORT"
echo "[+] Accounts: $ACCOUNTS (moi account co $BALANCE ETH)"
echo ""
echo "Cac tai khoan test mac dinh cua Anvil:"
echo "  Account #0: 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
echo "  Private Key: 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
echo "  Account #1: 0x70997970C51812e339D9B73b0245ad59c36d569D"
echo "  Account #2: 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"
echo "  Account #3: 0x90F79bf6EB2c4f870365E785982E1f101E93b906"
echo "  ..."
echo ""
echo "Nhan Ctrl+C de dung Anvil"
echo "============================================"
echo ""

# Run anvil
eval "$ANVIL_CMD"
