# ============================================================================
# BENCHMARK LATENCY - wstETH / USDC on Aave (Hardhat Fork)
# ============================================================================
#
# Script nay do do tre (latency) cua 5 loai giao dich tren Aave:
#   - Deposit    (3 lan)
#   - Borrow     (3 lan)
#   - Repay      (3 lan)
#   - Withdraw   (3 lan)
#   - PriceUpdate (3 lan - thay doi gia wstETH qua MockPriceFeed)
#
# Ket qua in ra bang tong hop cuoi script de copy vao bao cao.
#
# Yeu cau:
#   - Hardhat dang chay (scripts/start_hardhat.ps1)
#   - Da chay setup_liquidation_scenario_wstETH.ps1
#   - MockPriceFeed da duoc compile (out\MockPriceFeed.sol\MockPriceFeed.json)
#
# Cach dung:
#   .\scripts\single-user\benchmark_latency_wstETH.ps1
#   .\scripts\single-user\benchmark_latency_wstETH.ps1 -RpcUrl http://127.0.0.1:8545
# ============================================================================

param(
    [string]$RpcUrl = "http://127.0.0.1:8545"
)

# ============================================================================
# CONFIGURATION
# ============================================================================

$AAVE_POOL   = "0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2"
$AAVE_ORACLE = "0x54586bE62E3c3580375aE3723C145253060Ca0C2"
$WSTETH      = "0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0"
$USDC        = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"

$BORROWER      = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"
$BORROWER_KEY  = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"
$DEPLOYER_KEY  = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"

$REPEAT = 3   # So lan lap lai moi loai giao dich

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

    $to      = $parsed.Groups['to'].Value
    $sig     = $parsed.Groups['sig'].Value
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
            jsonrpc = "2.0"; id = 1
            method  = "eth_call"
            params  = @(@{ to = $to; data = $calldata }, "latest")
        } | ConvertTo-Json -Compress

        $resp   = Invoke-RestMethod -Uri $RpcUrl -Method Post -Body $payload -ContentType "application/json"
        $rawHex = $resp.result
        if ([string]::IsNullOrWhiteSpace($rawHex) -or $rawHex -eq "0x") { return "" }

        if ($sig -match '\)\(.*\)$') {
            $decode = & cast abi-decode $sig $rawHex 2>&1
            if ($LASTEXITCODE -eq 0) { return ($decode | Out-String).Trim() }
        }

        return $rawHex
    } catch { return "" }
}

function Invoke-CastSend {
    param([string]$CastArgs)
    $cmd    = "cast send $CastArgs --rpc-url $RpcUrl --gas-limit 5000000 --legacy"
    $result = Invoke-Expression $cmd 2>&1
    return (($result | Out-String).Trim())
}

function Invoke-CastRpc {
    param([string]$CastArgs)
    $cmd    = "cast rpc $CastArgs --rpc-url $RpcUrl"
    $result = Invoke-Expression $cmd 2>&1
    return (($result | Out-String).Trim())
}

function Strip-CastAnnotation {
    param([string]$Value)
    return (($Value -replace '\[.*?\]', '').Trim())
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

# Gui tx bang cast send --json, tach stdout/stderr de parse JSON sach
function Invoke-CastSendJson {
    param([string]$CastSendArgs)

    $tmpOut = [System.IO.Path]::GetTempFileName()
    $tmpErr = [System.IO.Path]::GetTempFileName()

    try {
        $proc = Start-Process -FilePath "cast" `
            -ArgumentList ("send " + $CastSendArgs + " --rpc-url $RpcUrl --gas-limit 5000000 --legacy --json") `
            -RedirectStandardOutput $tmpOut `
            -RedirectStandardError  $tmpErr `
            -NoNewWindow -Wait -PassThru

        $stdout = if ((Get-Item $tmpOut).Length -gt 0) { (Get-Content $tmpOut -Raw).Trim() } else { "" }
        $stderr = if ((Get-Item $tmpErr).Length -gt 0) { (Get-Content $tmpErr -Raw).Trim() } else { "" }

        return @{ Stdout = $stdout; Stderr = $stderr; ExitCode = $proc.ExitCode }
    } finally {
        Remove-Item $tmpOut, $tmpErr -ErrorAction SilentlyContinue
    }
}

# Ghi startTime truoc khi gui tx.
# Latency duoc tinh o ngoai bang cong thuc:
#   Latency = Bot_detect_time - StartTime
# (chap nhan lech vai chuc ms do overhead cast send)
function Measure-TxLatency {
    param(
        [string]$TxType,
        [int]$Run,
        [string]$CastSendArgs
    )

    $startTime = (Get-Date).ToUniversalTime()
    $startStr  = $startTime.ToString("yyyy-MM-ddTHH:mm:ss.ffffffZ")
    Write-Host "  [>] [$TxType] Run $Run - Start: $startStr" -ForegroundColor Yellow

    $result = Invoke-CastSendJson $CastSendArgs

    if ($result.ExitCode -ne 0 -or [string]::IsNullOrWhiteSpace($result.Stdout)) {
        Write-Host "  [X] [$TxType] Run $Run - cast send that bai (exit $($result.ExitCode))" -ForegroundColor Red
        Write-Host "      $($result.Stderr -split "`n" | Select-Object -First 2 | Out-String)" -ForegroundColor DarkRed
        return $null
    }

    $txHash      = $null
    $blockNumber = $null
    try {
        $json        = $result.Stdout | ConvertFrom-Json
        $txHash      = $json.transactionHash
        $blockNumber = [Convert]::ToInt64($json.blockNumber, 16)
    } catch {
        if ($result.Stdout -match '"transactionHash"\s*:\s*"(0x[a-fA-F0-9]{64})"') { $txHash = $Matches[1] }
        if ($result.Stdout -match '"blockNumber"\s*:\s*"(0x[a-fA-F0-9]+)"')         { $blockNumber = [Convert]::ToInt64($Matches[1], 16) }
    }

    if ([string]::IsNullOrWhiteSpace($txHash)) {
        Write-Host "  [X] [$TxType] Run $Run - Khong parse duoc txHash!" -ForegroundColor Red
        return $null
    }

    Write-Host "  [#] [$TxType] Run $Run - TxHash:  $txHash" -ForegroundColor DarkGray
    Write-Host "  [#] [$TxType] Run $Run - Block#:  $blockNumber" -ForegroundColor DarkGray
    Write-Host "  [i] [$TxType] Run $Run - Latency = Bot_detect_time - $startStr" -ForegroundColor DarkYellow

    return [PSCustomObject]@{
        TxType      = $TxType
        Run         = $Run
        StartTime   = $startStr
        BlockNumber = $blockNumber
        TxHash      = $txHash
    }
}

# ============================================================================
# BANNER
# ============================================================================

Write-Host "============================================" -ForegroundColor Magenta
Write-Host "  BENCHMARK LATENCY - wstETH / USDC Aave" -ForegroundColor Magenta
Write-Host "============================================" -ForegroundColor Magenta
Write-Host ""
$scriptStartTime = (Get-Date).ToUniversalTime()
Write-Host "  [*] Start time: $($scriptStartTime.ToString('yyyy-MM-ddTHH:mm:ss.ffffffZ'))" -ForegroundColor Magenta
Write-Host "  [*] RPC: $RpcUrl" -ForegroundColor Gray
Write-Host "  [*] Repeat per tx type: $REPEAT" -ForegroundColor Gray

# Precheck
if (-not (Get-Command "cast" -ErrorAction SilentlyContinue)) {
    Write-Host "  [X] Cast (Foundry) chua duoc cai dat!" -ForegroundColor Red
    exit 1
}

$chainIdRaw = Invoke-Expression "cast chain-id --rpc-url $RpcUrl" 2>&1
$chainId    = ($chainIdRaw | Out-String).Trim()
if ([string]::IsNullOrWhiteSpace($chainId)) {
    Write-Host "  [X] Khong the ket noi RPC!" -ForegroundColor Red
    exit 1
}
Write-Host "  [OK] Connected (Chain ID: $chainId)" -ForegroundColor Green

# Lay gia wstETH va wstETH price source
$wstSourceRaw    = Invoke-CastCall "$AAVE_ORACLE `"getSourceOfAsset(address)(address)`" $WSTETH"
$WSTETH_FEED     = ($wstSourceRaw -replace '\[.*?\]', '').Trim()
$wstPriceRaw     = Invoke-CastCall "$WSTETH_FEED `"latestAnswer()(int256)`""
$wstPriceCurrent = [decimal](Strip-CastAnnotation $wstPriceRaw)

if ($wstPriceCurrent -eq 0) {
    Write-Host "  [X] Khong doc duoc gia wstETH hien tai!" -ForegroundColor Red
    exit 1
}

Write-Host "  [i] wstETH Feed: $WSTETH_FEED" -ForegroundColor Gray
Write-Host "  [i] wstETH Price: $([math]::Round($wstPriceCurrent / 1e8, 4)) USD" -ForegroundColor Gray

# Ket qua tong hop
$allResults = @()

# ============================================================================
# STEP 1: DEPOSIT (3 lan)
# ============================================================================
Write-Step "1/5" "DEPOSIT wstETH (x$REPEAT)"

# Chuan bi: approve wstETH truoc
$maxApproval = "115792089237316195423570985008687907853269984665640564039457584007913129639935"
Invoke-CastSend "$WSTETH `"approve(address,uint256)`" $AAVE_POOL $maxApproval --private-key $BORROWER_KEY" | Out-Null
Invoke-CastRpc "evm_mine" | Out-Null

# Moi lan deposit 0.01 wstETH
$depositAmt = "10000000000000000"  # 0.01 wstETH in wei

for ($i = 1; $i -le $REPEAT; $i++) {
    $r = Measure-TxLatency -TxType "Deposit" -Run $i `
        -CastSendArgs "$AAVE_POOL `"supply(address,uint256,address,uint16)`" $WSTETH $depositAmt $BORROWER 0 --private-key $BORROWER_KEY"
    if ($null -ne $r) { $allResults += $r }
}

# ============================================================================
# STEP 2: BORROW (3 lan)
# ============================================================================
Write-Step "2/5" "BORROW USDC (x$REPEAT)"

# Moi lan borrow 1 USDC (1_000_000 wei voi 6 decimals)
$borrowAmt = "1000000"  # 1 USDC

for ($i = 1; $i -le $REPEAT; $i++) {
    $r = Measure-TxLatency -TxType "Borrow" -Run $i `
        -CastSendArgs "$AAVE_POOL `"borrow(address,uint256,uint256,uint16,address)`" $USDC $borrowAmt 2 0 $BORROWER --private-key $BORROWER_KEY"
    if ($null -ne $r) { $allResults += $r }
}

# ============================================================================
# STEP 3: REPAY (3 lan)
# ============================================================================
Write-Step "3/5" "REPAY USDC (x$REPEAT)"

# Approve USDC cho Aave Pool truoc
Invoke-CastSend "$USDC `"approve(address,uint256)`" $AAVE_POOL $maxApproval --private-key $BORROWER_KEY" | Out-Null
Invoke-CastRpc "evm_mine" | Out-Null

$repayAmt = "1000000"  # 1 USDC

for ($i = 1; $i -le $REPEAT; $i++) {
    $r = Measure-TxLatency -TxType "Repay" -Run $i `
        -CastSendArgs "$AAVE_POOL `"repay(address,uint256,uint256,address)`" $USDC $repayAmt 2 $BORROWER --private-key $BORROWER_KEY"
    if ($null -ne $r) { $allResults += $r }
}

# ============================================================================
# STEP 4: WITHDRAW (3 lan)
# ============================================================================
Write-Step "4/5" "WITHDRAW wstETH (x$REPEAT)"

$withdrawAmt = "1000000000000000"  # 0.001 wstETH in wei

for ($i = 1; $i -le $REPEAT; $i++) {
    $r = Measure-TxLatency -TxType "Withdraw" -Run $i `
        -CastSendArgs "$AAVE_POOL `"withdraw(address,uint256,address)`" $WSTETH $withdrawAmt $BORROWER --private-key $BORROWER_KEY"
    if ($null -ne $r) { $allResults += $r }
}

# ============================================================================
# STEP 5: PRICE UPDATE (3 lan) - dung MockPriceFeed
# ============================================================================
Write-Step "5/5" "PRICE UPDATE wstETH via MockPriceFeed (x$REPEAT)"

$mockJsonPath = "out\MockPriceFeed.sol\MockPriceFeed.json"
if (-not (Test-Path $mockJsonPath)) {
    Write-Host "  [!] MockPriceFeed chua compile, dang compile..." -ForegroundColor Yellow
    $null = Invoke-Expression "forge build contracts/MockPriceFeed.sol 2>&1"
}

if (-not (Test-Path $mockJsonPath)) {
    Write-Host "  [X] Khong tim thay MockPriceFeed bytecode, skip Price Update!" -ForegroundColor Red
} else {
    $mockJson        = Get-Content $mockJsonPath | ConvertFrom-Json
    $deployedBytecode = $mockJson.deployedBytecode.object

    # Replace feed code 1 lan truoc khi do
    Invoke-Cast "rpc hardhat_setCode $WSTETH_FEED $deployedBytecode" | Out-Null
    Invoke-Cast "rpc hardhat_setStorageAt $WSTETH_FEED `"0x0000000000000000000000000000000000000000000000000000000000000001`" `"0x0000000000000000000000000000000000000000000000000000000000000008`"" | Out-Null

    # Cac muc gia se duoc thay doi qua lai (tang/giam nhe 1%)
    $prices = @(
        [math]::Floor($wstPriceCurrent * 0.99),
        [math]::Floor($wstPriceCurrent * 1.01),
        [math]::Floor($wstPriceCurrent * 0.98)
    )

    for ($i = 1; $i -le $REPEAT; $i++) {
        $newPrice = $prices[$i - 1]
        Write-Host "  [i] Set price -> $([math]::Round($newPrice / 1e8, 4)) USD" -ForegroundColor Gray

        $r = Measure-TxLatency -TxType "PriceUpdate" -Run $i `
            -CastSendArgs "$WSTETH_FEED `"setAnswer(int256)`" $newPrice --private-key $DEPLOYER_KEY"
        if ($null -ne $r) { $allResults += $r }
    }
}

# ============================================================================
# TONG HOP KET QUA
# ============================================================================

Write-Host ""
Write-Host "============================================================" -ForegroundColor Magenta
Write-Host "  KET QUA BENCHMARK - START TIME DE TINH LATENCY VOI BOT" -ForegroundColor Magenta
Write-Host "  Cong thuc: Latency = Bot_detect_time - StartTime" -ForegroundColor DarkYellow
Write-Host "============================================================" -ForegroundColor Magenta
Write-Host ""

$txTypes = @("Deposit", "Borrow", "Repay", "Withdraw", "PriceUpdate")
$validResults = $allResults | Where-Object { $null -ne $_ }

Write-Host "  +------------------+-----+----------------+------------------------------------------+------------------------------------------+" -ForegroundColor White
Write-Host "  | TxType           | Run | Block#         | Start Time (UTC)                         | TxHash                                   |" -ForegroundColor White
Write-Host "  +------------------+-----+----------------+------------------------------------------+------------------------------------------+" -ForegroundColor White

foreach ($row in $validResults) {
    $txPad    = $row.TxType.PadRight(16)
    $runPad   = ($row.Run.ToString()).PadLeft(3)
    $blkPad   = ($row.BlockNumber.ToString()).PadRight(14)
    $startPad = $row.StartTime.PadRight(40)
    $hashPad  = $row.TxHash.PadRight(40)
    Write-Host "  | $txPad | $runPad | $blkPad | $startPad | $hashPad |" -ForegroundColor Gray
}

Write-Host "  +------------------+-----+----------------+------------------------------------------+------------------------------------------+" -ForegroundColor White

Write-Host ""
$scriptEndTime = (Get-Date).ToUniversalTime()
$totalSec = [math]::Round(($scriptEndTime - $scriptStartTime).TotalSeconds, 2)
Write-Host "  [*] End time   : $($scriptEndTime.ToString('yyyy-MM-ddTHH:mm:ss.ffffffZ'))" -ForegroundColor Magenta
Write-Host "  [*] Total time : ${totalSec}s" -ForegroundColor Magenta
Write-Host ""
Write-Host "============================================================" -ForegroundColor Magenta
Write-Host "  BENCHMARK COMPLETE" -ForegroundColor Magenta
Write-Host "============================================================" -ForegroundColor Magenta