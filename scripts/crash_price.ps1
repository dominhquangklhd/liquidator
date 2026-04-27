# ============================================================================
# UNIFIED CRASH PRICE WRAPPER (single-user + multi-users)
# ============================================================================
# Cach dung:
#   .\scripts\crash_price.ps1 -Mode single
#   .\scripts\crash_price.ps1 -Mode single -PriceDrop 30
#   .\scripts\crash_price.ps1 -Mode multi
#   .\scripts\crash_price.ps1 -Mode multi -PriceDrop 25 -PriceDropBuffer 8
#   .\scripts\crash_price.ps1 -Mode multi -SeedAaveEvent
# ============================================================================

param(
    [ValidateSet("single", "multi")]
    [string]$Mode = "single",
    [string]$RpcUrl = "http://127.0.0.1:8545",
    [ValidateSet("auto", "mainnet", "sepolia")]
    [string]$Network = "auto",
    [int]$PriceDrop = 0,
    [int]$PriceDropBuffer = 8,
    [switch]$SeedAaveEvent
)

$scriptPath = if ($Mode -eq "multi") {
    Join-Path $PSScriptRoot "multi-users\crash_price_multi.ps1"
} else {
    Join-Path $PSScriptRoot "single-user\crash_price.ps1"
}

if (-not (Test-Path $scriptPath)) {
    Write-Host "[X] Khong tim thay script: $scriptPath" -ForegroundColor Red
    exit 1
}

$forwardParams = @{
    RpcUrl  = $RpcUrl
    Network = $Network
}

if ($Mode -eq "single") {
    if ($PriceDrop -gt 0) {
        $forwardParams.PriceDrop = $PriceDrop
    }
} else {
    if ($PriceDrop -ge 0) {
        $forwardParams.PriceDrop = $PriceDrop
    }
    $forwardParams.PriceDropBuffer = $PriceDropBuffer
}

if ($SeedAaveEvent.IsPresent) {
    $forwardParams.SeedAaveEvent = $true
}

Write-Host "[i] Mode: $Mode" -ForegroundColor Cyan
Write-Host "[i] Running: $scriptPath" -ForegroundColor DarkGray

& $scriptPath @forwardParams
exit $LASTEXITCODE
