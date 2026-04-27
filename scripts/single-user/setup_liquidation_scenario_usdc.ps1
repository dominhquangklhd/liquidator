# ============================================================================
# SETUP LIQUIDATION SCENARIO (USDC COLLATERAL)
# ============================================================================
#
# Script nay tao kich ban liquidation KHONG phu thuoc WETH collateral:
#   1. Seed USDC cho Borrower (storage manipulation)
#   2. Supply USDC lam collateral tren Aave
#   3. Borrow WBTC de day HF sat 1.0
#   4. Chuyen 1 phan WBTC cho Liquidator + approve
#
# Yeu cau: Hardhat dang chay (scripts/start_hardhat.ps1)
#
# Cach dung:
#   .\scripts\single-user\setup_liquidation_scenario_usdc.ps1
#   .\scripts\single-user\setup_liquidation_scenario_usdc.ps1 -Network mainnet
#   .\scripts\single-user\setup_liquidation_scenario_usdc.ps1 -SeedBorrowerUsdc 600000
# ============================================================================

param(
	[string]$RpcUrl = "http://127.0.0.1:8545",
	[ValidateSet("auto", "mainnet", "sepolia")]
	[string]$Network = "auto",
	[int]$SeedBorrowerUsdc = 800000
)

# ============================================================================
# NETWORK CONFIGURATION
# ============================================================================

# Mainnet addresses (Chain ID: 1)
$MAINNET_CONFIG = @{
	AAVE_POOL               = "0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2"
	AAVE_ORACLE             = "0x54586bE62E3c3580375aE3723C145253060Ca0C2"
	USDC                    = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"
	WBTC                    = "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599"
	aUSDC                   = "0x98C23E9d8f34FEFb1B7BD6a91B7FF122F4e16F5c"
	USDC_BALANCE_SLOT       = 9    # Mainnet USDC balanceOf mapping slot
	NetworkName             = "Ethereum Mainnet"
}

# Sepolia addresses (Chain ID: 11155111)
$SEPOLIA_CONFIG = @{
	AAVE_POOL               = "0x6Ae43d3271ff6888e7Fc43Fd7321a503ff738951"
	AAVE_ORACLE             = "0x2da88497588bf89281816106C7259e31AF45a663"
	USDC                    = "0x94a9D9AC8a22534E3FaCa9F4e7F2E2cf85d5E4C8"
	WBTC                    = "0x29f2D40B0605204364af54EC677bD022dA425d03"
	aUSDC                   = "0x16da4541aD1807f4443d92D26044C1147406EB80"
	USDC_BALANCE_SLOT       = 0    # Sepolia USDC balanceOf mapping slot
	NetworkName             = "Sepolia Testnet"
}

# Hardhat default accounts
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
	Write-Host ""
	Write-Host "----------------------------------------" -ForegroundColor Cyan
	Write-Host "  STEP $Step : $Description" -ForegroundColor Cyan
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

# ============================================================================
# PREREQUISITES + NETWORK DETECTION
# ============================================================================

Write-Host "============================================" -ForegroundColor Green
Write-Host "  SETUP LIQUIDATION SCENARIO (USDC COLL)" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green

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

if ($Network -eq "auto") {
	if ($chainId -eq "1") {
		$Network = "mainnet"
	} elseif ($chainId -eq "11155111") {
		$Network = "sepolia"
	} elseif ($chainId -eq "31337") {
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

$AAVE_POOL         = $CONFIG.AAVE_POOL
$AAVE_ORACLE       = $CONFIG.AAVE_ORACLE
$USDC              = $CONFIG.USDC
$WBTC              = $CONFIG.WBTC
$aUSDC             = $CONFIG.aUSDC
$USDC_BALANCE_SLOT = $CONFIG.USDC_BALANCE_SLOT
$NetworkName       = $CONFIG.NetworkName

Write-Host "[OK] Connected to $NetworkName (Chain ID: $chainId)" -ForegroundColor Green
Write-Host "[i] Aave Pool: $AAVE_POOL" -ForegroundColor Gray

# ============================================================================
# STEP 0: Check current market and pool context
# ============================================================================
Write-Step "0/7" "Kiem tra context truoc khi setup"

$poolUsdcRaw = Invoke-CastCall "$USDC `"balanceOf(address)(uint256)`" $aUSDC"
$poolUsdc = [decimal](Strip-CastAnnotation $poolUsdcRaw)
$poolUsdcDisplay = [math]::Round($poolUsdc / 1e6, 2)
Write-Host "  [i] USDC liquidity trong pool: $poolUsdcDisplay USDC" -ForegroundColor Gray

$wbtcPriceRaw = Invoke-CastCall "$AAVE_ORACLE `"getAssetPrice(address)(uint256)`" $WBTC"
$wbtcPrice = [decimal](Strip-CastAnnotation $wbtcPriceRaw)
$wbtcUsd = [math]::Round($wbtcPrice / 1e8, 2)
Write-Host "  [i] WBTC price (Aave Oracle): `$$wbtcUsd" -ForegroundColor Gray

# ============================================================================
# STEP 1: Seed borrower USDC via storage
# ============================================================================
Write-Step "1/7" "Seed USDC cho Borrower"

$seedBorrowerRaw = [decimal]$SeedBorrowerUsdc * 1e6
$seedBorrowerHex = To-WordHex $seedBorrowerRaw
$borrowerBalanceSlot = Invoke-Expression "cast index address $BORROWER $USDC_BALANCE_SLOT" 2>&1
$borrowerBalanceSlot = ($borrowerBalanceSlot | Out-String).Trim()

Write-Host "  [>] hardhat_setStorageAt USDC slot=$USDC_BALANCE_SLOT..." -ForegroundColor Gray
$null = Invoke-CastRpc "hardhat_setStorageAt $USDC $borrowerBalanceSlot $seedBorrowerHex"

$borrowerUsdcRaw = Invoke-CastCall "$USDC `"balanceOf(address)(uint256)`" $BORROWER"
$borrowerUsdcVal = [decimal](Strip-CastAnnotation $borrowerUsdcRaw)
$borrowerUsdcDisplay = [math]::Round($borrowerUsdcVal / 1e6, 2)

if ($borrowerUsdcVal -le 0) {
	Write-Host "  [X] Khong set duoc USDC cho Borrower!" -ForegroundColor Red
	exit 1
}

Write-Host "  [OK] Borrower USDC: $borrowerUsdcDisplay" -ForegroundColor Green

# ============================================================================
# STEP 2: Approve + Supply USDC collateral
# ============================================================================
Write-Step "2/7" "Approve + Supply USDC collateral"

$maxApproval = "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
$result = Invoke-CastSend "$USDC `"approve(address,uint256)`" $AAVE_POOL $maxApproval --private-key $BORROWER_KEY"
if ($null -eq $result) { Write-Host "  [X] Approve USDC that bai!" -ForegroundColor Red; exit 1 }
Write-Host "  [OK] USDC approved" -ForegroundColor Green

# Supply 75% seeded balance de con du buffer token wallet
$supplyUsdcRaw = [math]::Floor($borrowerUsdcVal * 0.75)
if ($supplyUsdcRaw -lt 1000000) {
	Write-Host "  [X] Supply amount qua nho (<1 USDC)!" -ForegroundColor Red
	exit 1
}
$supplyUsdcDisplay = [math]::Round([decimal]$supplyUsdcRaw / 1e6, 2)

Write-Host "  [>] Supplying $supplyUsdcDisplay USDC..." -ForegroundColor Gray
$result = Invoke-CastSend "$AAVE_POOL `"supply(address,uint256,address,uint16)`" $USDC $supplyUsdcRaw $BORROWER 0 --private-key $BORROWER_KEY"
if ($null -eq $result) { Write-Host "  [X] Supply USDC that bai!" -ForegroundColor Red; exit 1 }
Write-Host "  [OK] Supplied $supplyUsdcDisplay USDC" -ForegroundColor Green

$result = Invoke-CastSend "$AAVE_POOL `"setUserUseReserveAsCollateral(address,bool)`" $USDC true --private-key $BORROWER_KEY"
if ($null -eq $result) {
	Write-Host "  [!] setCollateral failed (co the da enable)" -ForegroundColor Yellow
}
Write-Host "  [OK] USDC enabled as collateral" -ForegroundColor Green

$accountData = Invoke-CastCall "$AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $BORROWER"
Write-Host "  [i] Account Data sau supply:" -ForegroundColor Gray
Write-AccountData $accountData

# ============================================================================
# STEP 3: Borrow WBTC de day HF sat 1.0
# ============================================================================
Write-Step "3/7" "Borrow WBTC"

$acctValues = Parse-CastValues $accountData
if ($acctValues.Count -lt 3) {
	Write-Host "  [X] Khong parse duoc getUserAccountData" -ForegroundColor Red
	exit 1
}

$availableBorrowsBase = [decimal]$acctValues[2]  # USD base, 8 decimals
$initialBorrowWbtcRaw = [math]::Floor(($availableBorrowsBase * 0.97 * 1e8) / $wbtcPrice)
if ($initialBorrowWbtcRaw -lt 1000) {
	Write-Host "  [X] Borrow capacity qua thap cho WBTC" -ForegroundColor Red
	exit 1
}

$totalBorrowedWbtcRaw = 0
$borrowWbtcDisplay = [math]::Round([decimal]$initialBorrowWbtcRaw / 1e8, 6)
Write-Host "  [>] Borrowing initial: $borrowWbtcDisplay WBTC" -ForegroundColor Gray

$result = Invoke-CastSend "$AAVE_POOL `"borrow(address,uint256,uint256,uint16,address)`" $WBTC $initialBorrowWbtcRaw 2 0 $BORROWER --private-key $BORROWER_KEY"
if ($null -eq $result) {
	Write-Host "  [!] Borrow initial failed, thu 50% amount..." -ForegroundColor Yellow
	$initialBorrowWbtcRaw = [math]::Floor($initialBorrowWbtcRaw * 0.5)
	$borrowWbtcDisplay = [math]::Round([decimal]$initialBorrowWbtcRaw / 1e8, 6)
	$result = Invoke-CastSend "$AAVE_POOL `"borrow(address,uint256,uint256,uint16,address)`" $WBTC $initialBorrowWbtcRaw 2 0 $BORROWER --private-key $BORROWER_KEY"
	if ($null -eq $result) { Write-Host "  [X] Borrow WBTC van that bai!" -ForegroundColor Red; exit 1 }
}

$totalBorrowedWbtcRaw += $initialBorrowWbtcRaw
$accountData = Invoke-CastCall "$AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $BORROWER"
Write-Host "  [OK] Borrowed initial WBTC" -ForegroundColor Green
Write-Host "  [i] Account Data sau borrow:" -ForegroundColor Gray
Write-AccountData $accountData

# Borrow loop: day HF gan 1.0
for ($i = 1; $i -le 4; $i++) {
	$currentHF = Get-HealthFactor $accountData
	if ($currentHF -lt 1.06) {
		Write-Host "  [OK] HF da sat 1.0 ($currentHF)" -ForegroundColor Green
		break
	}

	$values = Parse-CastValues $accountData
	if ($values.Count -lt 3) { break }

	$availBase = [decimal]$values[2]
	$extraBorrowRaw = [math]::Floor(($availBase * 0.98 * 1e8) / $wbtcPrice)
	if ($extraBorrowRaw -lt 5000) {
		Write-Host "  [i] Khong con du margin de vay them." -ForegroundColor Gray
		break
	}

	$extraBorrowDisplay = [math]::Round([decimal]$extraBorrowRaw / 1e8, 6)
	Write-Host "  [>] Extra borrow #${i}: $extraBorrowDisplay WBTC" -ForegroundColor Gray
	$result = Invoke-CastSend "$AAVE_POOL `"borrow(address,uint256,uint256,uint16,address)`" $WBTC $extraBorrowRaw 2 0 $BORROWER --private-key $BORROWER_KEY"
	if ($null -eq $result) {
		Write-Host "  [!] Extra borrow that bai, dung." -ForegroundColor Yellow
		break
	}

	$totalBorrowedWbtcRaw += $extraBorrowRaw
	$accountData = Invoke-CastCall "$AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $BORROWER"
	$newHF = Get-HealthFactor $accountData
	Write-Host "  [i] HF hien tai: $newHF" -ForegroundColor Yellow
}

$totalBorrowedWbtcDisplay = [math]::Round([decimal]$totalBorrowedWbtcRaw / 1e8, 6)
Write-Host "  [i] Tong WBTC da vay: $totalBorrowedWbtcDisplay WBTC" -ForegroundColor Gray

# ============================================================================
# STEP 4: Neu HF con cao, rut bot USDC collateral de day xuong ~1.03
# ============================================================================
$finalHF = Get-HealthFactor $accountData
if ($finalHF -gt 1.10) {
	Write-Step "4/7" "Rut bot USDC collateral de day HF xuong ~1.03"

	$targetHF = 1.03
	for ($w = 1; $w -le 6; $w++) {
		$vals = Parse-CastValues $accountData
		if ($vals.Count -lt 6) { break }

		$curCollateral8 = [decimal]$vals[0]
		$curDebt8 = [decimal]$vals[1]
		$curLiqThreshold = [decimal]$vals[3]
		$curHF = Get-HealthFactor $accountData

		if ($curHF -le 1.07) {
			Write-Host "  [OK] HF = $curHF da gan 1.0" -ForegroundColor Green
			break
		}

		if ($curDebt8 -lt 1e6) {
			Write-Host "  [!] Debt qua nho, bo qua withdraw." -ForegroundColor Yellow
			break
		}

		$liqRatio = $curLiqThreshold / 10000
		$targetCollateral8 = $targetHF * $curDebt8 / $liqRatio
		$withdrawAmount8 = $curCollateral8 - $targetCollateral8
		if ($withdrawAmount8 -lt 1e6) {
			Write-Host "  [i] Khong can rut them." -ForegroundColor Gray
			break
		}

		$usdcPriceRaw = Invoke-CastCall "$AAVE_ORACLE `"getAssetPrice(address)(uint256)`" $USDC"
		$usdcPrice8 = [decimal](Strip-CastAnnotation $usdcPriceRaw)

		# withdrawRaw(6 decimals) = usd8 * 1e6 / usdcPrice8
		$withdrawUsdcRaw = [math]::Floor($withdrawAmount8 * 1e6 / $usdcPrice8)
		$withdrawUsdcRaw = [math]::Floor($withdrawUsdcRaw * 0.95)
		if ($withdrawUsdcRaw -lt 10000) {
			Write-Host "  [i] Withdraw amount qua nho, dung." -ForegroundColor Gray
			break
		}

		$withdrawUsdcDisplay = [math]::Round([decimal]$withdrawUsdcRaw / 1e6, 2)
		Write-Host "  [>] Withdraw #${w}: $withdrawUsdcDisplay USDC" -ForegroundColor Gray
		$result = Invoke-CastSend "$AAVE_POOL `"withdraw(address,uint256,address)`" $USDC $withdrawUsdcRaw $BORROWER --private-key $BORROWER_KEY"
		if ($null -eq $result) {
			Write-Host "  [!] Withdraw that bai, dung." -ForegroundColor Yellow
			break
		}

		$accountData = Invoke-CastCall "$AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $BORROWER"
		$curHF2 = Get-HealthFactor $accountData
		Write-Host "  [i] HF sau rut: $curHF2" -ForegroundColor Yellow
	}
}

$accountData = Invoke-CastCall "$AAVE_POOL `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $BORROWER"
$finalHF = Get-HealthFactor $accountData
Write-Host "  [i] Account Data cuoi sau optimize HF:" -ForegroundColor Gray
Write-AccountData $accountData

# ============================================================================
# STEP 5: Fund liquidator bang 1 phan WBTC vua vay
# ============================================================================
Write-Step "5/7" "Chuyen WBTC cho Liquidator + approve"

$liquidatorFundRaw = [math]::Floor($totalBorrowedWbtcRaw * 0.5)
if ($liquidatorFundRaw -lt 1000) {
	Write-Host "  [!] So WBTC borrow qua nho de transfer cho liquidator" -ForegroundColor Yellow
} else {
	$liquidatorFundDisplay = [math]::Round([decimal]$liquidatorFundRaw / 1e8, 6)
	Write-Host "  [>] Transfer $liquidatorFundDisplay WBTC -> Liquidator" -ForegroundColor Gray
	$result = Invoke-CastSend "$WBTC `"transfer(address,uint256)`" $LIQUIDATOR $liquidatorFundRaw --private-key $BORROWER_KEY"
	if ($null -eq $result) {
		Write-Host "  [!] Transfer WBTC that bai - bo qua" -ForegroundColor Yellow
	} else {
		Write-Host "  [OK] Liquidator funded with WBTC" -ForegroundColor Green
	}
}

$result = Invoke-CastSend "$WBTC `"approve(address,uint256)`" $AAVE_POOL $maxApproval --private-key $LIQUIDATOR_KEY"
if ($null -ne $result) {
	Write-Host "  [OK] Liquidator approved WBTC" -ForegroundColor Green
}

$liquidatorWbtcRaw = Invoke-CastCall "$WBTC `"balanceOf(address)(uint256)`" $LIQUIDATOR"
$liquidatorWbtcVal = [decimal](Strip-CastAnnotation $liquidatorWbtcRaw)
$liquidatorWbtcDisplay = [math]::Round($liquidatorWbtcVal / 1e8, 6)
Write-Host "  [i] Liquidator WBTC balance: $liquidatorWbtcDisplay" -ForegroundColor Gray

# ============================================================================
# STEP 6: Snapshot
# ============================================================================
Write-Step "6/7" "Tao snapshot"

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
Write-Step "7/7" "Summary"

$neededDrop = [math]::Round((1 - 1.0 / $finalHF) * 100, 0)
if ($neededDrop -lt 1) { $neededDrop = 1 }

Write-Host ""
Write-Host "============================================" -ForegroundColor Green
Write-Host "  USDC-COLLATERAL SCENARIO READY" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green
Write-Host ""
Write-Host "  [i] Borrower:   $BORROWER" -ForegroundColor Gray
Write-Host "  [i] Collateral: ~$supplyUsdcDisplay USDC" -ForegroundColor Gray
Write-Host "  [i] Debt:       ~$totalBorrowedWbtcDisplay WBTC" -ForegroundColor Gray
Write-Host "  [i] HF:         $finalHF" -ForegroundColor Gray
Write-Host ""
Write-Host "  [i] Liquidator: $LIQUIDATOR" -ForegroundColor Gray
Write-Host "  [i] WBTC:       $liquidatorWbtcDisplay" -ForegroundColor Gray
Write-Host ""
Write-Host "  --> Buoc tiep theo:" -ForegroundColor Yellow
Write-Host "     1. .\scripts\single-user\crash_price_usdc.ps1 -PriceDrop $neededDrop" -ForegroundColor Yellow
Write-Host "     2. cargo test executor -- --nocapture" -ForegroundColor Yellow
Write-Host "     3. cargo run" -ForegroundColor Yellow
