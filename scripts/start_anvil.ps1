# ============================================================================
# START ANVIL - Local Ethereum Fork
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
#   .\scripts\start_anvil.ps1                           # Mainnet (default)
#   .\scripts\start_anvil.ps1 -Network sepolia          # Sepolia testnet
#   .\scripts\start_anvil.ps1 -RpcUrl "YOUR_CUSTOM_URL"
# ============================================================================

param(
    [string]$RpcUrl = "",
    [ValidateSet("mainnet", "sepolia")]
    [string]$Network = "mainnet",
    [int]$ForkBlock = 0,
    [int]$Port = 8545,
    [int]$Accounts = 10,
    [int]$Balance = 10000
)

# Kiem tra Foundry da cai chua
if (-not (Get-Command "anvil" -ErrorAction SilentlyContinue)) {
    Write-Host "[X] Anvil chua duoc cai dat!" -ForegroundColor Red
    Write-Host ""
    Write-Host "Cai dat Foundry:" -ForegroundColor Yellow
    Write-Host "  curl -L https://foundry.paradigm.xyz | bash"
    Write-Host "  foundryup"
    Write-Host ""
    Write-Host "Hoac tren Windows (PowerShell):" -ForegroundColor Yellow
    Write-Host "  Invoke-WebRequest -Uri https://foundry.paradigm.xyz -OutFile foundryup.sh"
    Write-Host "  # Chay trong WSL hoac Git Bash"
    exit 1
}

# Kiem tra RPC URL
if ([string]::IsNullOrEmpty($RpcUrl)) {
    # Use environment variable based on network
    if ($Network -eq "sepolia") {
        $RpcUrl = $env:SEPOLIA_RPC_URL
        $networkName = "Sepolia Testnet"
    } else {
        $RpcUrl = $env:ETH_RPC_URL
        $networkName = "Ethereum Mainnet"
    }
}

if ([string]::IsNullOrEmpty($RpcUrl)) {
    Write-Host "[!] Khong co RPC URL!" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "Ban can mot RPC URL de fork $networkName. Cach lay:" -ForegroundColor Cyan
    Write-Host "  1. Dang ky tai https://www.alchemy.com (mien phi)"
    Write-Host "  2. Tao app chon $networkName"
    Write-Host "  3. Copy API Key"
    Write-Host ""
    if ($Network -eq "sepolia") {
        Write-Host "Sau do chay:" -ForegroundColor Green
        Write-Host '  $env:SEPOLIA_RPC_URL = "https://eth-sepolia.g.alchemy.com/v2/YOUR_KEY"'
        Write-Host "  .\scripts\start_anvil.ps1 -Network sepolia"
    } else {
        Write-Host "Sau do chay:" -ForegroundColor Green
        Write-Host '  $env:ETH_RPC_URL = "https://eth-mainnet.g.alchemy.com/v2/YOUR_KEY"'
        Write-Host "  .\scripts\start_anvil.ps1"
    }
    Write-Host ""
    Write-Host "Hoac chay truc tiep:" -ForegroundColor Green
    Write-Host '  .\scripts\start_anvil.ps1 -RpcUrl "YOUR_RPC_URL"'
    exit 1
}

Write-Host "============================================" -ForegroundColor Cyan
Write-Host "  ANVIL - $networkName Fork" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ""

# Build command
$anvilCmd = "anvil --fork-url `"$RpcUrl`" --port $Port --accounts $Accounts --balance $Balance --steps-tracing"

if ($ForkBlock -gt 0) {
    $anvilCmd += " --fork-block-number $ForkBlock"
    Write-Host "[*] Fork tai block: $ForkBlock" -ForegroundColor Yellow
}

$urlPreview = $RpcUrl
if ($RpcUrl.Length -gt 50) {
    $urlPreview = $RpcUrl.Substring(0, 50) + "..."
}

Write-Host "[+] RPC URL: $urlPreview" -ForegroundColor Gray
Write-Host "[+] Local RPC: http://127.0.0.1:$Port" -ForegroundColor Green
Write-Host "[+] Accounts: $Accounts (moi account co $Balance ETH)" -ForegroundColor Green
Write-Host ""
Write-Host "Cac tai khoan test mac dinh cua Anvil:" -ForegroundColor Yellow
Write-Host "  Account #0: 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266" -ForegroundColor Gray
Write-Host "  Private Key: 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80" -ForegroundColor Gray
Write-Host ""
Write-Host "Nhan Ctrl+C de dung Anvil" -ForegroundColor Yellow
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ""

# Khoi chay Anvil
Invoke-Expression $anvilCmd
