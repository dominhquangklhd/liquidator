# ============================================================================
# SETUP MULTI-BORROWER LIQUIDATION SCENARIO (wstETH COLLATERAL)
# ============================================================================
#
# Script nay tao nhieu borrower cung 1 luc de test "nhieu vi the dong thoi bi thanh ly":
#   1. Mint wstETH cho 10 accounts bang storage manipulation (khong can wrap ETH)
#   2. Moi borrower: Supply wstETH -> Borrow USDC -> Day HF sat 1.0
#   3. Setup Liquidator wallet
#   4. Tao snapshot de rollback
#
# Yeu cau: Hardhat dang chay (scripts/start_hardhat.ps1)
#
# Cach dung:
#   .\scripts\multi-users\setup_multi_liquidation_wstETH.ps1                        # 10 borrowers, mainnet fork
#   .\scripts\multi-users\setup_multi_liquidation_wstETH.ps1 -SupplyWstEthPerUser 5 # 5 wstETH moi user
#   .\scripts\multi-users\setup_multi_liquidation_wstETH.ps1 -BorrowRatio 0.95      # Vay 95% capacity
# ============================================================================

param(
    [string]$RpcUrl = "http://127.0.0.1:8545",
    [decimal]$SupplyWstEthPerUser = 0,  # 0 = tu dong tinh (chia pool USDC cho so luong user)
    [decimal]$BorrowRatio = 0.92,       # 92% borrowing capacity per user (de HF ~ 1.04)
    [int]$TargetHF_Pct = 103,           # Target HF * 100 (103 = HF ~ 1.03)
    [int]$WstEthBalanceSlot = 0         # Storage slot cua balanceOf trong wstETH contract
)

$script:RpcClientFlavor = "unknown"

# ============================================================================
# NETWORK CONFIGURATION
# ============================================================================

$MAINNET_CONFIG = @{
    AAVE_POOL               = "0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2"
    AAVE_ORACLE             = "0x54586bE62E3c3580375aE3723C145253060Ca0C2"
    POOL_ADDRESSES_PROVIDER = "0x2f39d218133AFaB8F2B819B1066c7E434Ad94E9e"
    ACL_MANAGER             = "0xc2aaCf6553D20d1e9571216f576571920c0FBB3d"
    WSTETH                  = "0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0"
    USDC                    = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"
    aUSDC                   = "0x98C23E9d8f34FEFb1B7BD6a91B7FF122F4e16F5c"
    USDC_BALANCE_SLOT       = 9
    NetworkName             = "Ethereum Mainnet"
}


# ============================================================================
# HARDHAT ACCOUNTS - 10 BORROWERS (Account #2 den #11)
# Liquidator = Account #13, Deployer = Account #0
# ============================================================================

$BORROWERS = @(
    @{ Address = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"; Key = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"; Label = "Borrower-02" },
    @{ Address = "0x90F79bf6EB2c4f870365E785982E1f101E93b906"; Key = "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6"; Label = "Borrower-03" },
    @{ Address = "0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65"; Key = "0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a"; Label = "Borrower-04" },
    @{ Address = "0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc"; Key = "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba"; Label = "Borrower-05" },
    @{ Address = "0x976EA74026E726554dB657fA54763abd0C3a0aa9"; Key = "0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e"; Label = "Borrower-06" },
    @{ Address = "0x14dC79964da2C08b23698B3D3cc7Ca32193d9955"; Key = "0x4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356"; Label = "Borrower-07" },
    @{ Address = "0x23618e81E3f5cdF7f54C3d65f7FBc0aBf5B21E8f"; Key = "0xdbda1821b80551c9d65939329250298aa3472ba22feea921c0cf5d620ea67b97"; Label = "Borrower-08" },
    @{ Address = "0xa0Ee7A142d267C1f36714E4a8F75612F20a79720"; Key = "0x2a871d0798f97d79848a013d4936a73bf4cc922c825d33c1cf7073dff6d409c6"; Label = "Borrower-09" },
    @{ Address = "0xBcd4042DE499D14e55001CcbB24a551F3b954096"; Key = "0xf214f2b2cd398c806f84e317254e0f0b801d0643303237d97a22a48e01628897"; Label = "Borrower-10" },
    @{ Address = "0x71bE63f3384f5fb98995898A86B02Fb2426c5788"; Key = "0x701b615bbdfb9de65240bc28bd21bbc0d996645a3dd57e7b12bc2bdf6f192c82"; Label = "Borrower-11" }
)

$LIQUIDATOR      = "0xFABB0ac9d68B0B445fB7357272Ff202C5651694a"  # Account #13
$LIQUIDATOR_KEY  = "0xa267530f49f8280200edf313ee7af6b827f2a8bce2897751d06a843f644967b1"
$DEPLOYER        = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"  # Account #0
$DEPLOYER_KEY    = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"

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
        return "Error: Failed to build calldata"
    }

    try {
        $rpcPayload = @{
            jsonrpc = "2.0"; id = 1; method = "eth_call"
            params  = @(@{ to = $to; data = $calldata }, "latest")
        } | ConvertTo-Json -Compress

        $rpcResp = Invoke-RestMethod -Uri $RpcUrl -Method Post -Body $rpcPayload -ContentType "application/json"
        $rawHex = $rpcResp.result
        if ([string]::IsNullOrWhiteSpace($rawHex) -or $rawHex -eq "0x") { return "0" }

        if ($sig -match '\)\(.*\)$') {
            $decode = & cast abi-decode $sig $rawHex 2>&1
            if ($LASTEXITCODE -eq 0) { return ($decode | Out-String).Trim() }
        }
        return $rawHex
    } catch {
        return "Error: eth_call failed"
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
            Write-Host "    [i] TX fallback: --gas-limit + --legacy" -ForegroundColor DarkGray
            return $fallbackOutput
        }
        $errLines = $fallbackOutput -split "`n" | Select-Object -First 3
        foreach ($line in $errLines) { Write-Host "    $line" -ForegroundColor Red }
        return $null
    }
    return $output
}

function Invoke-CastRpc {
    param([string]$CastArgs)
    $cmd = "cast rpc $CastArgs --rpc-url $RpcUrl"
    $result = Invoke-Expression $cmd 2>&1
    return ($result | Out-String).Trim()
}

function Strip-CastAnnotation {
    param([string]$Value)
    return ($Value -replace '\[.*?\]', '').Trim()
}

function Parse-CastValues {
    param([string]$RawData)
    $cleaned = $RawData -replace '\[.*?\]', ''
    return ($cleaned.Trim() -split '\s+') | Where-Object { $_ -ne '' }
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

function Write-AccountData {
    param([string]$RawData, [string]$Prefix = "    ")
    $values = Parse-CastValues $RawData
    if ($values.Count -ge 6) {
        $col  = [math]::Round([decimal]$values[0] / 1e8, 2)
        $debt = [math]::Round([decimal]$values[1] / 1e8, 2)
        $hfRaw = $values[5]
        if ($hfRaw.Length -gt 30) {
            $hf = "Infinity"; $hfColor = "Green"
        } else {
            $hf = [math]::Round([decimal]$hfRaw / 1e18, 4)
            $hfColor = if ($hf -lt 1.0) { "Red" } elseif ($hf -lt 1.15) { "Yellow" } else { "Green" }
        }
        Write-Host "${Prefix}Collateral: `$$col  Debt: `$$debt  HF: $hf" -ForegroundColor $hfColor
    } else {
        Write-Host "${Prefix}$RawData" -ForegroundColor Gray
    }
}

function Write-Step {
    param([string]$Step, [string]$Description)
    Write-Host ""
    Write-Host "----------------------------------------" -ForegroundColor Cyan
    Write-Host "  STEP $Step : $Description" -ForegroundColor Cyan
    Write-Host "----------------------------------------" -ForegroundColor Cyan
}

function Write-BorrowerHeader {
    param([int]$Index, [hashtable]$Borrower, [int]$Total)
    Write-Host ""
    Write-Host "  [$($Index+1)/$Total] >>> $($Borrower.Label) : $($Borrower.Address)" -ForegroundColor Magenta
}

function To-WordHex {
    param([decimal]$RawValue)
    $big = [System.Numerics.BigInteger]::Parse(([math]::Floor($RawValue)).ToString("0"))
    return ("0x" + $big.ToString("x").PadLeft(64, '0'))
}

# ============================================================================
# PREREQUISITES
# ============================================================================

Write-Host ""
Write-Host "============================================" -ForegroundColor Green
Write-Host "  SETUP MULTI-BORROWER LIQUIDATION SCENARIO" -ForegroundColor Green
Write-Host "  Collateral: wstETH (storage manipulation)" -ForegroundColor Green
Write-Host "  $($BORROWERS.Count) borrowers se duoc tao" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green

if (-not (Get-Command "cast" -ErrorAction SilentlyContinue)) {
    Write-Host "[X] Cast (Foundry) chua duoc cai dat!" -ForegroundColor Red
    exit 1
}

$chainIdRaw = Invoke-Expression "cast chain-id --rpc-url $RpcUrl" 2>&1
$chainId = ($chainIdRaw | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($chainId)) {
    Write-Host "[X] Khong ket noi RPC: $RpcUrl" -ForegroundColor Red
    Write-Host "    Hay chay truoc: .\scripts\start_hardhat.ps1" -ForegroundColor Yellow
    exit 1
}

$clientVersionRaw = Invoke-Expression "cast rpc web3_clientVersion --rpc-url $RpcUrl" 2>&1
$clientVersion = ($clientVersionRaw | Out-String).Trim().ToLowerInvariant()
if ($LASTEXITCODE -eq 0 -and $clientVersion -match "hardhat") {
    $script:RpcClientFlavor = "hardhat"
    Write-Host "[i] RPC client: Hardhat" -ForegroundColor DarkGray
}

$CONFIG = $MAINNET_CONFIG

$AAVE_POOL         = $CONFIG.AAVE_POOL
$AAVE_ORACLE       = $CONFIG.AAVE_ORACLE
$WSTETH            = $CONFIG.WSTETH
$USDC              = $CONFIG.USDC
$aUSDC             = $CONFIG.aUSDC
$USDC_BALANCE_SLOT = $CONFIG.USDC_BALANCE_SLOT
$maxApproval       = "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"

Write-Host "[OK] Connected: $($CONFIG.NetworkName) (Chain ID: $chainId)" -ForegroundColor Green
Write-Host "[i] Aave Pool : $AAVE_POOL" -ForegroundColor Gray
Write-Host "[i] wstETH    : $WSTETH" -ForegroundColor Gray

# ============================================================================
# STEP 0: Kiem tra USDC liquidity & wstETH price
# ============================================================================
Write-Step "0" "Kiem tra USDC liquidity & wstETH price"

$poolUsdcRaw = Invoke-CastCall "$USDC `"balanceOf(address)(uint256)`" $aUSDC"
$poolUsdcAmount = [decimal](Strip-CastAnnotation $poolUsdcRaw)
$poolUsdcUSD = [math]::Round($poolUsdcAmount / 1e6, 2)
Write-Host "  [i] USDC trong Pool: `$$poolUsdcUSD" -ForegroundColor Gray

if ($poolUsdcAmount -lt 1000000) {
    Write-Host "  [X] Pool khong co du USDC!" -ForegroundColor Red; exit 1
}

$wstEthPriceRaw = Invoke-CastCall "$AAVE_ORACLE `"getAssetPrice(address)(uint256)`" $WSTETH"
$wstEthPriceBase = [decimal](Strip-CastAnnotation $wstEthPriceRaw)
$wstEthPriceUSD = [math]::Round($wstEthPriceBase / 1e8, 2)
Write-Host "  [i] wstETH price: `$$wstEthPriceUSD" -ForegroundColor Gray

if ($wstEthPriceBase -le 0) {
    Write-Host "  [X] Khong lay duoc gia wstETH tu Aave Oracle!" -ForegroundColor Red; exit 1
}

# Tinh wstETH can supply cho moi user
$numBorrowers = $BORROWERS.Count
if ($SupplyWstEthPerUser -le 0) {
    $targetBorrowPerUserUSD = [math]::Min(($poolUsdcUSD * 0.85) / $numBorrowers, 50000)
    # LTV cua wstETH tren Aave mainnet ~ 0.80
    $ltvRatio = 0.80
    $neededCollateralUSD = $targetBorrowPerUserUSD / ($ltvRatio * $BorrowRatio)
    $SupplyWstEthPerUser = [math]::Min([math]::Round($neededCollateralUSD / $wstEthPriceUSD + 0.5, 2), 30)
    $SupplyWstEthPerUser = [math]::Max($SupplyWstEthPerUser, 1.0)  # Toi thieu 1 wstETH
}

$supplyWeiPerUser = [decimal]([math]::Ceiling($SupplyWstEthPerUser * 1e18))

Write-Host "  [i] wstETH supply moi user : $SupplyWstEthPerUser wstETH" -ForegroundColor Gray
Write-Host "  [i] So borrowers           : $numBorrowers" -ForegroundColor Gray
Write-Host "  [i] Tong wstETH can        : $([math]::Round($SupplyWstEthPerUser * $numBorrowers, 2)) wstETH" -ForegroundColor Gray

# ============================================================================
# STEP 0.5: Mint wstETH cho tat ca borrowers bang storage manipulation
# ============================================================================
Write-Step "0.5" "Mint wstETH cho $numBorrowers borrowers (storage manipulation)"

$mintAmountRaw = [decimal]$SupplyWstEthPerUser * 2 * 1e18  # Mint gap doi de du supply + du phong
$mintHex = To-WordHex $mintAmountRaw

$mintSuccess = 0
foreach ($borrower in $BORROWERS) {
    $addr = $borrower.Address
    $balanceSlot = Invoke-Expression "cast index address $addr $WstEthBalanceSlot" 2>&1
    $balanceSlot = ($balanceSlot | Out-String).Trim()

    $null = Invoke-CastRpc "hardhat_setStorageAt $WSTETH $balanceSlot $mintHex"

    # Verify
    $balRaw = Invoke-CastCall "$WSTETH `"balanceOf(address)(uint256)`" $addr"
    $balVal = [decimal](Strip-CastAnnotation $balRaw)
    if ($balVal -gt 0) {
        $mintSuccess++
    } else {
        Write-Host "  [!] Khong mint duoc wstETH cho $($borrower.Label)!" -ForegroundColor Yellow
    }
}

if ($mintSuccess -eq 0) {
    Write-Host "  [X] Tat ca mint deu that bai! Thu doi -WstEthBalanceSlot <slot>" -ForegroundColor Red
    exit 1
}
Write-Host "  [OK] Mint wstETH thanh cong cho $mintSuccess / $numBorrowers accounts" -ForegroundColor Green

# ============================================================================
# STEP 1-4: Tao position cho tung borrower
# ============================================================================
Write-Step "1-4" "Tao Aave position cho $numBorrowers Borrowers (wstETH collateral)"

$successCount = 0
$failedBorrowers = @()
$borrowerResults = @()

for ($i = 0; $i -lt $BORROWERS.Count; $i++) {
    $borrower = $BORROWERS[$i]
    $addr = $borrower.Address
    $key  = $borrower.Key
    $label = $borrower.Label

    Write-BorrowerHeader -Index $i -Borrower $borrower -Total $BORROWERS.Count

    # -- Kiem tra balance wstETH
    $wstBalRaw = Invoke-CastCall "$WSTETH `"balanceOf(address)(uint256)`" $addr"
    $wstBalVal = [decimal](Strip-CastAnnotation $wstBalRaw)
    $wstBalDisplay = [math]::Round($wstBalVal / 1e18, 4)
    Write-Host "    [i] wstETH balance: $wstBalDisplay" -ForegroundColor DarkGray

    if ($wstBalVal -le 0) {
        Write-Host "    [X] Khong co wstETH! Bo qua borrower nay." -ForegroundColor Red
        $failedBorrowers += $label
        continue
    }

    # -- Fund ETH cho gas
    $ethBalRaw = Invoke-Expression "cast balance $addr --rpc-url $RpcUrl" 2>&1
    $ethBalStr = ($ethBalRaw | Out-String).Trim()
    $ethBal = try { [decimal]$ethBalStr / 1e18 } catch { 0 }
    if ($ethBal -lt 1) {
        # Dung BigInteger de tranh overflow: [long] chi chua toi ~9.2e18, con 1000 ETH = 1e21
        $ethWei = [System.Numerics.BigInteger]::Parse("1000000000000000000000")  # 1000 ETH in wei
        $ethHex = "0x" + $ethWei.ToString("x").TrimStart('0')
        $null = Invoke-CastRpc "hardhat_setBalance $addr $ethHex"
        Write-Host "    [i] ETH balance set: 1000 ETH (cho gas)" -ForegroundColor DarkGray
    }

    # -- 1. Approve wstETH cho Aave
    $r = Invoke-CastSend "$WSTETH `"approve(address,uint256)`" $AAVE_POOL $maxApproval --private-key $key"
    if ($null -eq $r) {
        Write-Host "    [X] Approve wstETH that bai! Bo qua." -ForegroundColor Red
        $failedBorrowers += $label; continue
    }
    Write-Host "    [OK] wstETH approved" -ForegroundColor Green

    # -- 2. Supply wstETH lam collateral (supply 75% balance de giu du phong)
    $supplyAmount = [math]::Floor($wstBalVal * 0.75)
    if ($supplyAmount -lt 1e15) {
        Write-Host "    [X] Supply amount qua nho! Bo qua." -ForegroundColor Red
        $failedBorrowers += $label; continue
    }
    $supplyDisplay = [math]::Round([decimal]$supplyAmount / 1e18, 4)
    Write-Host "    [>] Supply $supplyDisplay wstETH..." -ForegroundColor Gray

    $supplyStr = [math]::Floor($supplyAmount).ToString("0")
    $r = Invoke-CastSend "$AAVE_POOL `"supply(address,uint256,address,uint16)`" $WSTETH $supplyStr $addr 0 --private-key $key"
    if ($null -eq $r) {
        Write-Host "    [X] Supply that bai! Bo qua." -ForegroundColor Red
        $failedBorrowers += $label; continue
    }
    Write-Host "    [OK] Supplied $supplyDisplay wstETH" -ForegroundColor Green

    # Enable collateral
    $null = Invoke-CastSend "$AAVE_POOL `"setUserUseReserveAsCollateral(address,bool)`" $WSTETH true --private-key $key"

    # -- 3. Borrow USDC
    $accountData = Invoke-CastCall "$AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $addr"
    $acctValues = Parse-CastValues $accountData
    if ($acctValues.Count -lt 3) {
        Write-Host "    [X] Khong doc duoc account data!" -ForegroundColor Red
        $failedBorrowers += $label; continue
    }

    $availBorrowBase = [decimal]$acctValues[2]  # 8-decimal USD
    $maxBorrowUsdc   = [math]::Floor($availBorrowBase / 100)  # 6-decimal USDC

    # Re-check pool liquidity
    $poolUsdcNowRaw = Invoke-CastCall "$USDC `"balanceOf(address)(uint256)`" $aUSDC"
    $poolUsdcNow = [decimal](Strip-CastAnnotation $poolUsdcNowRaw)

    $borrowFromCapacity = [math]::Floor($maxBorrowUsdc * $BorrowRatio)
    $borrowFromPool     = [math]::Floor($poolUsdcNow * 0.88)
    $borrowAmount       = [math]::Min($borrowFromCapacity, $borrowFromPool)

    if ($borrowAmount -lt 1000000) {  # < 1 USDC
        Write-Host "    [!] Pool USDC can kiet! ($([math]::Round($poolUsdcNow/1e6,2)) con lai)" -ForegroundColor Yellow
        Write-Host "    [!] Bo qua cac borrower con lai." -ForegroundColor Yellow
        $failedBorrowers += $label
        break
    }

    $borrowAmountUSD = [math]::Round($borrowAmount / 1e6, 2)
    Write-Host "    [>] Borrowing `$$borrowAmountUSD USDC ($([math]::Round($BorrowRatio*100))% capacity)..." -ForegroundColor Gray

    $borrowStr = [math]::Floor($borrowAmount).ToString("0")
    $r = Invoke-CastSend "$AAVE_POOL `"borrow(address,uint256,uint256,uint16,address)`" $USDC $borrowStr 2 0 $addr --private-key $key"
    if ($null -eq $r) {
        Write-Host "    [!] Borrow that bai, thu 50%..." -ForegroundColor Yellow
        $borrowAmount = [math]::Floor($maxBorrowUsdc * 0.50)
        $borrowStr = $borrowAmount.ToString("0")
        $r = Invoke-CastSend "$AAVE_POOL `"borrow(address,uint256,uint256,uint16,address)`" $USDC $borrowStr 2 0 $addr --private-key $key"
        if ($null -eq $r) {
            Write-Host "    [X] Borrow van that bai!" -ForegroundColor Red
            $failedBorrowers += $label; continue
        }
    }
    Write-Host "    [OK] Borrowed `$$([math]::Round($borrowAmount/1e6,2)) USDC" -ForegroundColor Green

    # -- 4. Rut bot collateral de day HF xuong ~($TargetHF_Pct/100)
    $targetHFVal = $TargetHF_Pct / 100.0
    $accountData = Invoke-CastCall "$AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $addr"
    $currentHF = Get-HealthFactor $accountData

    if ($currentHF -gt ($targetHFVal + 0.08)) {
        Write-Host "    [>] HF = $currentHF, thu rut bot collateral de dat ~$targetHFVal..." -ForegroundColor Gray

        $wValues = Parse-CastValues $accountData
        if ($wValues.Count -ge 6) {
            $curCollateral8  = [decimal]$wValues[0]
            $curDebt8        = [decimal]$wValues[1]
            $curLiqThreshold = [decimal]$wValues[3]

            $liqRatio = $curLiqThreshold / 10000
            $targetCollateral8 = $targetHFVal * $curDebt8 / $liqRatio
            $withdrawAmount8   = $curCollateral8 - $targetCollateral8

            if ($withdrawAmount8 -gt 1e6) {
                $wstPriceNow = [decimal](Strip-CastAnnotation (Invoke-CastCall "$AAVE_ORACLE `"getAssetPrice(address)(uint256)`" $WSTETH"))
                $withdrawWeiRaw = [math]::Floor($withdrawAmount8 / $wstPriceNow * 1e18 * 0.95)

                if ($withdrawWeiRaw -gt 1e14) {
                    $withdrawStr = [math]::Floor($withdrawWeiRaw).ToString("0")
                    $r = Invoke-CastSend "$AAVE_POOL `"withdraw(address,uint256,address)`" $WSTETH $withdrawStr $addr --private-key $key"
                    if ($null -ne $r) {
                        $accountData = Invoke-CastCall "$AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $addr"
                        $currentHF = Get-HealthFactor $accountData
                        Write-Host "    [i] HF sau rut collateral: $currentHF" -ForegroundColor Yellow
                    }
                }
            }
        }
    }

    # Final status
    Write-AccountData -RawData $accountData -Prefix "    "
    $finalHF = Get-HealthFactor $accountData

    $borrowerResults += [PSCustomObject]@{
        Label   = $label
        Address = $addr
        HF      = $finalHF
        Status  = if ($finalHF -lt 1.5) { "OK" } else { "HIGH_HF" }
    }
    $successCount++
    Write-Host "    [OK] $label setup thanh cong! HF = $finalHF" -ForegroundColor Green
}

# ============================================================================
# STEP 5: Setup Liquidator wallet
# ============================================================================
Write-Step "5" "Setup Liquidator Wallet"

Write-Host "  [>] Setting USDC balance cho Liquidator..." -ForegroundColor Gray
$balanceSlot = Invoke-Expression "cast index address $LIQUIDATOR $USDC_BALANCE_SLOT" 2>&1
$balanceSlot = ($balanceSlot | Out-String).Trim()
# 2,000,000 USDC = 2_000_000 * 1e6 = 2_000_000_000_000 raw = 0x1D1A94A2000
# LƯU Ý: hex cũ 0x1DCD650000 = 128,000 USDC (SAI)
$usdcHex = "0x" + "1D1A94A2000".PadLeft(64, '0')
$null = Invoke-CastRpc "hardhat_setStorageAt $USDC $balanceSlot $usdcHex"

$liquidatorUSDC_raw = Invoke-CastCall "$USDC `"balanceOf(address)(uint256)`" $LIQUIDATOR"
$liquidatorUSDC_val = [math]::Round([decimal](Strip-CastAnnotation $liquidatorUSDC_raw) / 1e6, 2)

if ($liquidatorUSDC_val -gt 0) {
    Write-Host "  [OK] Liquidator USDC: $liquidatorUSDC_val" -ForegroundColor Green
} else {
    Write-Host "  [!] Storage slot khong dung, fallback impersonate aUSDC..." -ForegroundColor Yellow
    $aUSDC_addr = $CONFIG.aUSDC
    Invoke-CastRpc "hardhat_impersonateAccount $aUSDC_addr" | Out-Null
    Invoke-CastRpc "hardhat_setBalance $aUSDC_addr 0x56BC75E2D63100000" | Out-Null
    $transferAmt = [math]::Min([math]::Floor($poolUsdcAmount * 0.3), 2000000000000).ToString("0")
    $null = Invoke-CastSend "$USDC `"transfer(address,uint256)`" $LIQUIDATOR $transferAmt --from $aUSDC_addr"
    Invoke-CastRpc "hardhat_stopImpersonatingAccount $aUSDC_addr" | Out-Null
    $liquidatorUSDC_val = [math]::Round([decimal](Strip-CastAnnotation (Invoke-CastCall "$USDC `"balanceOf(address)(uint256)`" $LIQUIDATOR")) / 1e6, 2)
    Write-Host "  $(if ($liquidatorUSDC_val -gt 0) { '[OK]' } else { '[X]' }) Liquidator USDC (impersonate): $liquidatorUSDC_val" -ForegroundColor $(if ($liquidatorUSDC_val -gt 0) { 'Green' } else { 'Red' })
}

$null = Invoke-CastSend "$USDC `"approve(address,uint256)`" $AAVE_POOL $maxApproval --private-key $LIQUIDATOR_KEY"
Write-Host "  [OK] Liquidator approved USDC for Aave" -ForegroundColor Green

# ============================================================================
# STEP 6: Snapshot
# ============================================================================
Write-Step "6" "Tao EVM Snapshot"

$snapshotId = Invoke-CastRpc "evm_snapshot"
if ([string]::IsNullOrWhiteSpace($snapshotId) -or $snapshotId -match '^Error') {
    Write-Host "  [!] Khong tao duoc snapshot" -ForegroundColor Yellow
} else {
    Write-Host "  [*] Snapshot ID: $snapshotId" -ForegroundColor Green
    Write-Host "  [i] Rollback: cast rpc evm_revert $snapshotId --rpc-url $RpcUrl" -ForegroundColor Gray
}

# ============================================================================
# SUMMARY
# ============================================================================
Write-Host ""
Write-Host "============================================" -ForegroundColor Green
Write-Host "  MULTI-BORROWER (wstETH) SETUP COMPLETE" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green
Write-Host ""
Write-Host "  [i] Ket qua:" -ForegroundColor Cyan
Write-Host "     Thanh cong : $successCount / $($BORROWERS.Count) borrowers" -ForegroundColor $(if ($successCount -eq $BORROWERS.Count) { 'Green' } else { 'Yellow' })
if ($failedBorrowers.Count -gt 0) {
    Write-Host "     That bai   : $($failedBorrowers -join ', ')" -ForegroundColor Red
}
Write-Host ""

Write-Host "  [i] Trang thai cac Borrower:" -ForegroundColor Cyan
foreach ($r in $borrowerResults) {
    $color = if ($r.HF -lt 1.1) { "Yellow" } elseif ($r.HF -lt 1.5) { "Green" } else { "Gray" }
    $note  = if ($r.HF -gt 1.3) { " (can crash manh hon)" } else { "" }
    Write-Host "     $($r.Label) : $($r.Address.Substring(0,10))...  HF = $($r.HF)$note" -ForegroundColor $color
}

Write-Host ""
Write-Host "     wstETH/USD : `$$wstEthPriceUSD" -ForegroundColor Gray
Write-Host "     Liquidator : $LIQUIDATOR" -ForegroundColor Gray
Write-Host "     USDC       : $liquidatorUSDC_val" -ForegroundColor Gray
Write-Host ""

# Tinh PriceDrop can thiet dua vao HF cao nhat
$maxHF = ($borrowerResults | Measure-Object -Property HF -Maximum).Maximum
if ($null -ne $maxHF -and $maxHF -lt 999999) {
    $neededDrop = [math]::Round((1 - 1.0 / $maxHF) * 100 + 5, 0)
    Write-Host "  --> Buoc tiep theo:" -ForegroundColor Yellow
    Write-Host "     1. .\scripts\multi-users\crash_price_multi_wstETH.ps1 -PriceDrop $neededDrop" -ForegroundColor Yellow
    Write-Host "     2. cargo test executor                   - Integration test" -ForegroundColor Yellow
    Write-Host "     3. cargo run                             - Chay liquidator bot" -ForegroundColor Yellow
}
Write-Host ""
Write-Host "  [i] Cac Borrower address (copy vao config neu can):" -ForegroundColor Gray
foreach ($b in $BORROWERS[0..($successCount-1)]) {
    Write-Host "     $($b.Address)" -ForegroundColor DarkGray
}