# ============================================================================
# SETUP LIQUIDATION SCENARIO
# ============================================================================
#
# Script nay tao kich ban liquidation tren Hardhat fork:
#   1. Kiem tra pool USDC liquidity truoc
#   2. Tinh toan supply WETH vua du de vay gan het USDC trong pool
#   3. Day HF sat 1.0 de chi can crash gia nhe la liquidatable
#
# Yeu cau: Hardhat dang chay (scripts/start_hardhat.ps1)
#
# Cach dung:
#   .\scripts\single-user\setup_liquidation_scenario.ps1              # Auto-detect network
#   .\scripts\single-user\setup_liquidation_scenario.ps1 -Network mainnet
#   .\scripts\single-user\setup_liquidation_scenario.ps1 -Network sepolia
# ============================================================================

param(
    [string]$RpcUrl = "http://127.0.0.1:8545",
    [ValidateSet("auto", "mainnet", "sepolia")]
    [string]$Network = "auto"
)

$script:RpcClientFlavor = "unknown"

# ============================================================================
# NETWORK CONFIGURATION
# ============================================================================

# Mainnet addresses (Chain ID: 1)
$MAINNET_CONFIG = @{
    AAVE_POOL               = "0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2"
    AAVE_ORACLE             = "0x54586bE62E3c3580375aE3723C145253060Ca0C2"
    POOL_ADDRESSES_PROVIDER = "0x2f39d218133AFaB8F2B819B1066c7E434Ad94E9e"
    ACL_MANAGER             = "0xc2aaCf6553D20d1e9571216f576571920c0FBB3d"
    WETH                    = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"
    USDC                    = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"
    WBTC                    = "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599"
    aWETH                   = "0x4d5F47FA6A74757f35C14fD3a6Ef8E3C9BC514E8"
    aUSDC                   = "0x98C23E9d8f34FEFb1B7BD6a91B7FF122F4e16F5c"
    ETH_USD_FEED            = "0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419"
    USDC_BALANCE_SLOT       = 9    # Mainnet USDC balanceOf mapping at slot 9
    NetworkName             = "Ethereum Mainnet"
}

# Sepolia addresses (Chain ID: 11155111)
$SEPOLIA_CONFIG = @{
    AAVE_POOL               = "0x6Ae43d3271ff6888e7Fc43Fd7321a503ff738951"
    AAVE_ORACLE             = "0x2da88497588bf89281816106C7259e31AF45a663"
    POOL_ADDRESSES_PROVIDER = "0x012bAC54348C0E635dCAc9D5FB99f06F24136C9A"
    ACL_MANAGER             = "0x7F2bE3b178deeFF716CD6Ff03Ef79A1dFf360ddD"
    WETH                    = "0xC558DBdd856501FCd9aaF1E62eae57A9F0629a3c"
    USDC                    = "0x94a9D9AC8a22534E3FaCa9F4e7F2E2cf85d5E4C8"
    WBTC                    = "0x29f2D40B0605204364af54EC677bD022dA425d03"
    aWETH                   = "0x5b071b590a59395fE4025A0Ccc1FcC931AAc1830"
    aUSDC                   = "0x16da4541aD1807f4443d92D26044C1147406EB80"
    ETH_USD_FEED            = "0x694AA1769357215DE4FAC081bf1f309aDC325306"
    USDC_BALANCE_SLOT       = 0    # Sepolia USDC balanceOf mapping at slot 0
    NetworkName             = "Sepolia Testnet"
}

# Hardhat default accounts (tu dong co 10000 ETH)
# Dung Account #2 & #3 de tranh position cu tren Sepolia (Account #0/#1 da co Aave state)
$BORROWER        = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"  # Account #2
$BORROWER_KEY    = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"
$LIQUIDATOR      = "0x90F79bf6EB2c4f870365E785982E1f101E93b906"  # Account #3
$LIQUIDATOR_KEY  = "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6"

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
        # Hardhat compatibility: bypass eth_estimateGas by forcing gas limit.
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
        # Extract short error message
        $errLines = $output -split "`n" | Select-Object -First 3
        foreach ($line in $errLines) {
            Write-Host "      $line" -ForegroundColor Red
        }
        return $null
    }
    return $output
}

function Invoke-CastRpc {
    param([string]$CastArgs)

    $cmd = "cast rpc $CastArgs --rpc-url $RpcUrl"
    $result = Invoke-Expression $cmd 2>&1
    $output = ($result | Out-String).Trim()

    if ($LASTEXITCODE -eq 0) {
        return $output
    }

    return $output
}

function Write-Step {
    param([string]$Step, [string]$Description)
    Write-Host ""
    Write-Host "----------------------------------------" -ForegroundColor Cyan
    Write-Host "  STEP $Step : $Description" -ForegroundColor Cyan
    Write-Host "----------------------------------------" -ForegroundColor Cyan
}

function Strip-CastAnnotation {
    param([string]$Value)
    $stripped = ($Value -replace '\[.*?\]', '').Trim()
    return $stripped
}

function Parse-CastValues {
    param([string]$RawData)
    $cleaned = $RawData -replace '\[.*?\]', ''
    $values = ($cleaned.Trim() -split '\s+') | Where-Object { $_ -ne '' }
    return $values
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
        if ($hfRaw.Length -gt 30) { return 999999.0 }
        return [math]::Round([decimal]$hfRaw / 1e18, 4)
    }
    return 999999.0
}

# ============================================================================
# KIEM TRA PREREQUISITES
# ============================================================================

Write-Host "============================================" -ForegroundColor Green
Write-Host "  SETUP LIQUIDATION SCENARIO" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green

if (-not (Get-Command "cast" -ErrorAction SilentlyContinue)) {
    Write-Host "[X] Cast (Foundry) chua duoc cai dat!" -ForegroundColor Red
    exit 1
}

$chainIdRaw = Invoke-Expression "cast chain-id --rpc-url $RpcUrl" 2>&1
$chainId = ($chainIdRaw | Out-String).Trim()

if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($chainId)) {
    Write-Host "[X] Khong ket noi duoc RPC: $RpcUrl" -ForegroundColor Red
    Write-Host "    Hay chay truoc: .\\scripts\\start_hardhat.ps1" -ForegroundColor Yellow
    exit 1
}

# Detect RPC client flavor once to avoid noisy unsupported-method fallbacks.
$clientVersionRaw = Invoke-Expression "cast rpc web3_clientVersion --rpc-url $RpcUrl" 2>&1
$clientVersion = ($clientVersionRaw | Out-String).Trim().ToLowerInvariant()
if ($LASTEXITCODE -eq 0) {
    if ($clientVersion -match "hardhat") {
        $script:RpcClientFlavor = "hardhat"
    }
}

if ($script:RpcClientFlavor -eq "hardhat") {
    Write-Host "[i] RPC client: Hardhat" -ForegroundColor DarkGray
}

if ($Network -eq "auto") {
    if ($chainId -eq "1") {
        $Network = "mainnet"
    } elseif ($chainId -eq "11155111") {
        $Network = "sepolia"
    } elseif ($chainId -eq "31337") {
        # Hardhat local fork uses 31337 by default.
        $Network = "mainnet"
        Write-Host "[i] Detected local fork chain (31337) - using mainnet addresses" -ForegroundColor DarkGray
    } else {
        Write-Host "[!] Unknown chain ID: $chainId - defaulting to mainnet config" -ForegroundColor Yellow
        $Network = "mainnet"
    }
}

if ($Network -eq "sepolia") {
    $CONFIG = $SEPOLIA_CONFIG
} else {
    $CONFIG = $MAINNET_CONFIG
}

$AAVE_POOL              = $CONFIG.AAVE_POOL
$AAVE_ORACLE            = $CONFIG.AAVE_ORACLE
$POOL_ADDRESSES_PROVIDER = $CONFIG.POOL_ADDRESSES_PROVIDER
$ACL_MANAGER            = $CONFIG.ACL_MANAGER
$WETH                   = $CONFIG.WETH
$USDC                   = $CONFIG.USDC
$WBTC                   = $CONFIG.WBTC
$aWETH                  = $CONFIG.aWETH
$aUSDC                  = $CONFIG.aUSDC
$ETH_USD_FEED           = $CONFIG.ETH_USD_FEED
$USDC_BALANCE_SLOT      = $CONFIG.USDC_BALANCE_SLOT
$NetworkName            = $CONFIG.NetworkName

Write-Host "[OK] Connected to $NetworkName (Chain ID: $chainId)" -ForegroundColor Green
Write-Host "[i] Aave Pool: $AAVE_POOL" -ForegroundColor Gray

# ============================================================================
# STEP 0: Kiem tra pool USDC liquidity & tinh toan supply amount
# ============================================================================
Write-Step "0/8" "Kiem tra USDC liquidity & tinh toan"

# Kiem tra USDC available trong pool
$poolUsdcRaw = Invoke-CastCall "$USDC `"balanceOf(address)(uint256)`" $aUSDC"
$poolUsdcClean = Strip-CastAnnotation $poolUsdcRaw
$poolUsdcAmount = [decimal]$poolUsdcClean   # raw USDC (6-decimal)
$poolUsdcUSD = [math]::Round($poolUsdcAmount / 1e6, 2)
Write-Host "  [i] USDC kha dung trong Pool: $poolUsdcUSD USDC" -ForegroundColor Gray

if ($poolUsdcAmount -lt 1000000) {  # < 1 USDC
    Write-Host "  [X] Pool khong co du USDC liquidity!" -ForegroundColor Red
    exit 1
}

# Lay ETH price tu Aave Oracle
$ethPriceRaw = Invoke-CastCall "$AAVE_ORACLE `"getAssetPrice(address)(uint256)`" $WETH"
$ethPriceClean = Strip-CastAnnotation $ethPriceRaw
$ethPriceBase = [decimal]$ethPriceClean  # 8-decimal USD
$ethPriceUSD = [math]::Round($ethPriceBase / 1e8, 2)
Write-Host "  [i] ETH price (Aave Oracle): `$$ethPriceUSD" -ForegroundColor Gray

# ── Tinh WETH can supply ──
# Muc tieu: supply 50 WETH => co du collateral vay ~$80-95k USDC
# Gioi han hop ly trong 1-50 WETH (Hardhat account co 10,000 ETH)
$ltvRatio = 0.80
$maxSupplyETH = 50   # Max 50 WETH supply

# Borrow target = min(90% pool, collateral capacity)
$maxCollateralUSD = $maxSupplyETH * $ethPriceUSD
$maxBorrowFromCollateral = $maxCollateralUSD * $ltvRatio
$borrowTargetUsdc6 = [math]::Min([math]::Floor($poolUsdcAmount * 0.90), $maxBorrowFromCollateral * 1e6)
$borrowTargetUSD = [math]::Round($borrowTargetUsdc6 / 1e6, 2)
Write-Host "  [i] Borrow target: $borrowTargetUSD USDC" -ForegroundColor Gray

# WETH can supply = borrow / LTV / ethPrice (voi +5% buffer)
$neededCollateralUSD = $borrowTargetUSD / $ltvRatio
$neededWethETH = $neededCollateralUSD / $ethPriceUSD * 1.05  # +5% buffer

# Cap: min 1 WETH, max 50 WETH
if ($neededWethETH -lt 1) { $neededWethETH = 1 }
if ($neededWethETH -gt $maxSupplyETH) { $neededWethETH = $maxSupplyETH }
$supplyWethETH = [math]::Round($neededWethETH, 4)

# Convert to wei dung [decimal] (tranh overflow Int64)
$neededWethWei = [decimal]([math]::Ceiling($supplyWethETH * 1e18))

Write-Host "  [i] Supply: ~$supplyWethETH WETH (du de vay $borrowTargetUSD USDC)" -ForegroundColor Gray

# ============================================================================
# STEP 1: Wrap ETH -> WETH cho borrower
# ============================================================================
Write-Step "1/8" "Wrap ETH -> WETH cho Borrower"

$wrapAmount = [math]::Ceiling($supplyWethETH)  # Round up to whole ETH
$result = Invoke-CastSend "$WETH `"deposit()`" --value ${wrapAmount}ether --private-key $BORROWER_KEY"
if ($null -eq $result) { Write-Host "  [X] Wrap ETH that bai!" -ForegroundColor Red; exit 1 }
Write-Host "  [OK] Wrapped $wrapAmount ETH -> WETH" -ForegroundColor Green

$wethBalance = Invoke-CastCall "$WETH `"balanceOf(address)(uint256)`" $BORROWER"
Write-Host "  [i] Borrower WETH balance: $wethBalance" -ForegroundColor Gray

# ============================================================================
# STEP 2: Approve WETH cho Aave Pool
# ============================================================================
Write-Step "2/8" "Approve WETH cho Aave Pool"

$maxApproval = "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
$result = Invoke-CastSend "$WETH `"approve(address,uint256)`" $AAVE_POOL $maxApproval --private-key $BORROWER_KEY"
if ($null -eq $result) { Write-Host "  [X] Approve that bai!" -ForegroundColor Red; exit 1 }
Write-Host "  [OK] WETH approved" -ForegroundColor Green

# ============================================================================
# STEP 3: Supply WETH vao Aave (collateral)
# ============================================================================
Write-Step "3/8" "Supply WETH vao Aave lam Collateral"

$supplyAmount = $neededWethWei.ToString("0")  # decimal -> string (no scientific notation)
Write-Host "  [>] Supplying $supplyWethETH WETH..." -ForegroundColor Gray
$result = Invoke-CastSend "$AAVE_POOL `"supply(address,uint256,address,uint16)`" $WETH $supplyAmount $BORROWER 0 --private-key $BORROWER_KEY"
if ($null -eq $result) { Write-Host "  [X] Supply that bai!" -ForegroundColor Red; exit 1 }
Write-Host "  [OK] Supplied $supplyWethETH WETH" -ForegroundColor Green

Write-Host "  [>] Enabling WETH as collateral..." -ForegroundColor Gray
$result = Invoke-CastSend "$AAVE_POOL `"setUserUseReserveAsCollateral(address,bool)`" $WETH true --private-key $BORROWER_KEY"
if ($null -eq $result) { Write-Host "  [!] setCollateral failed (co the da enable)" -ForegroundColor Yellow }
Write-Host "  [OK] WETH enabled as collateral" -ForegroundColor Green

$accountData = Invoke-CastCall "$AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $BORROWER"
Write-Host "  [i] Account Data sau khi supply:" -ForegroundColor Gray
Write-AccountData $accountData

# ============================================================================
# STEP 4: Borrow USDC (gioi han boi pool liquidity)
# ============================================================================
Write-Step "4/8" "Borrow USDC (gioi han boi pool liquidity)"

# Re-check pool liquidity (co the da thay doi)
$poolUsdcNowRaw = Invoke-CastCall "$USDC `"balanceOf(address)(uint256)`" $aUSDC"
$poolUsdcNow = [decimal](Strip-CastAnnotation $poolUsdcNowRaw)

$acctValues = Parse-CastValues $accountData
if ($acctValues.Count -ge 3) {
    $availableBorrowsBase = [decimal]$acctValues[2]  # 8-decimal USD
    $maxBorrowUsdc = [math]::Floor($availableBorrowsBase / 100)  # -> 6-decimal USDC
    
    # Borrow = min(99% capacity, 90% pool liquidity)
    $borrowFromCapacity = [math]::Floor($maxBorrowUsdc * 0.99)
    $borrowFromPool = [math]::Floor($poolUsdcNow * 0.90)
    $borrowAmount = [math]::Min($borrowFromCapacity, $borrowFromPool)
    
    $borrowAmountUSD = [math]::Round([decimal]$borrowAmount / 1e6, 2)
    $maxBorrowUSD = [math]::Round($maxBorrowUsdc / 1e6, 0)
    Write-Host "  [i] Max borrow capacity: `$$maxBorrowUSD" -ForegroundColor Gray
    Write-Host "  [i] Pool USDC available:  $([math]::Round($poolUsdcNow / 1e6, 2))" -ForegroundColor Gray
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
# STEP 4b: Vay them de day HF sat 1.0
# ============================================================================
Write-Step "4b/8" "Vay them USDC de day HF sat 1.0"

$totalBorrowedUSD = $borrowAmountUSD
for ($i = 1; $i -le 5; $i++) {
    # Re-check pool liquidity
    $poolUsdcCheck = Invoke-CastCall "$USDC `"balanceOf(address)(uint256)`" $aUSDC"
    $poolUsdcCheckVal = [decimal](Strip-CastAnnotation $poolUsdcCheck)

    $acctValues2 = Parse-CastValues $accountData
    if ($acctValues2.Count -lt 6) { break }

    $availLeft = [decimal]$acctValues2[2]  # availableBorrowsBase (8-decimal)
    $availLeftUsdc = [math]::Floor($availLeft / 100)

    # Cap boi pool liquidity
    $poolCap = [math]::Floor($poolUsdcCheckVal * 0.90)
    $extraBorrow = [math]::Min([math]::Floor($availLeftUsdc * 0.99), $poolCap)

    if ($extraBorrow -lt 100000) {  # < 0.10 USDC
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
# STEP 4c: Rut bot collateral de day HF xuong sat 1.0
# ============================================================================
if ($finalHF -gt 1.10) {
    Write-Step "4c/8" "Rut bot collateral de day HF xuong ~1.03"

    # HF = (totalCollateral * liqThreshold) / totalDebt
    # targetCollateral = targetHF * totalDebt / liqThreshold
    # withdrawAmount = currentCollateral - targetCollateral (in base 8-decimal USD)
    # Convert to WETH: withdrawWeth = withdrawUSD / ethPrice

    $targetHF = 1.03
    for ($w = 1; $w -le 8; $w++) {
        $wValues = Parse-CastValues $accountData
        if ($wValues.Count -lt 6) { break }

        $curCollateral8 = [decimal]$wValues[0]   # 8-decimal USD
        $curDebt8 = [decimal]$wValues[1]          # 8-decimal USD
        $curLiqThreshold = [decimal]$wValues[3]   # basis points (e.g., 8445 = 84.45%)

        if ($curDebt8 -lt 1e6) {
            Write-Host "  [!] Debt qua nho, khong can rut collateral." -ForegroundColor Yellow
            break
        }

        $curHF = Get-HealthFactor $accountData
        if ($curHF -le 1.08) {
            Write-Host "  [OK] HF = $curHF da gan 1.0!" -ForegroundColor Green
            break
        }

        # targetCollateral = targetHF * debt / (liqThreshold / 10000)
        $liqRatio = $curLiqThreshold / 10000
        $targetCollateral8 = $targetHF * $curDebt8 / $liqRatio
        $withdrawAmount8 = $curCollateral8 - $targetCollateral8

        if ($withdrawAmount8 -lt 1e6) {  # < $0.01
            Write-Host "  [i] Khong can rut them." -ForegroundColor Gray
            break
        }

        # Convert USD to WETH (18-decimal)
        # withdrawWeth = withdrawUSD_8decimal / ethPrice_8decimal * 1e18
        $ethPriceNow = Invoke-CastCall "$AAVE_ORACLE `"getAssetPrice(address)(uint256)`" $WETH"
        $ethPriceNowVal = [decimal](Strip-CastAnnotation $ethPriceNow)
        $withdrawWethWei = [math]::Floor($withdrawAmount8 / $ethPriceNowVal * 1e18)

        # Safety: withdraw 95% of calculated amount to avoid revert
        $withdrawWethWei = [math]::Floor($withdrawWethWei * 0.95)

        if ($withdrawWethWei -lt 1e14) {  # < 0.0001 WETH
            Write-Host "  [i] Withdraw amount qua nho, dung." -ForegroundColor Gray
            break
        }

        $withdrawWethETH = [math]::Round([decimal]$withdrawWethWei / 1e18, 6)
        $withdrawUSD = [math]::Round($withdrawAmount8 / 1e8 * 0.95, 2)
        Write-Host "  [>] Rut #$w : $withdrawWethETH WETH (~`$$withdrawUSD) ..." -ForegroundColor Gray

        $withdrawStr = [math]::Floor($withdrawWethWei).ToString("0")
        $result = Invoke-CastSend "$AAVE_POOL `"withdraw(address,uint256,address)`" $WETH $withdrawStr $BORROWER --private-key $BORROWER_KEY"
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

if ($finalHF -gt 1.20) {
    $neededDrop = [math]::Round((1 - 1.0 / $finalHF) * 100, 0)
    Write-Host ""
    Write-Host "  [!] HF = $finalHF van cao." -ForegroundColor Yellow
    Write-Host "  [!] Can crash gia ~${neededDrop}% de HF < 1.0" -ForegroundColor Yellow
} else {
    $neededDrop = [math]::Round((1 - 1.0 / $finalHF) * 100, 0)
    Write-Host ""
    Write-Host "  [OK] HF = $finalHF - chi can crash ~${neededDrop}% la liquidatable!" -ForegroundColor Green
}

# ============================================================================
# STEP 5: Chuan bi Liquidator wallet
# ============================================================================
Write-Step "5/8" "Setup Liquidator Wallet"

# Set USDC balance qua storage manipulation (slot khac nhau tuy network)
Write-Host "  [>] Setting USDC balance (storage slot $USDC_BALANCE_SLOT)..." -ForegroundColor Gray
$balanceSlot = Invoke-Expression "cast index address $LIQUIDATOR $USDC_BALANCE_SLOT" 2>&1
$balanceSlot = ($balanceSlot | Out-String).Trim()
# 500,000 USDC = 500000 * 10^6 = 500000000000 = 0x746A528800
$usdcHex = "0x" + "746A528800".PadLeft(64, '0')
$null = Invoke-CastRpc "hardhat_setStorageAt $USDC $balanceSlot $usdcHex"

# Verify
$liquidatorUSDC_raw = Invoke-CastCall "$USDC `"balanceOf(address)(uint256)`" $LIQUIDATOR"
$liquidatorUSDC_clean = Strip-CastAnnotation $liquidatorUSDC_raw
$liquidatorUSDC_val = [math]::Round([decimal]$liquidatorUSDC_clean / 1e6, 2)

if ($liquidatorUSDC_val -gt 0) {
    Write-Host "  [OK] Liquidator USDC: $liquidatorUSDC_val" -ForegroundColor Green
} else {
    Write-Host "  [X] Storage slot $USDC_BALANCE_SLOT incorrect!" -ForegroundColor Red
    Write-Host "  [>] Fallback: impersonate aUSDC de transfer..." -ForegroundColor Yellow
    
    # Impersonate aUSDC contract (holds pool's USDC) and transfer
    Invoke-CastRpc "hardhat_impersonateAccount $aUSDC"
    # Give aUSDC some ETH for gas
    Invoke-CastRpc "hardhat_setBalance $aUSDC 0x56BC75E2D63100000"
    $transferAmt = [math]::Min($poolUsdcAmount * 0.5, 500000000000).ToString("0")  # min(50% pool, 500k USDC)
    $result = Invoke-CastSend "$USDC `"transfer(address,uint256)`" $LIQUIDATOR $transferAmt --from $aUSDC"
    Invoke-CastRpc "hardhat_stopImpersonatingAccount $aUSDC"
    
    $liquidatorUSDC_raw = Invoke-CastCall "$USDC `"balanceOf(address)(uint256)`" $LIQUIDATOR"
    $liquidatorUSDC_clean = Strip-CastAnnotation $liquidatorUSDC_raw
    $liquidatorUSDC_val = [math]::Round([decimal]$liquidatorUSDC_clean / 1e6, 2)
    if ($liquidatorUSDC_val -gt 0) {
        Write-Host "  [OK] Liquidator USDC (impersonate): $liquidatorUSDC_val" -ForegroundColor Green
    } else {
        Write-Host "  [X] Khong set duoc USDC cho Liquidator!" -ForegroundColor Red
    }
}

# Approve USDC cho Aave Pool
$result = Invoke-CastSend "$USDC `"approve(address,uint256)`" $AAVE_POOL $maxApproval --private-key $LIQUIDATOR_KEY"
if ($null -ne $result) {
    Write-Host "  [OK] Liquidator approved USDC" -ForegroundColor Green
}

# ============================================================================
# STEP 6: Kiem tra trang thai cuoi cung
# ============================================================================
Write-Step "6/8" "Kiem tra trang thai cuoi cung"

$ethPriceChainlink = Invoke-CastCall "$ETH_USD_FEED `"latestAnswer()(int256)`""
$ethPriceChainlinkClean = Strip-CastAnnotation $ethPriceChainlink
$ethPriceChainlinkUSD = [math]::Round([decimal]$ethPriceChainlinkClean / 1e8, 2)
Write-Host "  [`$] ETH/USD (Chainlink): `$$ethPriceChainlinkUSD" -ForegroundColor Green

$accountData = Invoke-CastCall "$AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $BORROWER"
Write-Host "  [i] Borrower Account:" -ForegroundColor Gray
Write-AccountData $accountData

Write-Host ""
Write-Host "  [OK] Scenario san sang!" -ForegroundColor Green

# ============================================================================
# STEP 7: Tao snapshot
# ============================================================================
Write-Step "7/8" "Tao Snapshot"

$snapshotId = Invoke-CastRpc "evm_snapshot"
if ([string]::IsNullOrWhiteSpace($snapshotId) -or $snapshotId -match '^Error') {
    Write-Host "  [!] Khong tao duoc snapshot tren node hien tai" -ForegroundColor Yellow
} else {
    Write-Host "  [*] Snapshot ID: $snapshotId" -ForegroundColor Green
    Write-Host "  [i] Rollback: cast rpc evm_revert $snapshotId --rpc-url $RpcUrl" -ForegroundColor Gray
}

# ============================================================================
# SUMMARY
# ============================================================================
Write-Host ""
Write-Host "============================================" -ForegroundColor Green
Write-Host "  SCENARIO SETUP COMPLETE" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green
Write-Host ""
Write-Host "  [i] Tom tat:" -ForegroundColor Cyan
Write-Host "     Borrower:   $BORROWER" -ForegroundColor Gray
Write-Host "     Collateral: $supplyWethETH WETH" -ForegroundColor Gray
Write-Host "     Debt:       ~`$$borrowAmountUSD USDC" -ForegroundColor Gray
Write-Host "     ETH/USD:    `$$ethPriceChainlinkUSD" -ForegroundColor Gray
Write-Host "     HF:         $finalHF" -ForegroundColor Gray
Write-Host ""
Write-Host "     Liquidator: $LIQUIDATOR" -ForegroundColor Gray
Write-Host "     USDC:       $liquidatorUSDC_val" -ForegroundColor Gray
Write-Host ""
Write-Host "  --> Buoc tiep theo:" -ForegroundColor Yellow
if ($finalHF -gt 1.20) {
    $suggestedDrop = [math]::Min([int]$neededDrop + 5, 95)
    Write-Host "     1. .\scripts\crash_price.ps1 -PriceDrop $suggestedDrop  - Crash ${suggestedDrop}%" -ForegroundColor Yellow
} else {
    Write-Host "     1. .\scripts\crash_price.ps1              - Crash gia ETH" -ForegroundColor Yellow
}
Write-Host "     2. cargo test executor                   - Integration test" -ForegroundColor Yellow
Write-Host "     3. cargo run                             - Chay bot" -ForegroundColor Yellow
