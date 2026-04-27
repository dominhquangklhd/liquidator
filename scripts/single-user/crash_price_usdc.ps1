# ============================================================================
# CRASH USDC PRICE - Trigger liquidation for USDC collateral scenario
# ============================================================================
#
# Script nay depeg gia USDC trong oracle de day HF < 1.0:
#   1. Lay USDC price source dang duoc Aave Oracle su dung
#   2. Replace code bang MockPriceFeed
#   3. Set USDC/USD gia thap hon (depeg)
#
# Yeu cau:
#   - Hardhat dang chay (scripts/start_hardhat.ps1)
#   - Da chay setup_liquidation_scenario_usdc.ps1
#
# Cach dung:
#   .\scripts\single-user\crash_price_usdc.ps1 -PriceDrop 15
#   .\scripts\single-user\crash_price_usdc.ps1 -Network mainnet -PriceDrop 20
# ============================================================================

param(
    [string]$RpcUrl = "http://127.0.0.1:8545",
    [ValidateSet("auto", "mainnet", "sepolia")]
    [string]$Network = "auto",
    [int]$PriceDrop = 15
)

# ============================================================================
# NETWORK CONFIGURATION
# ============================================================================

# Mainnet addresses
$MAINNET_CONFIG = @{
    AAVE_POOL       = "0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2"
    AAVE_ORACLE     = "0x54586bE62E3c3580375aE3723C145253060Ca0C2"
    USDC            = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"
    USDC_USD_FEED   = "0x8fFfFfd4AfB6115b954Bd326cbe7B4BA576818f6"
    NetworkName     = "Ethereum Mainnet"
}

# Sepolia addresses
$SEPOLIA_CONFIG = @{
    AAVE_POOL       = "0x6Ae43d3271ff6888e7Fc43Fd7321a503ff738951"
    AAVE_ORACLE     = "0x2da88497588bf89281816106C7259e31AF45a663"
    USDC            = "0x94a9D9AC8a22534E3FaCa9F4e7F2E2cf85d5E4C8"
    USDC_USD_FEED   = "0xA2F78ab2355fe2f984D808B5CeE7FD0A93D5270E"
    NetworkName     = "Sepolia Testnet"
}

$BORROWER     = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"
$DEPLOYER_KEY = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"

# ============================================================================
# HELPER FUNCTIONS
# ============================================================================

function Invoke-Cast {
    param([string]$CastArgs)
    $cmd = "cast $CastArgs --rpc-url $RpcUrl"
    $result = Invoke-Expression $cmd 2>&1
    return (($result | Out-String).Trim())
}

function Invoke-CastCall {
    param([string]$CallArgs)

    $parsed = [regex]::Match($CallArgs, '^(?<to>0x[a-fA-F0-9]{40})\s+"(?<sig>[^"]+)"\s*(?<args>.*)$')
    if (-not $parsed.Success) {
        return Invoke-Cast "call $CallArgs"
    }

    $to = $parsed.Groups['to'].Value
    $sig = $parsed.Groups['sig'].Value
    $argsText = $parsed.Groups['args'].Value.Trim()

    if ([string]::IsNullOrWhiteSpace($argsText)) {
        $calldataOut = & cast calldata $sig 2>&1
    } else {
        $argList = $argsText -split '\s+'
        $calldataOut = & cast calldata $sig @argList 2>&1
    }

    $calldata = ($calldataOut | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or -not ($calldata -match '^0x[0-9a-fA-F]+$')) {
        return ""
    }

    try {
        $payload = @{
            jsonrpc = "2.0"
            id      = 1
            method  = "eth_call"
            params  = @(@{ to = $to; data = $calldata }, "latest")
        } | ConvertTo-Json -Compress

        $resp = Invoke-RestMethod -Uri $RpcUrl -Method Post -Body $payload -ContentType "application/json"
        $rawHex = $resp.result

        if ([string]::IsNullOrWhiteSpace($rawHex) -or $rawHex -eq "0x") {
            return ""
        }

        if ($sig -match '\)\(.*\)$') {
            $decode = & cast abi-decode $sig $rawHex 2>&1
            if ($LASTEXITCODE -eq 0) {
                return ($decode | Out-String).Trim()
            }
        }

        return $rawHex
    } catch {
        return ""
    }
}

function Parse-HexOrDecimal {
    param([string]$Value)

    if ([string]::IsNullOrWhiteSpace($Value)) {
        return $null
    }

    $Value = ($Value -replace '\[.*?\]', '').Trim()

    try {
        if ($Value -match '^0x[a-fA-F0-9]+$') {
            return [Convert]::ToInt64($Value, 16)
        }

        if ($Value -match '^-?\d+$') {
            return [decimal]$Value
        }

        return $null
    } catch {
        return $null
    }
}

function Parse-CastValues {
    param([string]$RawData)

    $cleaned = $RawData -replace '\[.*?\]', ''
    return (($cleaned.Trim() -split '\s+') | Where-Object { $_ -ne '' })
}

function Get-HealthFactorFromAccountData {
    param([string]$AccountDataRaw)

    $values = Parse-CastValues $AccountDataRaw
    if ($values.Count -lt 6) {
        return "N/A"
    }

    $hfRaw = $values[5]
    if ($hfRaw.Length -gt 30) {
        return "Infinity"
    }

    return [math]::Round([decimal]$hfRaw / 1e18, 4)
}

function Write-Step {
    param([string]$Step, [string]$Description)

    Write-Host ""
    Write-Host "----------------------------------------" -ForegroundColor Cyan
    Write-Host "  STEP $Step : $Description" -ForegroundColor Cyan
    Write-Host "----------------------------------------" -ForegroundColor Cyan
}

# ============================================================================
# PRECHECK + NETWORK DETECTION
# ============================================================================

Write-Host "============================================" -ForegroundColor Red
Write-Host "  CRASH USDC PRICE - DEPEG TRIGGER" -ForegroundColor Red
Write-Host "============================================" -ForegroundColor Red
Write-Host ""
Write-Host "  Price Drop: $PriceDrop%" -ForegroundColor Yellow

if (-not (Get-Command "cast" -ErrorAction SilentlyContinue)) {
    Write-Host "  [X] Cast (Foundry) chua duoc cai dat!" -ForegroundColor Red
    exit 1
}

$chainIdRaw = Invoke-Expression "cast chain-id --rpc-url $RpcUrl" 2>&1
$chainId = ($chainIdRaw | Out-String).Trim()

if ([string]::IsNullOrWhiteSpace($chainId)) {
    Write-Host "  [X] Khong the ket noi RPC!" -ForegroundColor Red
    exit 1
}

if ($Network -eq "auto") {
    if ($chainId -eq "1") {
        $Network = "mainnet"
    } elseif ($chainId -eq "11155111") {
        $Network = "sepolia"
    } elseif ($chainId -eq "31337") {
        $Network = "mainnet"
        Write-Host "  [i] Detected local fork chain (31337) - using mainnet addresses" -ForegroundColor DarkGray
    } else {
        Write-Host "  [!] Unknown chain ID: $chainId - defaulting to mainnet" -ForegroundColor Yellow
        $Network = "mainnet"
    }
}

if ($Network -eq "sepolia") {
    $CONFIG = $SEPOLIA_CONFIG
} else {
    $CONFIG = $MAINNET_CONFIG
}

$AAVE_POOL     = $CONFIG.AAVE_POOL
$AAVE_ORACLE   = $CONFIG.AAVE_ORACLE
$USDC          = $CONFIG.USDC
$USDC_USD_FEED = $CONFIG.USDC_USD_FEED
$NetworkName   = $CONFIG.NetworkName

Write-Host "  [OK] Connected to $NetworkName (Chain ID: $chainId)" -ForegroundColor Green

# ============================================================================
# STEP 1: Read USDC price source + current USDC price
# ============================================================================
Write-Step "1/5" "Lay USDC source va gia hien tai"

$usdcSourceRaw = Invoke-CastCall "$AAVE_ORACLE `"getSourceOfAsset(address)(address)`" $USDC"
$USDC_PRICE_SOURCE = ($usdcSourceRaw -replace '\[.*?\]', '').Trim()
Write-Host "  [i] Aave USDC Price Source: $USDC_PRICE_SOURCE" -ForegroundColor Cyan

$usdcPriceRaw = Invoke-CastCall "$USDC_PRICE_SOURCE `"latestAnswer()(int256)`""
$usdcPrice = Parse-HexOrDecimal $usdcPriceRaw

if ($null -eq $usdcPrice -or $usdcPrice -eq 0) {
    Write-Host "  [!] latestAnswer null/0, fallback latestRoundData..." -ForegroundColor Yellow
    $roundData = Invoke-CastCall "$USDC_PRICE_SOURCE `"latestRoundData()(uint80,int256,uint256,uint256,uint80)`""
    $roundValues = Parse-CastValues $roundData
    if ($roundValues.Count -ge 2) {
        $usdcPrice = Parse-HexOrDecimal $roundValues[1]
    }
}

if ($null -eq $usdcPrice -or $usdcPrice -eq 0) {
    Write-Host "  [X] Khong doc duoc USDC price!" -ForegroundColor Red
    exit 1
}

$usdcPriceUsd = [math]::Round($usdcPrice / 1e8, 6)
$newPrice = [math]::Round($usdcPrice * (100 - $PriceDrop) / 100)
$newPriceUsd = [math]::Round($newPrice / 1e8, 6)

Write-Host "  [i] USDC/USD hien tai: $usdcPriceUsd" -ForegroundColor Green
Write-Host "  [CRASH] USDC/USD moi: $newPriceUsd" -ForegroundColor Red

# ============================================================================
# STEP 2: Check HF before crash
# ============================================================================
Write-Step "2/5" "Kiem tra HF truoc crash"

$accountDataBefore = Invoke-CastCall "$AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $BORROWER"
$hfBefore = Get-HealthFactorFromAccountData $accountDataBefore
Write-Host "  [i] HF before: $hfBefore" -ForegroundColor Gray

# ============================================================================
# STEP 3: Replace feed code with MockPriceFeed
# ============================================================================
Write-Step "3/5" "Replace feed code"

$mockJsonPath = "out\MockPriceFeed.sol\MockPriceFeed.json"
if (-not (Test-Path $mockJsonPath)) {
    Write-Host "  [!] MockPriceFeed chua compile, dang compile..." -ForegroundColor Yellow
    $null = Invoke-Expression "forge build contracts/MockPriceFeed.sol 2>&1"
}

if (-not (Test-Path $mockJsonPath)) {
    Write-Host "  [X] Khong tim thay MockPriceFeed bytecode" -ForegroundColor Red
    exit 1
}

$mockJson = Get-Content $mockJsonPath | ConvertFrom-Json
$deployedBytecode = $mockJson.deployedBytecode.object

$targets = @($USDC_PRICE_SOURCE)
if ($USDC_USD_FEED -and $USDC_USD_FEED -ne $USDC_PRICE_SOURCE) {
    $targets += $USDC_USD_FEED
}

foreach ($feed in $targets) {
    Write-Host "  [>] Replacing code at $feed ..." -ForegroundColor Gray
    Invoke-Cast "rpc hardhat_setCode $feed $deployedBytecode" | Out-Null
    Write-Host "  [OK] Code replaced" -ForegroundColor Green
}

# ============================================================================
# STEP 4: Set depeg price and mine block
# ============================================================================
Write-Step "4/5" "Set depeg price"

foreach ($feed in $targets) {
    # Ensure decimals() = 8 for parser compatibility
    Invoke-Cast "rpc hardhat_setStorageAt $feed `"0x0000000000000000000000000000000000000000000000000000000000000001`" `"0x0000000000000000000000000000000000000000000000000000000000000008`"" | Out-Null

    $out = Invoke-Cast "send $feed `"setAnswer(int256)`" $newPrice --private-key $DEPLOYER_KEY --gas-limit 5000000 --legacy"
    if ($out -match "0x[a-fA-F0-9]{64}") {
        Write-Host "  [OK] setAnswer emitted event at $feed" -ForegroundColor Green
    } else {
        $newPriceHex = "0x" + ([Convert]::ToString([long]$newPrice, 16)).PadLeft(64, '0')
        Invoke-Cast "rpc hardhat_setStorageAt $feed `"0x0000000000000000000000000000000000000000000000000000000000000000`" $newPriceHex" | Out-Null
        Write-Host "  [OK] Fallback set slot0 at $feed" -ForegroundColor Green
    }
}

Invoke-Cast "rpc evm_mine" | Out-Null
Write-Host "  [OK] Block mined" -ForegroundColor Green

# ============================================================================
# STEP 5: Verify price + HF
# ============================================================================
Write-Step "5/5" "Verify HF sau crash"

$newUsdcPriceRaw = Invoke-CastCall "$USDC_PRICE_SOURCE `"latestAnswer()(int256)`""
$newUsdcPrice = Parse-HexOrDecimal $newUsdcPriceRaw
if ($null -ne $newUsdcPrice -and $newUsdcPrice -gt 0) {
    $newUsdcPriceActual = [math]::Round($newUsdcPrice / 1e8, 6)
    Write-Host "  [i] USDC/USD after: $newUsdcPriceActual" -ForegroundColor Red
}

$accountDataAfter = Invoke-CastCall "$AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $BORROWER"
$hfAfter = Get-HealthFactorFromAccountData $accountDataAfter
Write-Host "  [i] HF after: $hfAfter" -ForegroundColor Yellow

if ($hfAfter -ne "N/A" -and $hfAfter -ne "Infinity" -and [decimal]$hfAfter -lt 1.0) {
    Write-Host "" 
    Write-Host "  !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!" -ForegroundColor Red
    Write-Host "  !  POSITION IS NOW LIQUIDATABLE   !" -ForegroundColor Red
    Write-Host "  !  Health Factor: $hfAfter" -ForegroundColor Red
    Write-Host "  !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!" -ForegroundColor Red
} else {
    Write-Host "  [!] HF van >= 1.0, thu tang -PriceDrop" -ForegroundColor Yellow
}

Write-Host ""
Write-Host "============================================" -ForegroundColor Green
Write-Host "  USDC DEPEG COMPLETE" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green
Write-Host ""
Write-Host "  [*] Ket qua:" -ForegroundColor Cyan
Write-Host "     USDC Before:  $usdcPriceUsd" -ForegroundColor Gray
Write-Host "     USDC After:   $newPriceUsd (target)" -ForegroundColor Red
Write-Host "     Drop:         $PriceDrop%" -ForegroundColor Gray
Write-Host ""
Write-Host "     HF Before:    $hfBefore" -ForegroundColor Gray
Write-Host "     HF After:     $hfAfter" -ForegroundColor $(if ($hfAfter -ne "N/A" -and $hfAfter -ne "Infinity" -and [decimal]$hfAfter -lt 1.0) { "Red" } else { "Yellow" })
Write-Host ""
Write-Host "  --> Buoc tiep theo:" -ForegroundColor Yellow
Write-Host "     cargo test --test executor_integration -- --nocapture" -ForegroundColor Yellow
Write-Host "     cargo run" -ForegroundColor Yellow
