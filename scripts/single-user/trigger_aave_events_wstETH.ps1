# ============================================================================
# TRIGGER AAVE EVENTS (wstETH COLLATERAL)
# - UserDeposit (Aave Supply)
# - UserBorrow
# - UserRepay
# - UserWithdraw
# ============================================================================
# Requirements:
# - Hardhat mainnet fork is running (scripts/start_hardhat.ps1)
# - Foundry cast is installed
# Usage:
#   .\scripts\single-user\trigger_aave_events_wstETH.ps1
#   .\scripts\single-user\trigger_aave_events_wstETH.ps1 -SeedBorrowerWstEth 5 -SupplyWstEth 2 -BorrowUsdc 100 -RepayUsdc 50 -WithdrawWstEth 0.2

param(
    [string]$RpcUrl = "http://127.0.0.1:8545",
    [decimal]$SeedBorrowerWstEth = 5,
    [decimal]$SupplyWstEth = 2,
    [decimal]$WithdrawWstEth = 0.2,
    [decimal]$BorrowUsdc = 100,
    [decimal]$RepayUsdc = 50,
    [int]$WstEthBalanceSlot = 0
)

# ============================================================================
# MAINNET CONFIGURATION ONLY
# ============================================================================

$AAVE_POOL = "0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2"
$WSTETH    = "0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0"
$USDC      = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"
$aUSDC     = "0x98C23E9d8f34FEFb1B7BD6a91B7FF122F4e16F5c"

# Hardhat default accounts (Account #2)
$BORROWER     = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"
$BORROWER_KEY = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"

# ============================================================================
# HELPER FUNCTIONS
# ============================================================================

function Invoke-CastCall {
    param([string]$CastArgs)

    $parsed = [regex]::Match($CastArgs, '^(?<to>0x[a-fA-F0-9]{40})\s+"(?<sig>[^"]+)"\s*(?<args>.*)$')
    if (-not $parsed.Success) {
        $cmd = "cast call $CastArgs --rpc-url $RpcUrl"
        $result = Invoke-Expression $cmd 2>&1
        return ($result | Out-String).Trim()
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
        return "Error: Failed to build calldata for $sig"
    }

    try {
        $rpcPayload = @{
            jsonrpc = "2.0"
            id      = 1
            method  = "eth_call"
            params  = @(@{ to = $to; data = $calldata }, "latest")
        } | ConvertTo-Json -Compress

        $rpcResp = Invoke-RestMethod -Uri $RpcUrl -Method Post -Body $rpcPayload -ContentType "application/json"
        $rawHex = $rpcResp.result
        if ([string]::IsNullOrWhiteSpace($rawHex) -or $rawHex -eq "0x") {
            return "0"
        }

        if ($sig -match '\)\(.*\)$') {
            $decode = & cast abi-decode $sig $rawHex 2>&1
            if ($LASTEXITCODE -eq 0) {
                return ($decode | Out-String).Trim()
            }
        }

        return $rawHex
    } catch {
        return "Error: eth_call failed for $sig"
    }
}

function Invoke-CastSend {
    param([string]$CastArgs)

    $cmd = "cast send $CastArgs --rpc-url $RpcUrl"
    $result = Invoke-Expression $cmd 2>&1
    $output = ($result | Out-String).Trim()

    if ($LASTEXITCODE -ne 0) {
        $fallbackCmd = "cast send $CastArgs --rpc-url $RpcUrl --gas-limit 5000000 --legacy"
        $fallbackResult = Invoke-Expression $fallbackCmd 2>&1
        $fallbackOutput = ($fallbackResult | Out-String).Trim()
        if ($LASTEXITCODE -eq 0) {
            Write-Host "  [i] TX fallback: force --gas-limit + --legacy" -ForegroundColor DarkGray
            return $fallbackOutput
        }
        $output = $fallbackOutput
    }

    if ($LASTEXITCODE -ne 0) {
        Write-Host "  [X] Transaction FAILED!" -ForegroundColor Red
        ($output -split "`n" | Select-Object -First 3) | ForEach-Object {
            Write-Host "      $_" -ForegroundColor Red
        }
        return $null
    }

    return $output
}

function Invoke-CastRpc {
    param([string]$CastArgs)

    $cmd = "cast rpc $CastArgs --rpc-url $RpcUrl"
    $result = Invoke-Expression $cmd 2>&1
    return (($result | Out-String).Trim())
}

function Strip-CastAnnotation {
    param([string]$Value)
    return (($Value -replace '\[.*?\]', '').Trim())
}

function Parse-CastValues {
    param([string]$RawData)
    $cleaned = $RawData -replace '\[.*?\]', ''
    return (($cleaned.Trim() -split '\s+') | Where-Object { $_ -ne '' })
}

function To-WordHex {
    param([decimal]$RawValue)
    $big = [System.Numerics.BigInteger]::Parse(([math]::Floor($RawValue)).ToString("0"))
    return ("0x" + $big.ToString("x").PadLeft(64, '0'))
}

function Write-Step {
    param([string]$Step, [string]$Description)
    $stepTime = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ss.ffffffZ")
    Write-Host ""
    Write-Host "----------------------------------------" -ForegroundColor Cyan
    Write-Host "  STEP $Step : $Description" -ForegroundColor Cyan
    Write-Host "  Time: $stepTime" -ForegroundColor DarkGray
    Write-Host "----------------------------------------" -ForegroundColor Cyan
}

# ============================================================================
# PRECHECKS
# ============================================================================

Write-Host "============================================" -ForegroundColor Green
Write-Host "  TRIGGER AAVE EVENTS (wstETH)" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green
Write-Host ""

if (-not (Get-Command "cast" -ErrorAction SilentlyContinue)) {
    Write-Host "[X] Cast (Foundry) is not installed" -ForegroundColor Red
    exit 1
}

$chainIdRaw = Invoke-Expression "cast chain-id --rpc-url $RpcUrl" 2>&1
$chainId = ($chainIdRaw | Out-String).Trim()

if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($chainId)) {
    Write-Host "[X] Cannot connect to RPC: $RpcUrl" -ForegroundColor Red
    Write-Host "    Start Hardhat: .\\scripts\\start_hardhat.ps1" -ForegroundColor Yellow
    exit 1
}

if ($chainId -ne "31337" -and $chainId -ne "1") {
    Write-Host "[X] Only supports mainnet fork (chainId 31337)" -ForegroundColor Red
    exit 1
}

Write-Host "[OK] Connected (Chain ID: $chainId)" -ForegroundColor Green
Write-Host "[i] Aave Pool: $AAVE_POOL" -ForegroundColor Gray
Write-Host "[i] wstETH:    $WSTETH" -ForegroundColor Gray
Write-Host ""

# ============================================================================
# STEP 0: Seed wstETH balance for borrower
# ============================================================================

Write-Step "0/4" "Seed wstETH for borrower"

$seedWei = [decimal]$SeedBorrowerWstEth * 1e18
$seedHex = To-WordHex $seedWei
$balanceSlot = Invoke-Expression "cast index address $BORROWER $WstEthBalanceSlot" 2>&1
$balanceSlot = ($balanceSlot | Out-String).Trim()
$null = Invoke-CastRpc "hardhat_setStorageAt $WSTETH $balanceSlot $seedHex"

$borrowerWstRaw = Invoke-CastCall "$WSTETH `"balanceOf(address)(uint256)`" $BORROWER"
$borrowerWstVal = [decimal](Strip-CastAnnotation $borrowerWstRaw)
$borrowerWstDisplay = [math]::Round($borrowerWstVal / 1e18, 6)

if ($borrowerWstVal -le 0) {
    Write-Host "[X] Failed to seed wstETH. Check -WstEthBalanceSlot" -ForegroundColor Red
    exit 1
}

Write-Host "[OK] Borrower wstETH: $borrowerWstDisplay" -ForegroundColor Green

# ============================================================================
# STEP 1: Approve + Supply (UserDeposit)
# ============================================================================

Write-Step "1/4" "UserDeposit: supply wstETH"

$maxApproval = "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
$result = Invoke-CastSend "$WSTETH `"approve(address,uint256)`" $AAVE_POOL $maxApproval --private-key $BORROWER_KEY"
if ($null -eq $result) { Write-Host "[X] Approve failed" -ForegroundColor Red; exit 1 }

$supplyWstEth = [math]::Min($SupplyWstEth, [math]::Floor($borrowerWstVal / 1e18 * 0.90))
if ($supplyWstEth -le 0) {
    Write-Host "[X] Supply amount is zero" -ForegroundColor Red
    exit 1
}

$supplyWei = [math]::Floor([decimal]$supplyWstEth * 1e18)
Write-Host "[>] Supplying $supplyWstEth wstETH..." -ForegroundColor Gray
$result = Invoke-CastSend "$AAVE_POOL `"supply(address,uint256,address,uint16)`" $WSTETH $supplyWei $BORROWER 0 --private-key $BORROWER_KEY"
if ($null -eq $result) { Write-Host "[X] Supply failed" -ForegroundColor Red; exit 1 }

$null = Invoke-CastSend "$AAVE_POOL `"setUserUseReserveAsCollateral(address,bool)`" $WSTETH true --private-key $BORROWER_KEY"
Write-Host "[OK] UserDeposit emitted (supply)" -ForegroundColor Green

# ============================================================================
# STEP 2: Borrow USDC (UserBorrow)
# ============================================================================

Write-Step "2/4" "UserBorrow: borrow USDC"

$accountData = Invoke-CastCall "$AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $BORROWER"
$acctValues = Parse-CastValues $accountData
if ($acctValues.Count -lt 3) {
    Write-Host "[X] Cannot read account data" -ForegroundColor Red
    exit 1
}

$availableBorrowsBase = [decimal]$acctValues[2]
$maxBorrowUsdc = [math]::Floor($availableBorrowsBase / 100)
$poolUsdcRaw = Invoke-CastCall "$USDC `"balanceOf(address)(uint256)`" $aUSDC"
$poolUsdcNow = [decimal](Strip-CastAnnotation $poolUsdcRaw)

$desiredBorrow = [math]::Floor([decimal]$BorrowUsdc * 1e6)
$borrowCapByUser = [math]::Floor($maxBorrowUsdc * 0.8)
$borrowCapByPool = [math]::Floor($poolUsdcNow * 0.9)
$borrowAmount = [math]::Min($desiredBorrow, [math]::Min($borrowCapByUser, $borrowCapByPool))

if ($borrowAmount -lt 100000) {
    Write-Host "[X] Borrow amount too small (pool or capacity low)" -ForegroundColor Red
    exit 1
}

$borrowUsd = [math]::Round($borrowAmount / 1e6, 2)
Write-Host "[>] Borrowing $borrowUsd USDC..." -ForegroundColor Gray
$result = Invoke-CastSend "$AAVE_POOL `"borrow(address,uint256,uint256,uint16,address)`" $USDC $borrowAmount 2 0 $BORROWER --private-key $BORROWER_KEY"
if ($null -eq $result) { Write-Host "[X] Borrow failed" -ForegroundColor Red; exit 1 }
Write-Host "[OK] UserBorrow emitted" -ForegroundColor Green

# ============================================================================
# STEP 3: Repay USDC (UserRepay) + Withdraw (UserWithdraw)
# ============================================================================

Write-Step "3/4" "UserRepay + UserWithdraw"

$repayAmount = [math]::Min([math]::Floor([decimal]$RepayUsdc * 1e6), [math]::Floor($borrowAmount * 0.9))
if ($repayAmount -lt 100000) { $repayAmount = [math]::Floor($borrowAmount * 0.5) }

$result = Invoke-CastSend "$USDC `"approve(address,uint256)`" $AAVE_POOL $maxApproval --private-key $BORROWER_KEY"
if ($null -eq $result) { Write-Host "[X] Approve USDC failed" -ForegroundColor Red; exit 1 }

$repayUsd = [math]::Round($repayAmount / 1e6, 2)
Write-Host "[>] Repaying $repayUsd USDC..." -ForegroundColor Gray
$result = Invoke-CastSend "$AAVE_POOL `"repay(address,uint256,uint256,address)`" $USDC $repayAmount 2 $BORROWER --private-key $BORROWER_KEY"
if ($null -eq $result) { Write-Host "[X] Repay failed" -ForegroundColor Red; exit 1 }
Write-Host "[OK] UserRepay emitted" -ForegroundColor Green

$withdrawWei = [math]::Floor([decimal]$WithdrawWstEth * 1e18)
if ($withdrawWei -le 0) { $withdrawWei = [math]::Floor($supplyWei * 0.1) }
$withdrawEth = [math]::Round([decimal]$withdrawWei / 1e18, 6)

Write-Host "[>] Withdrawing $withdrawEth wstETH..." -ForegroundColor Gray
$result = Invoke-CastSend "$AAVE_POOL `"withdraw(address,uint256,address)`" $WSTETH $withdrawWei $BORROWER --private-key $BORROWER_KEY"
if ($null -eq $result) { Write-Host "[X] Withdraw failed" -ForegroundColor Red; exit 1 }
Write-Host "[OK] UserWithdraw emitted" -ForegroundColor Green

# ============================================================================
# DONE
# ============================================================================

Write-Step "4/4" "Done"
Write-Host "All 4 events sent: deposit, borrow, repay, withdraw." -ForegroundColor Green
Write-Host "Next: check your event pipeline detection." -ForegroundColor Yellow
