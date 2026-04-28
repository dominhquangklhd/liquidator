# ============================================================================
# CRASH ETH PRICE - Make MULTIPLE Positions Liquidatable Simultaneously
# ============================================================================
#
# Script nay crash gia ETH de tat ca cac position cua nhieu borrower
# cung tro nen liquidatable (HF < 1.0) trong 1 lan:
#   1. Doc HF hien tai cua tat ca borrowers
#   2. Tinh PriceDrop toi uu (du de push nguoi kho nhat xuong HF < 1.0)
#   3. Deploy/update MockPriceFeed voi gia moi
#   4. Verify tung borrower HF < 1.0
#
# Yeu cau:
#   - Hardhat dang chay (scripts/start_hardhat.ps1)
#   - Da chay setup_multi_liquidation.ps1
#
# Cach dung:
#   .\scripts\multi_users\crash_price_multi.ps1                  # Tu dong tinh PriceDrop
#   .\scripts\multi_users\crash_price_multi.ps1 -PriceDrop 30    # Drop 30% co dinh
#   .\scripts\multi_users\crash_price_multi.ps1 -SeedAaveEvent   # Emit them event cho bot
# ============================================================================

param(
    [string]$RpcUrl = "http://127.0.0.1:8545",
    [int]$PriceDrop = 0,      # 0 = tu dong tinh du dua vao HF thap nhat
    [int]$PriceDropBuffer = 8, # Them % buffer de chac chan HF < 1.0
    [switch]$SeedAaveEvent    # Optional: emit extra Aave event sau crash
)

# ============================================================================
# NETWORK CONFIGURATION
# ============================================================================

$MAINNET_CONFIG = @{
    AAVE_POOL               = "0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2"
    AAVE_ORACLE             = "0x54586bE62E3c3580375aE3723C145253060Ca0C2"
    POOL_ADDRESSES_PROVIDER = "0x2f39d218133AFaB8F2B819B1066c7E434Ad94E9e"
    ACL_MANAGER             = "0xc2aaCf6553D20d1e9571216f576571920c0FBB3d"
    WETH                    = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"
    USDC                    = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"
    ETH_USD_FEED            = "0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419"
    NetworkName             = "Ethereum Mainnet"
}


# ============================================================================
# 10 BORROWERS (phai khop voi setup_multi_liquidation.ps1)
# ============================================================================
$BORROWERS = @(
    @{ Address = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"; Key = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"; Label = "Borrower-02" },
    @{ Address = "0x90F79bf6EB2c4f870365E785982E1f101E93b906"; Key = "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6"; Label = "Borrower-03" },
    @{ Address = "0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65"; Key = "0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926b"; Label = "Borrower-04" },
    @{ Address = "0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc"; Key = "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba"; Label = "Borrower-05" },
    @{ Address = "0x976EA74026E726554dB657fA54763abd0C3a0aa9"; Key = "0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e"; Label = "Borrower-06" },
    @{ Address = "0x14dC79964da2C08b23698B3D3cc7Ca32193d9955"; Key = "0x4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356"; Label = "Borrower-07" },
    @{ Address = "0x23618e81E3f5cdF7f54C3d65f7FBc0aBf5B21E8f"; Key = "0xdbda1821b80551c9d65939329250132c444d57e4ef0a7a3fffce5fc96bf0af81"; Label = "Borrower-08" },
    @{ Address = "0xa0Ee7A142d267C1f36714E4a8F75612F20a79720"; Key = "0x2a871d0798f97d79848a013d4936a73bf4cc922c825d33c1cf7073dff6d409c6"; Label = "Borrower-09" },
    @{ Address = "0xBcd4042DE499D14e55001CcbB24a551F3b954096"; Key = "0xf214f2b2cd398c806f84e317254e0f0b801d0643303237d97a22a48e01628897"; Label = "Borrower-10" },
    @{ Address = "0x71bE63f3384f5fb98995898A86B02Fb2426c5788"; Key = "0x701b615bbdfb9de65240bc28bd21bbc0d996645a3dd57e7b12bc2bdf6f192c82"; Label = "Borrower-11" }
)

$DEPLOYER_KEY = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"

# ============================================================================
# HELPER FUNCTIONS
# ============================================================================

function Invoke-Cast {
    param([string]$CastArgs)
    $cmd = "cast $CastArgs --rpc-url $RpcUrl"
    $result = Invoke-Expression $cmd 2>&1
    return ($result | Out-String).Trim()
}

function Invoke-CastCall {
    param([string]$CallArgs)
    $parsed = [regex]::Match($CallArgs, '^(?<to>0x[a-fA-F0-9]{40})\s+"(?<sig>[^"]+)"\s*(?<args>.*)$')
    if (-not $parsed.Success) { return Invoke-Cast "call $CallArgs" }

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
    if ($LASTEXITCODE -ne 0 -or -not ($calldata -match '^0x[0-9a-fA-F]+$')) { return "" }

    try {
        $payload = @{
            jsonrpc = "2.0"; id = 1; method = "eth_call"
            params  = @(@{ to = $to; data = $calldata }, "latest")
        } | ConvertTo-Json -Compress

        $resp = Invoke-RestMethod -Uri $RpcUrl -Method Post -Body $payload -ContentType "application/json"
        $rawHex = $resp.result
        if ([string]::IsNullOrWhiteSpace($rawHex) -or $rawHex -eq "0x") { return "" }

        if ($sig -match '\)\(.*\)$') {
            $decode = & cast abi-decode $sig $rawHex 2>&1
            if ($LASTEXITCODE -eq 0) { return ($decode | Out-String).Trim() }
        }
        return $rawHex
    } catch { return "" }
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

function Format-HFStatus {
    param([decimal]$HF)
    if ($HF -ge 999999) { return @{ Text = "Infinity (no debt)"; Color = "Gray" } }
    if ($HF -lt 1.0)    { return @{ Text = "$HF [LIQUIDATABLE]"; Color = "Red" } }
    if ($HF -lt 1.1)    { return @{ Text = "$HF [NEAR]";         Color = "Yellow" } }
    return                       @{ Text = "$HF";                 Color = "Green" }
}

# ============================================================================
# MAIN
# ============================================================================

Write-Host ""
Write-Host "============================================" -ForegroundColor Green
Write-Host "  CRASH PRICE - MULTI BORROWER ($($BORROWERS.Count) positions)" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green

if (-not (Get-Command "cast" -ErrorAction SilentlyContinue)) {
    Write-Host "[X] Cast (Foundry) chua duoc cai dat!" -ForegroundColor Red; exit 1
}

$chainIdRaw = Invoke-Expression "cast chain-id --rpc-url $RpcUrl" 2>&1
$chainId = ($chainIdRaw | Out-String).Trim()
if ($LASTEXITCODE -ne 0) {
    Write-Host "[X] Khong ket noi RPC: $RpcUrl" -ForegroundColor Red; exit 1
}

$CONFIG = $MAINNET_CONFIG
$AAVE_POOL   = $CONFIG.AAVE_POOL
$AAVE_ORACLE = $CONFIG.AAVE_ORACLE
$WETH        = $CONFIG.WETH
$USDC        = $CONFIG.USDC
$ETH_USD_FEED = $CONFIG.ETH_USD_FEED

Write-Host "[OK] Connected: $($CONFIG.NetworkName) (Chain ID: $chainId)" -ForegroundColor Green

# ============================================================================
# STEP 1: Doc gia ETH hien tai
# ============================================================================
Write-Step "1/5" "Doc gia ETH hien tai"

$ethPriceRaw = Invoke-CastCall "$ETH_USD_FEED `"latestAnswer()(int256)`""
$ethPrice = try { [decimal](Strip-CastAnnotation $ethPriceRaw) } catch { 0 }
if ($ethPrice -le 0) {
    $ethPriceRaw = Invoke-CastCall "$AAVE_ORACLE `"getAssetPrice(address)(uint256)`" $WETH"
    $ethPrice = try { [decimal](Strip-CastAnnotation $ethPriceRaw) } catch { 0 }
}

if ($ethPrice -le 0) {
    Write-Host "  [X] Khong doc duoc gia ETH!" -ForegroundColor Red; exit 1
}

$ethPriceUSD = [math]::Round($ethPrice / 1e8, 2)
Write-Host "  [i] ETH/USD hien tai: `$$ethPriceUSD (raw: $ethPrice)" -ForegroundColor Gray

# Lay WETH price source tu Aave Oracle
$wethPriceSourceRaw = Invoke-CastCall "$AAVE_ORACLE `"getSourceOfAsset(address)(address)`" $WETH"
$WETH_PRICE_SOURCE = (Strip-CastAnnotation $wethPriceSourceRaw).Trim()
if (-not ($WETH_PRICE_SOURCE -match '^0x[a-fA-F0-9]{40}$')) {
    $WETH_PRICE_SOURCE = $ETH_USD_FEED
    Write-Host "  [!] Fallback: dung ETH_USD_FEED lam price source" -ForegroundColor Yellow
} else {
    Write-Host "  [i] WETH price source: $WETH_PRICE_SOURCE" -ForegroundColor Gray
}

# ============================================================================
# STEP 2: Doc HF tat ca borrowers & tinh PriceDrop toi uu
# ============================================================================
Write-Step "2/5" "Scan Health Factor $($BORROWERS.Count) Borrowers"

$hfResults = @()
$minHF = 999999.0
$maxHF = 0.0

foreach ($borrower in $BORROWERS) {
    $accountData = Invoke-CastCall "$AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $($borrower.Address)"
    $hf = Get-HealthFactor $accountData
    $values = Parse-CastValues $accountData
    $hasDebt = $values.Count -ge 2 -and [decimal]$values[1] -gt 0

    $status = Format-HFStatus -HF $hf
    Write-Host "  $($borrower.Label) : $($status.Text)" -ForegroundColor $status.Color

    if ($hasDebt -and $hf -lt 999999) {
        $hfResults += [PSCustomObject]@{ Label = $borrower.Label; Address = $borrower.Address; HF = $hf; HasDebt = $true }
        if ($hf -lt $minHF) { $minHF = $hf }
        if ($hf -gt $maxHF) { $maxHF = $hf }
    } else {
        $hfResults += [PSCustomObject]@{ Label = $borrower.Label; Address = $borrower.Address; HF = $hf; HasDebt = $false }
        Write-Host "    [!] Khong co debt, bo qua" -ForegroundColor DarkGray
    }
}

$activeCount = ($hfResults | Where-Object { $_.HasDebt }).Count
Write-Host ""
Write-Host "  [i] Active borrowers (co debt): $activeCount / $($BORROWERS.Count)" -ForegroundColor Cyan
Write-Host "  [i] HF cao nhat: $maxHF  |  HF thap nhat: $minHF" -ForegroundColor Gray

if ($activeCount -eq 0) {
    Write-Host "  [X] Khong co borrower nao co debt! Hay chay setup_multi_liquidation.ps1 truoc." -ForegroundColor Red
    exit 1
}

# Tinh PriceDrop can thiet de push CA HOF cao nhat xuong HF < 1.0
# HF_new = HF_old * (newPrice / oldPrice)
# => newPrice/oldPrice = targetHF / HF_old
# => priceDrop = (1 - targetHF/HF_old) * 100
if ($PriceDrop -le 0) {
    # Can drop du de ngay ca borrower co HF cao nhat cung duoi 1.0
    $targetHFAfterCrash = 0.93  # Muon tat ca xuong ~ 0.93 (bien de bot co the chay)
    $dropNeeded = (1 - $targetHFAfterCrash / $maxHF) * 100
    $PriceDrop = [math]::Min([int][math]::Ceiling($dropNeeded) + $PriceDropBuffer, 92)
    $PriceDrop = [math]::Max($PriceDrop, 5)
    Write-Host "  [i] Tu dong tinh PriceDrop: $PriceDrop% (target HF_max -> ~$targetHFAfterCrash)" -ForegroundColor Cyan
} else {
    Write-Host "  [i] PriceDrop co dinh: $PriceDrop%" -ForegroundColor Cyan
}

# ============================================================================
# STEP 3: Tinh gia moi & kiem tra he so phan bo
# ============================================================================
Write-Step "3/5" "Tinh gia ETH moi sau khi crash $PriceDrop%"

$newPrice    = [long]([math]::Floor($ethPrice * (100 - $PriceDrop) / 100))
$newPriceUSD = [math]::Round($newPrice / 1e8, 2)

Write-Host "  [i] ETH truoc crash : `$$ethPriceUSD" -ForegroundColor Gray
Write-Host "  [i] ETH sau crash   : `$$newPriceUSD  (-$PriceDrop%)" -ForegroundColor Red
Write-Host ""

# Uoc tinh HF moi cua tung borrower
Write-Host "  [i] Du bao HF sau crash:" -ForegroundColor Gray
$allWillBeLiquidatable = $true
foreach ($r in $hfResults | Where-Object { $_.HasDebt }) {
    $priceRatio = $newPrice / $ethPrice
    $estimatedHF = [math]::Round($r.HF * $priceRatio, 4)
    $willLiquidate = $estimatedHF -lt 1.0
    if (-not $willLiquidate) { $allWillBeLiquidatable = $false }
    $color = if ($willLiquidate) { "Red" } else { "Yellow" }
    $note  = if ($willLiquidate) { "[LIQUIDATABLE]" } else { "[SAFE - tang PriceDrop!]" }
    Write-Host "     $($r.Label) : HF $($r.HF) -> ~$estimatedHF $note" -ForegroundColor $color
}

if (-not $allWillBeLiquidatable) {
    Write-Host ""
    Write-Host "  [!] Mot so position van safe! Tang -PriceDrop hoac giam BorrowRatio." -ForegroundColor Yellow
}

$newPriceHex = "0x" + ([Convert]::ToString([long]$newPrice, 16)).PadLeft(64, '0')

# ==========================================================================
# STEP 4: Update gia tren MockPriceFeed (replace bytecode / setAnswer)
# ==========================================================================
Write-Step "4/5" "Cap nhat ETH price -> `$$newPriceUSD"

$targetFeeds = @($WETH_PRICE_SOURCE)
if ($ETH_USD_FEED -ne $WETH_PRICE_SOURCE) { $targetFeeds += $ETH_USD_FEED }

$mockJsonPath = "out\MockPriceFeed.sol\MockPriceFeed.json"
if (-not (Test-Path $mockJsonPath)) {
    Write-Host "  [!] MockPriceFeed chua compile, dang compile..." -ForegroundColor Yellow
    $null = Invoke-Expression "forge build contracts/MockPriceFeed.sol 2>&1"
}

if (-not (Test-Path $mockJsonPath)) {
    Write-Host "  [X] Khong tim thay MockPriceFeed bytecode!" -ForegroundColor Red
    exit 1
}

$mockJson = Get-Content $mockJsonPath | ConvertFrom-Json
$deployedBytecode = $mockJson.deployedBytecode.object

foreach ($feed in $targetFeeds) {
    Write-Host "  [>] Cap nhat feed: $feed" -ForegroundColor Gray

    # Replace code before setAnswer(); neu khong setAnswer se luon fail tren feed that.
    Invoke-Cast "rpc hardhat_setCode $feed $deployedBytecode" | Out-Null
    Write-Host "  [OK] Contract code replaced" -ForegroundColor Green

    # Dam bao decimals = 8 cho parser.
    Invoke-Cast "rpc hardhat_setStorageAt $feed `"0x0000000000000000000000000000000000000000000000000000000000000001`" `"0x0000000000000000000000000000000000000000000000000000000000000008`"" | Out-Null

    # Thu setAnswer() de emit AnswerUpdated event (bot co the catch WS).
    $setAnswerOut = Invoke-Cast "send $feed `"setAnswer(int256)`" $newPrice --private-key $DEPLOYER_KEY --gas-limit 5000000 --legacy"
    if ($setAnswerOut -match "0x[a-fA-F0-9]{64}") {
        Write-Host "  [OK] setAnswer() - AnswerUpdated event emitted" -ForegroundColor Green
    } else {
        # Fallback: ghi storage slot 0.
        Write-Host "  [!] setAnswer that bai, fallback storage slot 0..." -ForegroundColor Yellow
        Invoke-Cast "rpc hardhat_setStorageAt $feed `"0x0000000000000000000000000000000000000000000000000000000000000000`" $newPriceHex" | Out-Null
        Write-Host "  [OK] Storage slot 0 da duoc ghi: $newPrice" -ForegroundColor Green
    }
}

# Mine block moi de state duoc phan anh
Invoke-Cast "rpc evm_mine" | Out-Null
Write-Host "  [OK] Block mined" -ForegroundColor Green

# ============================================================================
# STEP 5: Verify tung borrower HF < 1.0
# ============================================================================
Write-Step "5/5" "Verify HF sau crash - Tat ca $($BORROWERS.Count) Borrowers"

Start-Sleep -Seconds 1

# Doc gia moi
$newEthPriceActualRaw = Invoke-CastCall "$WETH_PRICE_SOURCE `"latestAnswer()(int256)`""
$newEthPriceActual = try { [decimal](Strip-CastAnnotation $newEthPriceActualRaw) } catch { 0 }
if ($newEthPriceActual -gt 0) {
    Write-Host "  [`$] ETH/USD SAU CRASH: `$$([math]::Round($newEthPriceActual/1e8,2))" -ForegroundColor Red
} else {
    Write-Host "  [!] Khong doc duoc gia moi (co the fallback storage)" -ForegroundColor Yellow
}

Write-Host ""

$liquidatableCount = 0
$safeCount = 0
$finalResults = @()

foreach ($borrower in $BORROWERS) {
    $accountData = Invoke-CastCall "$AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $($borrower.Address)"
    $hfAfter = Get-HealthFactor $accountData
    $values = Parse-CastValues $accountData
    $debtUSD = if ($values.Count -ge 2) { [math]::Round([decimal]$values[1] / 1e8, 2) } else { 0 }

    $isLiquidatable = $hfAfter -lt 1.0 -and $debtUSD -gt 0
    if ($isLiquidatable) { $liquidatableCount++ } else { $safeCount++ }

    $status = Format-HFStatus -HF $hfAfter
    $debtStr = if ($debtUSD -gt 0) { "  Debt: `$$debtUSD" } else { "  (no debt)" }
    Write-Host "  $($borrower.Label) : $($status.Text)$debtStr" -ForegroundColor $status.Color

    $finalResults += [PSCustomObject]@{
        Label          = $borrower.Label
        Address        = $borrower.Address
        HF             = $hfAfter
        IsLiquidatable = $isLiquidatable
        DebtUSD        = $debtUSD
    }
}

# ============================================================================
# OPTIONAL: Seed Aave event
# ============================================================================
if ($SeedAaveEvent) {
    Write-Host ""
    Write-Host "  [>] Seeding Aave event (tiny repay tu Borrower-02)..." -ForegroundColor Gray
    $seedBorrower = $BORROWERS[0]
    try {
        $seedRepayAmount = "1000"  # 0.001 USDC
        $txOut = Invoke-Cast "send $AAVE_POOL `"repay(address,uint256,uint256,address)`" $USDC $seedRepayAmount 2 $($seedBorrower.Address) --private-key $($seedBorrower.Key)"
        if ($txOut -match "0x[a-fA-F0-9]{64}") {
            Write-Host "  [OK] Repay event emitted (tx: $($Matches[0].Substring(0,10))...)" -ForegroundColor Green
        }
        Start-Sleep -Milliseconds 500
    } catch {
        Write-Host "  [!] Seed event that bai (non-critical): $_" -ForegroundColor Yellow
    }
} else {
    Write-Host ""
    Write-Host "  [i] Skip seed Aave event (them -SeedAaveEvent de bat)" -ForegroundColor DarkGray
}

# ============================================================================
# SUMMARY
# ============================================================================
Write-Host ""
Write-Host "============================================" -ForegroundColor Green
Write-Host "  PRICE CRASH COMPLETE - MULTI BORROWER" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green
Write-Host ""
Write-Host "  [*] ETH/USD : `$$ethPriceUSD --> `$$newPriceUSD  (-$PriceDrop%)" -ForegroundColor Cyan
Write-Host ""

if ($liquidatableCount -gt 0) {
    Write-Host "  !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!" -ForegroundColor Red
    Write-Host "  !  $liquidatableCount / $($BORROWERS.Count) POSITIONS LIQUIDATABLE  !" -ForegroundColor Red
    Write-Host "  !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!" -ForegroundColor Red
} else {
    Write-Host "  [!] Khong co position nao liquidatable!" -ForegroundColor Yellow
    Write-Host "      Tang PriceDrop: .\scripts\crash_price_multi.ps1 -PriceDrop $([math]::Min($PriceDrop + 10, 92))" -ForegroundColor Yellow
}

Write-Host ""
Write-Host "  [i] Chi tiet tung position:" -ForegroundColor Cyan
foreach ($r in $finalResults) {
    $statusStr = if ($r.IsLiquidatable) { "LIQUIDATABLE" } elseif ($r.DebtUSD -eq 0) { "no debt" } else { "SAFE (HF $($r.HF))" }
    $color = if ($r.IsLiquidatable) { "Red" } elseif ($r.DebtUSD -eq 0) { "DarkGray" } else { "Yellow" }
    Write-Host "     $($r.Label) : $statusStr  Debt = `$$($r.DebtUSD)" -ForegroundColor $color
}

Write-Host ""
Write-Host "  --> Buoc tiep theo:" -ForegroundColor Yellow
Write-Host "     cargo test --test executor_integration -- --nocapture" -ForegroundColor Yellow
Write-Host "     (hoac: cargo run -- --once)" -ForegroundColor Yellow
Write-Host ""
Write-Host "  [i] Reset gia ve ban dau:" -ForegroundColor Gray
Write-Host "     cast send $WETH_PRICE_SOURCE `"setAnswer(int256)`" $([long]$ethPrice) --private-key $DEPLOYER_KEY --rpc-url $RpcUrl" -ForegroundColor DarkGray

# In ra danh sach address de paste vao test
Write-Host ""
Write-Host "  [i] Liquidatable addresses (copy vao integration test):" -ForegroundColor Gray
$liquidatableAddresses = ($finalResults | Where-Object { $_.IsLiquidatable } | ForEach-Object { "`"$($_.Address)`"" }) -join ", "
Write-Host "     [$liquidatableAddresses]" -ForegroundColor DarkCyan
