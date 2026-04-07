# ============================================================================
# START HARDHAT - Local Ethereum Fork
# ============================================================================
#
# Script nay khoi dong Hardhat node de fork Ethereum
# Muc tieu: thay the Anvil khi can HTTP + WS subscription
#
# Cach dung:
#   .\scripts\start_hardhat.ps1
#   .\scripts\start_hardhat.ps1 -Network sepolia
#   .\scripts\start_hardhat.ps1 -RpcUrl "YOUR_CUSTOM_URL"
#   .\scripts\start_hardhat.ps1 -ForkBlock 24700000
# ============================================================================

param(
    [string]$RpcUrl = "",
    [ValidateSet("mainnet", "sepolia")]
    [string]$Network = "mainnet",
    [int]$ForkBlock = 0,
    [int]$Port = 8545,
    [string]$ForkProjectPath = "fork-blockchain"
)

if (-not (Get-Command "npx" -ErrorAction SilentlyContinue)) {
    Write-Host "[X] npx khong ton tai. Hay cai Node.js truoc!" -ForegroundColor Red
    exit 1
}

if ([string]::IsNullOrEmpty($RpcUrl)) {
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
    Write-Host "Dat ETH_RPC_URL hoac SEPOLIA_RPC_URL trong env/.env" -ForegroundColor Yellow
    exit 1
}

$projectRoot = Split-Path -Parent $PSScriptRoot
$hardhatDir = Join-Path $projectRoot $ForkProjectPath
$configJs = Join-Path $hardhatDir "hardhat.config.js"
$packageJson = Join-Path $hardhatDir "package.json"

if (-not (Test-Path $hardhatDir)) {
    Write-Host "[X] Khong tim thay thu muc: $hardhatDir" -ForegroundColor Red
    exit 1
}

if (-not (Test-Path $configJs) -or -not (Test-Path $packageJson)) {
    Write-Host "[X] fork-blockchain chua duoc setup Hardhat toi thieu" -ForegroundColor Red
    Write-Host "Can co hardhat.config.js va package.json" -ForegroundColor Yellow
    exit 1
}

$urlPreview = $RpcUrl
if ($RpcUrl.Length -gt 50) {
    $urlPreview = $RpcUrl.Substring(0, 50) + "..."
}

Write-Host "============================================" -ForegroundColor Cyan
Write-Host "  HARDHAT - $networkName Fork" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host "[+] RPC URL: $urlPreview" -ForegroundColor Gray
Write-Host "[+] Local RPC: http://127.0.0.1:$Port" -ForegroundColor Green
Write-Host "[+] Local WS : ws://127.0.0.1:$Port" -ForegroundColor Green
if ($ForkBlock -gt 0) {
    Write-Host "[*] Fork tai block: $ForkBlock" -ForegroundColor Yellow
}
Write-Host "Nhan Ctrl+C de dung Hardhat" -ForegroundColor Yellow
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ""

Push-Location $hardhatDir
try {
    $hardhatArgs = @("hardhat", "node", "--fork", $RpcUrl, "--port", "$Port")
    if ($ForkBlock -gt 0) {
        $hardhatArgs += @("--fork-block-number", "$ForkBlock")
    }

    & npx @hardhatArgs
} finally {
    Pop-Location
}
