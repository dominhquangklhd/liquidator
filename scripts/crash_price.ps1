# ============================================================================
# CRASH ETH PRICE - Make Position Liquidatable
# ============================================================================
#
# Script nay crash gia ETH de position tro nen liquidatable:
#   1. Deploy MockPriceFeed contract
#   2. Update Aave Oracle de dung mock price feed
#   3. Set gia ETH thap hon de HF < 1.0
#
# Yeu cau: 
#   - Anvil dang chay (scripts/start_anvil.ps1)
#   - Da chay setup_liquidation_scenario.ps1
#
# Cach dung:
#   .\scripts\crash_price.ps1 [-PriceDrop 30]  # Drop 30%
#   .\scripts\crash_price.ps1 -Network sepolia -PriceDrop 25
# ============================================================================

param(
    [string]$RpcUrl = "http://127.0.0.1:8545",
    [ValidateSet("auto", "mainnet", "sepolia")]
    [string]$Network = "auto",
    [int]$PriceDrop = 25,  # % price drop, default 25%
    [switch]$SeedAaveEvent  # Optional: emit an extra Aave event after crash
)

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
    ETH_USD_FEED            = "0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419"
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
    ETH_USD_FEED            = "0x694AA1769357215DE4FAC081bf1f309aDC325306"
    NetworkName             = "Sepolia Testnet"
}

# Anvil accounts (Account #2 = Borrower, Account #0 = Deployer for gas)
$BORROWER      = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"
$BORROWER_KEY  = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"
$DEPLOYER      = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
$DEPLOYER_KEY  = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"

# ============================================================================
# HELPER FUNCTIONS
# ============================================================================

function Invoke-Cast {
    param([string]$CastArgs)
    $cmd = "cast $CastArgs --rpc-url $RpcUrl"
    $result = Invoke-Expression $cmd 2>&1
    return ($result | Out-String).Trim()
}

function Write-Step {
    param([string]$Step, [string]$Description)
    Write-Host ""
    Write-Host "----------------------------------------" -ForegroundColor Cyan
    Write-Host "  STEP $Step : $Description" -ForegroundColor Cyan
    Write-Host "----------------------------------------" -ForegroundColor Cyan
}

function Parse-HexOrDecimal {
    param([string]$Value)
    if ([string]::IsNullOrWhiteSpace($Value)) {
        return $null
    }
    # Strip cast annotations like [1.847e11]
    $Value = ($Value -replace '\[.*?\]', '').Trim()
    try {
        if ($Value -match "^0x[a-fA-F0-9]+$") {
            # Hex value
            return [Convert]::ToInt64($Value, 16)
        } elseif ($Value -match "^-?\d+$") {
            # Decimal
            return [decimal]$Value
        } else {
            return $null
        }
    } catch {
        return $null
    }
}

function Parse-CastValues {
    param([string]$RawData)
    # Strip annotations and split multi-value output into array
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
        
        # Health Factor: uint256.max means no debt (infinity)
        $hfRaw = $values[5]
        if ($hfRaw.Length -gt 30) {
            # uint256.max has 78 digits - treat as infinity
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

# ============================================================================
# KIEM TRA CONNECTION & NETWORK DETECTION
# ============================================================================

Write-Host "============================================" -ForegroundColor Red
Write-Host "  CRASH ETH PRICE - LIQUIDATION TRIGGER" -ForegroundColor Red
Write-Host "============================================" -ForegroundColor Red
Write-Host ""
Write-Host "  Price Drop: $PriceDrop%" -ForegroundColor Yellow

# Lay chain ID de xac dinh network
$chainIdRaw = Invoke-Expression "cast chain-id --rpc-url $RpcUrl" 2>&1
$chainId = ($chainIdRaw | Out-String).Trim()

if ([string]::IsNullOrEmpty($chainId) -or $chainId -match "error") {
    Write-Host "  [X] Khong the ket noi Anvil!" -ForegroundColor Red
    exit 1
}

# Auto-detect or validate network
if ($Network -eq "auto") {
    if ($chainId -eq "1") {
        $Network = "mainnet"
    } elseif ($chainId -eq "11155111") {
        $Network = "sepolia"
    } else {
        Write-Host "  [!] Unknown chain ID: $chainId - defaulting to mainnet config" -ForegroundColor Yellow
        $Network = "mainnet"
    }
}

# Load network config
if ($Network -eq "sepolia") {
    $CONFIG = $SEPOLIA_CONFIG
} else {
    $CONFIG = $MAINNET_CONFIG
}

# Set variables from config
$AAVE_POOL              = $CONFIG.AAVE_POOL
$AAVE_ORACLE            = $CONFIG.AAVE_ORACLE
$POOL_ADDRESSES_PROVIDER = $CONFIG.POOL_ADDRESSES_PROVIDER
$ACL_MANAGER            = $CONFIG.ACL_MANAGER
$WETH                   = $CONFIG.WETH
$USDC                   = $CONFIG.USDC
$ETH_USD_FEED           = $CONFIG.ETH_USD_FEED
$NetworkName            = $CONFIG.NetworkName

Write-Host "  [OK] Connected to $NetworkName (Chain ID: $chainId)" -ForegroundColor Green
Write-Host "  [i] ETH/USD Feed: $ETH_USD_FEED" -ForegroundColor Gray

# ============================================================================
# STEP 1: Lay gia ETH hien tai
# ============================================================================
Write-Step "1/5" "Lay gia ETH hien tai"

$ethPriceRaw = Invoke-Cast "call $ETH_USD_FEED `"latestAnswer()(int256)`""
Write-Host "  [DEBUG] Raw output: '$ethPriceRaw'" -ForegroundColor Gray

$ethPrice = Parse-HexOrDecimal $ethPriceRaw

# Fallback: thu latestRoundData neu latestAnswer khong tra ve gia
if ($null -eq $ethPrice -or $ethPrice -eq 0) {
    Write-Host "  [!] latestAnswer tra ve null/0, thu latestRoundData..." -ForegroundColor Yellow
    $roundData = Invoke-Cast "call $ETH_USD_FEED `"latestRoundData()(uint80,int256,uint256,uint256,uint80)`""
    Write-Host "  [DEBUG] Round data: $roundData" -ForegroundColor Gray
    
    # Parse: co the o dang newline-separated hoac space-separated voi annotations
    $cleanedRound = $roundData -replace '\[.*?\]', ''
    $roundValues = ($cleanedRound.Trim() -split '\s+') | Where-Object { $_ -ne '' }
    if ($roundValues.Count -ge 2) {
        $ethPrice = Parse-HexOrDecimal $roundValues[1]
    }
}

if ($null -eq $ethPrice -or $ethPrice -eq 0) {
    Write-Host "  [X] Khong doc duoc gia ETH!" -ForegroundColor Red
    Write-Host "      Kiem tra Anvil da fork mainnet dung chua." -ForegroundColor Yellow
    Write-Host "      Chay: anvil --fork-url https://eth-mainnet.g.alchemy.com/v2/YOUR_KEY" -ForegroundColor Yellow
    exit 1
}

$ethPriceUSD = [math]::Round($ethPrice / 1e8, 2)
Write-Host "  [`$] ETH/USD hien tai: `$$ethPriceUSD" -ForegroundColor Green
Write-Host "  [*] Raw price (8 decimals): $ethPrice" -ForegroundColor Gray

# Tinh gia moi sau khi drop
$newPrice = [math]::Round($ethPrice * (100 - $PriceDrop) / 100)
$newPriceUSD = [math]::Round($newPrice / 1e8, 2)
Write-Host "  [CRASH] Gia moi sau khi crash $PriceDrop%: `$$newPriceUSD" -ForegroundColor Red

# ============================================================================
# STEP 2: Kiem tra HF truoc khi crash
# ============================================================================
Write-Step "2/5" "Kiem tra Health Factor truoc"

$accountData = Invoke-Cast "call $AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $BORROWER"
Write-Host "  [i] Borrower Account before crash:" -ForegroundColor Gray
Write-AccountData $accountData

# Parse HF (thu 6 trong array)
$acctValues = Parse-CastValues $accountData
$hfBefore = "N/A"
if ($acctValues.Count -ge 6) {
    $hfRaw = $acctValues[5]
    if ($hfRaw.Length -gt 30) {
        $hfBefore = "Infinity"
    } else {
        $hfBefore = [math]::Round([decimal]$hfRaw / 1e18, 4)
    }
}

# ============================================================================
# STEP 3: Get WETH Price Source from Aave Oracle & Replace Code
# ============================================================================
Write-Step "3/5" "Get WETH Price Source va Replace Code"

# Get the actual price source Aave uses for WETH
$wethSourceRaw = Invoke-Cast "call $AAVE_ORACLE `"getSourceOfAsset(address)(address)`" $WETH"
$WETH_PRICE_SOURCE = ($wethSourceRaw -replace '\[.*?\]', '').Trim()

Write-Host "  [i] Aave WETH Price Source: $WETH_PRICE_SOURCE" -ForegroundColor Cyan

# Load MockPriceFeed bytecode from compiled output
$mockJsonPath = "out\MockPriceFeed.sol\MockPriceFeed.json"
if (-not (Test-Path $mockJsonPath)) {
    Write-Host "  [!] MockPriceFeed chua compile, dang compile..." -ForegroundColor Yellow
    $null = Invoke-Expression "forge build contracts/MockPriceFeed.sol 2>&1"
}

if (Test-Path $mockJsonPath) {
    $mockJson = Get-Content $mockJsonPath | ConvertFrom-Json
    $deployedBytecode = $mockJson.deployedBytecode.object
    
    Write-Host "  [>] Replacing $WETH_PRICE_SOURCE code with MockPriceFeed..." -ForegroundColor Gray
    Invoke-Cast "rpc anvil_setCode $WETH_PRICE_SOURCE $deployedBytecode"
    Write-Host "  [OK] Contract code replaced" -ForegroundColor Green

    if ($ETH_USD_FEED -ne $WETH_PRICE_SOURCE) {
        Write-Host "  [>] Replacing $ETH_USD_FEED code with MockPriceFeed (for bot oracle worker)..." -ForegroundColor Gray
        Invoke-Cast "rpc anvil_setCode $ETH_USD_FEED $deployedBytecode"
        Write-Host "  [OK] ETH/USD feed code replaced" -ForegroundColor Green
    }
} else {
    Write-Host "  [X] Khong tim thay MockPriceFeed bytecode!" -ForegroundColor Red
    exit 1
}

# ============================================================================
# STEP 4: Set Crashed Price via Storage Manipulation
# ============================================================================
Write-Step "4/5" "Set Crashed Price truc tiep"

# MockPriceFeed storage layout:
# Slot 0: _answer (int256)
# Slot 1: _decimals (uint8)  
# Slot 2: _roundId (uint80)

$newPriceHex = "0x" + ([Convert]::ToString([long]$newPrice, 16)).PadLeft(64, '0')

$targetFeeds = @($WETH_PRICE_SOURCE)
if ($ETH_USD_FEED -ne $WETH_PRICE_SOURCE) {
    $targetFeeds += $ETH_USD_FEED
}

# MockPriceFeed storage layout (Solidity):
# slot 0: _answer (int256)
# slot 1: _decimals (uint8)
# slot 2: _description (string) 
# slot 3: _version (uint256)
# slot 4: _roundId (uint80)
# slot 5: _updatedAt (uint256) ← IMPORTANT: Oracle checks if stale!

# Set slot 5: _updatedAt = block.timestamp (current) so Oracle doesn't mark it STALE
# Get current block via RPC
$blockNum = Invoke-Expression "cast block-number --rpc-url $RpcUrl" 2>&1
$blockData = Invoke-Expression "cast block $blockNum --json --rpc-url $RpcUrl" 2>&1 | ConvertFrom-Json
$currentTimestamp = [Convert]::ToInt64($blockData.timestamp, 16)
$timestampHex = "0x" + $currentTimestamp.ToString("X").PadLeft(64, '0')

foreach ($feed in $targetFeeds) {
    Write-Host "  [>] Setting storage slots on $feed..." -ForegroundColor Gray

    # Set slot 0: _answer = newPrice
    Invoke-Cast "rpc anvil_setStorageAt $feed `"0x0000000000000000000000000000000000000000000000000000000000000000`" $newPriceHex"
    Write-Host "  [OK] Slot 0 (_answer): $newPrice" -ForegroundColor Green

    # Set slot 1: _decimals = 8
    Invoke-Cast "rpc anvil_setStorageAt $feed `"0x0000000000000000000000000000000000000000000000000000000000000001`" `"0x0000000000000000000000000000000000000000000000000000000000000008`""
    Write-Host "  [OK] Slot 1 (_decimals): 8" -ForegroundColor Green

    # Set slot 4: _roundId = 1 (increment)
    Invoke-Cast "rpc anvil_setStorageAt $feed `"0x0000000000000000000000000000000000000000000000000000000000000004`" `"0x0000000000000000000000000000000000000000000000000000000000000001`""
    Write-Host "  [OK] Slot 4 (_roundId): 1" -ForegroundColor Green

    # Set slot 5: _updatedAt = current block timestamp
    Invoke-Cast "rpc anvil_setStorageAt $feed `"0x0000000000000000000000000000000000000000000000000000000000000005`" $timestampHex"
    Write-Host "  [OK] Slot 5 (_updatedAt): $currentTimestamp (block timestamp)" -ForegroundColor Green
}

# ============================================================================
# STEP 4.5: MINE NEW BLOCK (ensure state is reflected on RPC)
# ============================================================================
Write-Host ""
Write-Host "  [>] Mining new block to ensure price change is reflected..." -ForegroundColor Gray
Invoke-Cast "rpc anvil_mine 1"
Write-Host "  [OK] Block mined" -ForegroundColor Green

# ============================================================================
# STEP 5: Verify crash va HF < 1.0
# ============================================================================
Write-Step "5/5" "Verify gia moi va Health Factor"

Start-Sleep -Seconds 1

# Check new price from WETH source (the one Aave actually uses)
$newEthPriceRaw = Invoke-Cast "call $WETH_PRICE_SOURCE `"latestAnswer()(int256)`""
$newEthPriceActual = Parse-HexOrDecimal $newEthPriceRaw

if ($null -ne $newEthPriceActual -and $newEthPriceActual -gt 0) {
    $newEthPriceActualUSD = [math]::Round($newEthPriceActual / 1e8, 2)
    Write-Host "  [`$] ETH/USD SAU CRASH: `$$newEthPriceActualUSD" -ForegroundColor Red
    
    if ($newEthPriceActual -eq $newPrice) {
        Write-Host "  [OK] Gia da duoc cap nhat thanh cong!" -ForegroundColor Green
    } else {
        Write-Host "  [!] Gia khac expected: expected=$newPrice, actual=$newEthPriceActual" -ForegroundColor Yellow
    }
} else {
    Write-Host "  [X] Khong doc duoc gia moi" -ForegroundColor Red
}

# Also verify ETH/USD feed used by Oracle worker
$workerEthPriceRaw = Invoke-Cast "call $ETH_USD_FEED `"latestAnswer()(int256)`""
$workerEthPrice = Parse-HexOrDecimal $workerEthPriceRaw
if ($null -ne $workerEthPrice -and $workerEthPrice -gt 0) {
    $workerEthPriceUsd = [math]::Round($workerEthPrice / 1e8, 2)
    Write-Host "  [i] ETH/USD feed for bot worker: `$$workerEthPriceUsd" -ForegroundColor Cyan
}

# Check new HF
$accountData = Invoke-Cast "call $AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $BORROWER"
Write-Host "  [i] Borrower Account AFTER crash:" -ForegroundColor Gray
Write-AccountData $accountData

# Parse HF
$acctValues = Parse-CastValues $accountData
$hfAfter = "N/A"
if ($acctValues.Count -ge 6) {
    $hfRaw = $acctValues[5]
    if ($hfRaw.Length -gt 30) {
        $hfAfter = "Infinity"
    } else {
        $hfAfter = [math]::Round([decimal]$hfRaw / 1e18, 4)
        
        if ($hfAfter -lt 1.0) {
            Write-Host ""
            Write-Host "  !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!" -ForegroundColor Red
            Write-Host "  !  POSITION IS NOW LIQUIDATABLE   !" -ForegroundColor Red
            Write-Host "  !  Health Factor: $hfAfter" -ForegroundColor Red
            Write-Host "  !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!" -ForegroundColor Red
        } else {
            Write-Host ""
            Write-Host "  [!] Health Factor van >= 1.0: $hfAfter" -ForegroundColor Yellow
            Write-Host "      Thu tang PriceDrop: -PriceDrop 40" -ForegroundColor Yellow
        }
    }
}

# ============================================================================
# STEP 5.5: OPTIONAL AAVE EVENT TRIGGER
# ============================================================================
if ($SeedAaveEvent) {
    Write-Host ""
    Write-Host "  [>] Triggering optional Aave event..." -ForegroundColor Gray
    Write-Host "     (Disabled by default to avoid changing borrower HF after crash)" -ForegroundColor Gray

    # Use a tiny repay event instead of extra supply to avoid artificially increasing collateral.
    try {
        $seedRepayAmount = "1000" # 0.001 USDC (6 decimals)
        $txOutput = Invoke-Cast "send $AAVE_POOL `"repay(address,uint256,uint256,address)`" $USDC $seedRepayAmount 2 $BORROWER --private-key $BORROWER_KEY"

        if ($txOutput -match "0x[a-fA-F0-9]{64}") {
            $txHash = $Matches[0]
            Write-Host "  [OK] Repay event emitted (tx: $($txHash.Substring(0, 10))...)" -ForegroundColor Green
        } else {
            Write-Host "  [!] Repay tx output khong hop le: $txOutput" -ForegroundColor Yellow
        }
        Start-Sleep -Seconds 1
    } catch {
        Write-Host "  [!] Optional event trigger failed (non-critical): $_" -ForegroundColor Yellow
    }
} else {
    Write-Host ""
    Write-Host "  [i] Skip optional Aave seed event (use -SeedAaveEvent to enable)" -ForegroundColor Gray
}

# ============================================================================
# SUMMARY
# ============================================================================
Write-Host ""
Write-Host "============================================" -ForegroundColor Green
Write-Host "  PRICE CRASH COMPLETE" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green
Write-Host ""
Write-Host "  [*] Ket qua:" -ForegroundColor Cyan
Write-Host "     ETH Before:  `$$ethPriceUSD" -ForegroundColor Gray
Write-Host "     ETH After:   `$$newPriceUSD (target)" -ForegroundColor Red
Write-Host "     Drop:        $PriceDrop%" -ForegroundColor Gray
Write-Host ""
Write-Host "     HF Before:   $hfBefore" -ForegroundColor Gray
Write-Host "     HF After:    $hfAfter" -ForegroundColor $(if ($hfAfter -ne "N/A" -and $hfAfter -lt 1.0) { "Red" } else { "Yellow" })
Write-Host ""
Write-Host "  --> Buoc tiep theo:" -ForegroundColor Yellow
Write-Host "     cargo test --test executor_integration -- --nocapture" -ForegroundColor Yellow
Write-Host ""
Write-Host "  [i] De reset gia:" -ForegroundColor Gray
Write-Host "     cast send $ETH_USD_FEED `"setAnswer(int256)`" $ethPrice --private-key $DEPLOYER_KEY --rpc-url $RpcUrl" -ForegroundColor Gray

# ============================================================================
# HELPER: Tao them vai position liquidatable
# ============================================================================

function Create-AdditionalPosition {
    param(
        [string]$AccountAddress,
        [string]$AccountKey,
        [decimal]$WethAmount,
        [decimal]$BorrowPercent = 90
    )
    
    Write-Host ""
    Write-Host "  [+] Creating additional position for $AccountAddress" -ForegroundColor Cyan
    
    # Wrap ETH
    Invoke-Cast "send $WETH `"deposit()`" --value ${WethAmount}ether --private-key $AccountKey"
    
    # Approve
    $maxApproval = "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
    Invoke-Cast "send $WETH `"approve(address,uint256)`" $AAVE_POOL $maxApproval --private-key $AccountKey"
    
    # Supply
    $supplyAmountWei = [math]::Floor($WethAmount * 1e18)
    Invoke-Cast "send $AAVE_POOL `"supply(address,uint256,address,uint16)`" $WETH $supplyAmountWei $AccountAddress 0 --private-key $AccountKey"
    
    # Calculate borrow amount
    $collateralUSD = $WethAmount * $newEthPriceUSD
    $maxBorrowUSD = $collateralUSD * 0.8  # 80% LTV
    $borrowUSD = $maxBorrowUSD * $BorrowPercent / 100
    $borrowAmount = [math]::Floor($borrowUSD * 1e6)  # USDC 6 decimals
    
    # Borrow
    Invoke-Cast "send $AAVE_POOL `"borrow(address,uint256,uint256,uint16,address)`" $USDC $borrowAmount 2 0 $AccountAddress --private-key $AccountKey"
    
    Write-Host "  [OK] Created position: ${WethAmount} WETH, ~`$$([math]::Round($borrowUSD)) USDC debt" -ForegroundColor Green
}

# Function Create-AdditionalPosition co san de su dung trong script khac
# (Da xoa Export-ModuleMember vi day khong phai module .psm1)
