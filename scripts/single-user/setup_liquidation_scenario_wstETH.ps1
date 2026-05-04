# ============================================================================
# SETUP LIQUIDATION SCENARIO (wstETH COLLATERAL)
# ============================================================================
#
# Script nay tao kich ban liquidation tren mainnet fork (Hardhat):
#   1. Mint wstETH cho 20 accounts bang storage manipulation
#   2. Supply wstETH lam collateral tren Aave
#   3. Borrow USDC de day HF sat 1.0
#   4. Fund USDC cho liquidator
#
# Yeu cau: Hardhat dang chay (scripts/start_hardhat.ps1)
#
# Cach dung:
#   .\scripts\single-user\setup_liquidation_scenario_wstETH.ps1
#   .\scripts\single-user\setup_liquidation_scenario_wstETH.ps1 -SeedBorrowerWstEth 250
#
# HUONG DAN CHO KICH BAN 5 (Partial Liquidation):
#   - User duoc cap von mac dinh rat lon (1000 wstETH = ~$3.5M).
#   - Chay script nay de setup, sau do chay `crash_price_wstETH.ps1 -PriceDrop 8`
#     de HF rot xuong ~0.98. Bot se chi duoc phep thanh ly 50% (Close Factor = 0.5).
#   - Sau khi thanh ly lan 1, tiep tuc crash them 15% de test thanh ly lan 2 tren cung 1 vi the.
#
# HUONG DAN CHO KICH BAN 6 (Vi liquidator khong du so du):
#   - De test bot xu ly the nao khi vi khong du USDC de tra no thay cho user:
#   - Keo xuong dong 509, sua muc cap von cua Liquidator thanh mot con so rat nho:
#     Tu: $usdcHex = "0x" + "746A528800".PadLeft(64, '0')  (500k USDC)
#     Thanh: $usdcHex = "0x" + "F4240".PadLeft(64, '0')    (1 USDC)
#   - Chay lai script nay, bot se phai tinh toan lai so luong duoc phep thanh ly (capapped by balance)
#     hoac bo qua (skip) neu so du qua nho.
# ============================================================================

param(
    [string]$RpcUrl = "http://127.0.0.1:8545",
    [int]$SeedBorrowerWstEth = 200,
    [int]$SeedOtherWstEth = 50,
    [int]$WstEthBalanceSlot = 0
)

# ============================================================================
# MAINNET CONFIGURATION ONLY
# ============================================================================

$MAINNET_CONFIG = @{
    AAVE_POOL               = "0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2"
    AAVE_ORACLE             = "0x54586bE62E3c3580375aE3723C145253060Ca0C2"
    WSTETH                  = "0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0"
    USDC                    = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"
    WBTC                    = "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599"
    aUSDC                   = "0x98C23E9d8f34FEFb1B7BD6a91B7FF122F4e16F5c"
    USDC_BALANCE_SLOT       = 9
    NetworkName             = "Ethereum Mainnet"
}

# Hardhat default accounts (Account #2 & #3)
$BORROWER        = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"  # Account #2
$BORROWER_KEY    = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"
$LIQUIDATOR      = "0xFABB0ac9d68B0B445fB7357272Ff202C5651694a"  # Account #13
$LIQUIDATOR_KEY  = "0xa267530f49f8280200edf313ee7af6b827f2a8bce2897751d06a843f644967b1"

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

function Write-Step {
    param([string]$Step, [string]$Description)
    $stepTime = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ss.ffffffZ")
    Write-Host ""
    Write-Host "----------------------------------------" -ForegroundColor Cyan
    Write-Host "  STEP $Step : $Description" -ForegroundColor Cyan
    Write-Host "  Time: $stepTime" -ForegroundColor DarkGray
    Write-Host "----------------------------------------" -ForegroundColor Cyan
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

function Write-AccountData {
    param([string]$RawData)

    $values = Parse-CastValues $RawData
    if ($values.Count -ge 6) {
        $totalCollateral = [math]::Round([decimal]$values[0] / 1e8, 2)
        $totalDebt = [math]::Round([decimal]$values[1] / 1e8, 2)
        $availableBorrow = [math]::Round([decimal]$values[2] / 1e8, 2)
        $liqThreshold = [math]::Round([decimal]$values[3] / 100, 2)
        $ltv = [math]::Round([decimal]$values[4] / 100, 2)

        $hfRaw = $values[5]
        if ($hfRaw.Length -gt 30) {
            $healthFactorDisplay = "Infinity (no debt)"
            $hfColor = "Green"
        } else {
            $healthFactor = [math]::Round([decimal]$hfRaw / 1e18, 4)
            $healthFactorDisplay = $healthFactor.ToString()
            if ($healthFactor -lt 1.0) {
                $hfColor = "Red"
            } elseif ($healthFactor -lt 1.15) {
                $hfColor = "Yellow"
            } else {
                $hfColor = "Green"
            }
        }

        Write-Host "     Total Collateral:   `$$totalCollateral" -ForegroundColor Gray
        Write-Host "     Total Debt:         `$$totalDebt" -ForegroundColor Gray
        Write-Host "     Available Borrow:   `$$availableBorrow" -ForegroundColor Gray
        Write-Host "     Liq. Threshold:     $liqThreshold%" -ForegroundColor Gray
        Write-Host "     LTV:                $ltv%" -ForegroundColor Gray
        Write-Host "     Health Factor:      $healthFactorDisplay" -ForegroundColor $hfColor
    } else {
        Write-Host "     $RawData" -ForegroundColor Gray
    }
}

function Get-HealthFactor {
    param([string]$RawData)

    $values = Parse-CastValues $RawData
    if ($values.Count -ge 6) {
        $hfRaw = $values[5]
        if ($hfRaw.Length -gt 30) {
            return 999999.0
        }
        return [math]::Round([decimal]$hfRaw / 1e18, 4)
    }
    return 999999.0
}

function To-WordHex {
    param([decimal]$RawValue)
    $big = [System.Numerics.BigInteger]::Parse(([math]::Floor($RawValue)).ToString("0"))
    return ("0x" + $big.ToString("x").PadLeft(64, '0'))
}

function Get-HardhatAccounts {
    $raw = Invoke-CastRpc "eth_accounts"
    try {
        return $raw | ConvertFrom-Json
    } catch {
        return @()
    }
}

# ============================================================================
# PREREQUISITES + NETWORK DETECTION (MAINNET FORK ONLY)
# ============================================================================

Write-Host "============================================" -ForegroundColor Green
Write-Host "  SETUP LIQUIDATION SCENARIO (wstETH COLL)" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green
Write-Host ""
$scriptStartTime = Get-Date
Write-Host "  [*] Start time: $($scriptStartTime.ToString('yyyy-MM-dd HH:mm:ss'))" -ForegroundColor Magenta
Write-Host ""

if (-not (Get-Command "cast" -ErrorAction SilentlyContinue)) {
    Write-Host "[X] Cast (Foundry) chua duoc cai dat!" -ForegroundColor Red
    exit 1
}

$chainIdRaw = Invoke-Expression "cast chain-id --rpc-url $RpcUrl" 2>&1
$chainId = ($chainIdRaw | Out-String).Trim()

if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($chainId)) {
    Write-Host "[X] Khong ket noi duoc RPC: $RpcUrl" -ForegroundColor Red
    Write-Host "    Hay chay truoc: .\scripts\start_hardhat.ps1" -ForegroundColor Yellow
    exit 1
}

if ($chainId -ne "31337" -and $chainId -ne "1") {
    Write-Host "[X] Chi ho tro mainnet fork (chainId 31337)" -ForegroundColor Red
    exit 1
}

$CONFIG = $MAINNET_CONFIG
$AAVE_POOL         = $CONFIG.AAVE_POOL
$AAVE_ORACLE       = $CONFIG.AAVE_ORACLE
$WSTETH            = $CONFIG.WSTETH
$USDC              = $CONFIG.USDC
$WBTC              = $CONFIG.WBTC
$aUSDC             = $CONFIG.aUSDC
$USDC_BALANCE_SLOT = $CONFIG.USDC_BALANCE_SLOT

Write-Host "[OK] Connected to $($CONFIG.NetworkName) fork (Chain ID: $chainId)" -ForegroundColor Green
Write-Host "[i] Aave Pool: $AAVE_POOL" -ForegroundColor Gray
Write-Host "[i] wstETH:    $WSTETH" -ForegroundColor Gray

# ============================================================================
# STEP 0: Load accounts (20) + mint wstETH via storage
# ============================================================================
Write-Step "0/8" "Mint wstETH cho 20 accounts (storage manipulation)"

$accounts = Get-HardhatAccounts
if ($accounts.Count -lt 20) {
    Write-Host "[X] Khong lay du 20 accounts tu node (eth_accounts)" -ForegroundColor Red
    exit 1
}

$seedBorrowerRaw = [decimal]$SeedBorrowerWstEth * 1e18
$seedOtherRaw = [decimal]$SeedOtherWstEth * 1e18
$seedBorrowerHex = To-WordHex $seedBorrowerRaw
$seedOtherHex = To-WordHex $seedOtherRaw

for ($i = 0; $i -lt 20; $i++) {
    $addr = $accounts[$i]
    $balanceSlot = Invoke-Expression "cast index address $addr $WstEthBalanceSlot" 2>&1
    $balanceSlot = ($balanceSlot | Out-String).Trim()

    if ($addr -eq $BORROWER) {
        $null = Invoke-CastRpc "hardhat_setStorageAt $WSTETH $balanceSlot $seedBorrowerHex"
    } else {
        $null = Invoke-CastRpc "hardhat_setStorageAt $WSTETH $balanceSlot $seedOtherHex"
    }
}

$borrowerWstRaw = Invoke-CastCall "$WSTETH `"balanceOf(address)(uint256)`" $BORROWER"
$borrowerWstVal = [decimal](Strip-CastAnnotation $borrowerWstRaw)
$borrowerWstDisplay = [math]::Round($borrowerWstVal / 1e18, 4)

if ($borrowerWstVal -le 0) {
    Write-Host "  [X] Khong set duoc wstETH cho Borrower!" -ForegroundColor Red
    Write-Host "  [!] Thu doi slot: -WstEthBalanceSlot <slot>" -ForegroundColor Yellow
    exit 1
}

Write-Host "  [OK] Borrower wstETH: $borrowerWstDisplay" -ForegroundColor Green
Write-Host "  [OK] Da seed wstETH cho 20 accounts" -ForegroundColor Green

# ============================================================================
# STEP 1: Approve + Supply wstETH collateral
# ============================================================================
Write-Step "1/8" "Approve + Supply wstETH collateral"

$maxApproval = "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
$result = Invoke-CastSend "$WSTETH `"approve(address,uint256)`" $AAVE_POOL $maxApproval --private-key $BORROWER_KEY"
if ($null -eq $result) { Write-Host "  [X] Approve wstETH that bai!" -ForegroundColor Red; exit 1 }
Write-Host "  [OK] wstETH approved" -ForegroundColor Green

$supplyWstRaw = [math]::Floor($borrowerWstVal * 0.75)
if ($supplyWstRaw -lt 1e16) {
    Write-Host "  [X] Supply amount qua nho!" -ForegroundColor Red
    exit 1
}
$supplyWstDisplay = [math]::Round([decimal]$supplyWstRaw / 1e18, 4)

Write-Host "  [>] Supplying $supplyWstDisplay wstETH..." -ForegroundColor Gray
$result = Invoke-CastSend "$AAVE_POOL `"supply(address,uint256,address,uint16)`" $WSTETH $supplyWstRaw $BORROWER 0 --private-key $BORROWER_KEY"
if ($null -eq $result) { Write-Host "  [X] Supply wstETH that bai!" -ForegroundColor Red; exit 1 }
Write-Host "  [OK] Supplied $supplyWstDisplay wstETH" -ForegroundColor Green

$result = Invoke-CastSend "$AAVE_POOL `"setUserUseReserveAsCollateral(address,bool)`" $WSTETH true --private-key $BORROWER_KEY"
if ($null -eq $result) {
    Write-Host "  [!] setCollateral failed (co the da enable)" -ForegroundColor Yellow
}
Write-Host "  [OK] wstETH enabled as collateral" -ForegroundColor Green

$accountData = Invoke-CastCall "$AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $BORROWER"
Write-Host "  [i] Account Data sau supply:" -ForegroundColor Gray
Write-AccountData $accountData

# ============================================================================
# STEP 2: Borrow USDC de day HF sat 1.0
# ============================================================================
Write-Step "2/8" "Borrow USDC"

$poolUsdcRaw = Invoke-CastCall "$USDC `"balanceOf(address)(uint256)`" $aUSDC"
$poolUsdcNow = [decimal](Strip-CastAnnotation $poolUsdcRaw)

$acctValues = Parse-CastValues $accountData
if ($acctValues.Count -ge 3) {
    $availableBorrowsBase = [decimal]$acctValues[2]
    $maxBorrowUsdc = [math]::Floor($availableBorrowsBase / 100)

    $borrowFromCapacity = [math]::Floor($maxBorrowUsdc * 0.99)
    $borrowFromPool = [math]::Floor($poolUsdcNow * 0.90)
    $borrowAmount = [math]::Min($borrowFromCapacity, $borrowFromPool)

    $borrowAmountUSD = [math]::Round([decimal]$borrowAmount / 1e6, 2)
    Write-Host "  [>] Borrowing $borrowAmountUSD USDC..." -ForegroundColor Gray
} else {
    $borrowAmount = [math]::Min(1000000000, [math]::Floor($poolUsdcNow * 0.90))
    $borrowAmountUSD = [math]::Round([decimal]$borrowAmount / 1e6, 2)
    Write-Host "  [!] Fallback: vay $borrowAmountUSD USDC" -ForegroundColor Yellow
}

$borrowAmountStr = [math]::Floor($borrowAmount).ToString("0")
$result = Invoke-CastSend "$AAVE_POOL `"borrow(address,uint256,uint256,uint16,address)`" $USDC $borrowAmountStr 2 0 $BORROWER --private-key $BORROWER_KEY"
if ($null -eq $result) {
    Write-Host "  [!] Borrow $borrowAmountUSD that bai, thu 50% pool..." -ForegroundColor Yellow
    $borrowAmount = [math]::Floor($poolUsdcNow * 0.50)
    $borrowAmountStr = [math]::Floor($borrowAmount).ToString("0")
    $borrowAmountUSD = [math]::Round([decimal]$borrowAmount / 1e6, 2)
    $result = Invoke-CastSend "$AAVE_POOL `"borrow(address,uint256,uint256,uint16,address)`" $USDC $borrowAmountStr 2 0 $BORROWER --private-key $BORROWER_KEY"
    if ($null -eq $result) { Write-Host "  [X] Borrow van that bai!" -ForegroundColor Red; exit 1 }
}
Write-Host "  [OK] Borrowed $borrowAmountUSD USDC" -ForegroundColor Green

$accountData = Invoke-CastCall "$AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $BORROWER"
Write-Host "  [i] Account Data sau borrow:" -ForegroundColor Gray
Write-AccountData $accountData

# ============================================================================
# STEP 3: Vay them de day HF sat 1.0
# ============================================================================
Write-Step "3/8" "Vay them USDC de day HF sat 1.0"

$totalBorrowedUSD = $borrowAmountUSD
for ($i = 1; $i -le 5; $i++) {
    $poolUsdcCheck = Invoke-CastCall "$USDC `"balanceOf(address)(uint256)`" $aUSDC"
    $poolUsdcCheckVal = [decimal](Strip-CastAnnotation $poolUsdcCheck)

    $acctValues2 = Parse-CastValues $accountData
    if ($acctValues2.Count -lt 6) { break }

    $availLeft = [decimal]$acctValues2[2]
    $availLeftUsdc = [math]::Floor($availLeft / 100)

    $poolCap = [math]::Floor($poolUsdcCheckVal * 0.90)
    $extraBorrow = [math]::Min([math]::Floor($availLeftUsdc * 0.99), $poolCap)

    if ($extraBorrow -lt 100000) {
        Write-Host "  [i] Khong con du de vay them, dung." -ForegroundColor Gray
        break
    }

    $extraBorrowUSD = [math]::Round([decimal]$extraBorrow / 1e6, 2)
    Write-Host "  [>] Vay them #$i : $extraBorrowUSD USDC ..." -ForegroundColor Gray

    $extraBorrowStr = [math]::Floor($extraBorrow).ToString("0")
    $result = Invoke-CastSend "$AAVE_POOL `"borrow(address,uint256,uint256,uint16,address)`" $USDC $extraBorrowStr 2 0 $BORROWER --private-key $BORROWER_KEY"
    if ($null -eq $result) {
        Write-Host "  [!] Vay them that bai, dung." -ForegroundColor Yellow
        break
    }

    $totalBorrowedUSD += $extraBorrowUSD

    $accountData = Invoke-CastCall "$AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $BORROWER"
    $currentHF = Get-HealthFactor $accountData
    Write-Host "  [i] HF hien tai: $currentHF" -ForegroundColor Yellow

    if ($currentHF -lt 1.05) {
        Write-Host "  [OK] HF da sat 1.0!" -ForegroundColor Green
        break
    }
}

$borrowAmountUSD = $totalBorrowedUSD
Write-Host ""
Write-Host "  [i] Tong no: ~`$$borrowAmountUSD USDC" -ForegroundColor Gray
Write-Host "  [i] Account Data cuoi:" -ForegroundColor Gray
Write-AccountData $accountData

$finalHF = Get-HealthFactor $accountData

# ============================================================================
# STEP 4: Rut bot collateral de day HF xuong sat 1.0
# ============================================================================
if ($finalHF -gt 1.10) {
    Write-Step "4/8" "Rut bot wstETH collateral de day HF xuong ~1.03"

    $targetHF = 1.03
    for ($w = 1; $w -le 8; $w++) {
        $wValues = Parse-CastValues $accountData
        if ($wValues.Count -lt 6) { break }

        $curCollateral8 = [decimal]$wValues[0]
        $curDebt8 = [decimal]$wValues[1]
        $curLiqThreshold = [decimal]$wValues[3]

        if ($curDebt8 -lt 1e6) {
            Write-Host "  [!] Debt qua nho, khong can rut collateral." -ForegroundColor Yellow
            break
        }

        $curHF = Get-HealthFactor $accountData
        if ($curHF -le 1.08) {
            Write-Host "  [OK] HF = $curHF da gan 1.0!" -ForegroundColor Green
            break
        }

        $liqRatio = $curLiqThreshold / 10000
        $targetCollateral8 = $targetHF * $curDebt8 / $liqRatio
        $withdrawAmount8 = $curCollateral8 - $targetCollateral8

        if ($withdrawAmount8 -lt 1e6) {
            Write-Host "  [i] Khong can rut them." -ForegroundColor Gray
            break
        }

        $wstPriceNow = Invoke-CastCall "$AAVE_ORACLE `"getAssetPrice(address)(uint256)`" $WSTETH"
        $wstPriceNowVal = [decimal](Strip-CastAnnotation $wstPriceNow)
        $withdrawWstWei = [math]::Floor($withdrawAmount8 / $wstPriceNowVal * 1e18)
        $withdrawWstWei = [math]::Floor($withdrawWstWei * 0.95)

        if ($withdrawWstWei -lt 1e14) {
            Write-Host "  [i] Withdraw amount qua nho, dung." -ForegroundColor Gray
            break
        }

        $withdrawWstDisplay = [math]::Round([decimal]$withdrawWstWei / 1e18, 6)
        $withdrawUSD = [math]::Round($withdrawAmount8 / 1e8 * 0.95, 2)
        Write-Host "  [>] Rut #$w : $withdrawWstDisplay wstETH (~`$$withdrawUSD) ..." -ForegroundColor Gray

        $withdrawStr = [math]::Floor($withdrawWstWei).ToString("0")
        $result = Invoke-CastSend "$AAVE_POOL `"withdraw(address,uint256,address)`" $WSTETH $withdrawStr $BORROWER --private-key $BORROWER_KEY"
        if ($null -eq $result) {
            Write-Host "  [!] Withdraw that bai (HF qua sat 1.0), dung." -ForegroundColor Yellow
            break
        }

        $accountData = Invoke-CastCall "$AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $BORROWER"
        $curHF = Get-HealthFactor $accountData
        Write-Host "  [i] HF sau rut: $curHF" -ForegroundColor Yellow
    }

    $accountData = Invoke-CastCall "$AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $BORROWER"
    Write-Host "  [i] Account Data sau khi rut collateral:" -ForegroundColor Gray
    Write-AccountData $accountData
    $finalHF = Get-HealthFactor $accountData
}

# ============================================================================
# STEP 5: Fund liquidator USDC
# ============================================================================
Write-Step "5/8" "Fund liquidator USDC"

Write-Host "  [>] Setting USDC balance (slot $USDC_BALANCE_SLOT)..." -ForegroundColor Gray
$balanceSlot = Invoke-Expression "cast index address $LIQUIDATOR $USDC_BALANCE_SLOT" 2>&1
$balanceSlot = ($balanceSlot | Out-String).Trim()
$usdcHex = "0x" + "746A528800".PadLeft(64, '0')
$null = Invoke-CastRpc "hardhat_setStorageAt $USDC $balanceSlot $usdcHex"

$liquidatorUSDC_raw = Invoke-CastCall "$USDC `"balanceOf(address)(uint256)`" $LIQUIDATOR"
$liquidatorUSDC_val = [math]::Round([decimal](Strip-CastAnnotation $liquidatorUSDC_raw) / 1e6, 2)

if ($liquidatorUSDC_val -gt 0) {
    Write-Host "  [OK] Liquidator USDC: $liquidatorUSDC_val" -ForegroundColor Green
} else {
    Write-Host "  [X] Storage slot $USDC_BALANCE_SLOT incorrect!" -ForegroundColor Red
    exit 1
}

$result = Invoke-CastSend "$USDC `"approve(address,uint256)`" $AAVE_POOL $maxApproval --private-key $LIQUIDATOR_KEY"
if ($null -ne $result) {
    Write-Host "  [OK] Liquidator approved USDC" -ForegroundColor Green
}

# ============================================================================
# STEP 6: Snapshot
# ============================================================================
Write-Step "6/8" "Tao snapshot"

$snapshotId = Invoke-CastRpc "evm_snapshot"
if ([string]::IsNullOrWhiteSpace($snapshotId) -or $snapshotId -match '^Error') {
    Write-Host "  [!] Khong tao duoc snapshot tren node hien tai" -ForegroundColor Yellow
} else {
    Write-Host "  [*] Snapshot ID: $snapshotId" -ForegroundColor Green
    Write-Host "  [i] Rollback: cast rpc evm_revert $snapshotId --rpc-url $RpcUrl" -ForegroundColor Gray
}

# ============================================================================
# STEP 7: Summary
# ============================================================================
Write-Step "7/8" "Summary"

$neededDrop = [math]::Round((1 - 1.0 / $finalHF) * 100, 0)
if ($neededDrop -lt 1) { $neededDrop = 1 }

Write-Host ""
Write-Host "============================================" -ForegroundColor Green
Write-Host "  WSTETH-COLLATERAL SCENARIO READY" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green
Write-Host ""
Write-Host "  [i] Borrower:   $BORROWER" -ForegroundColor Gray
Write-Host "  [i] Collateral: ~$supplyWstDisplay wstETH" -ForegroundColor Gray
Write-Host "  [i] Debt:       ~`$$borrowAmountUSD USDC" -ForegroundColor Gray
Write-Host "  [i] HF:         $finalHF" -ForegroundColor Gray
Write-Host ""
Write-Host "  [i] Liquidator: $LIQUIDATOR" -ForegroundColor Gray
Write-Host "  [i] USDC:       $liquidatorUSDC_val" -ForegroundColor Gray
Write-Host ""
Write-Host "  --> Buoc tiep theo:" -ForegroundColor Yellow
Write-Host "     1. .\scripts\single-user\crash_price_wstETH.ps1 -PriceDrop $neededDrop" -ForegroundColor Yellow
Write-Host "     2. cargo test executor -- --nocapture" -ForegroundColor Yellow
Write-Host "     3. cargo run" -ForegroundColor Yellow