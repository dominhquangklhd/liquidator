# ============================================================================
# SETUP MULTI-TOKEN SCENARIO
# ============================================================================
#
# Kich ban 5 user:
# - User 1: The chap cbETH (khong ho tro), vay USDC
# - User 2: The chap wstETH, vay LUSD (khong ho tro)
# - User 3,4,5: The chap wstETH + WBTC, vay USDC + DAI
#
# Cach dung:
#   .\scripts\multi-users\setup_multi_tokens.ps1
#
# ============================================================================

param(
    [string]$RpcUrl = "http://127.0.0.1:8545"
)

$MAINNET_CONFIG = @{
    AAVE_POOL               = "0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2"
    WSTETH                  = "0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0"
    WBTC                    = "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599"
    CBETH                   = "0xBe9895146f7AF43049ca1c1AE358B0541Ea49704"
    USDC                    = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"
    DAI                     = "0x6B175474E89094C44Da98b954EedeAC495271d0F"
    LUSD                    = "0x5f98805A4E8be255a32880FDeC7F6728C6568bA0"
    USDC_BALANCE_SLOT       = 9
}

$BORROWERS = @(
    @{ Address = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"; Key = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"; Label = "User-1 (cbETH coll)" },
    @{ Address = "0x90F79bf6EB2c4f870365E785982E1f101E93b906"; Key = "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6"; Label = "User-2 (LUSD debt)" },
    @{ Address = "0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65"; Key = "0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a"; Label = "User-3 (Multi)" },
    @{ Address = "0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc"; Key = "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba"; Label = "User-4 (Multi)" },
    @{ Address = "0x976EA74026E726554dB657fA54763abd0C3a0aa9"; Key = "0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e"; Label = "User-5 (Multi)" }
)

$LIQUIDATOR      = "0xFABB0ac9d68B0B445fB7357272Ff202C5651694a"
$LIQUIDATOR_KEY  = "0x8166f546bab6da521a8369cab06c5d2b9e46670292d85c875ee9ec20e84ffb61"
$maxApproval = "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"

function Invoke-CastSend {
    param([string]$CastArgs)
    $cmd = "cast send $CastArgs --rpc-url $RpcUrl"
    $result = Invoke-Expression $cmd 2>&1
    $output = ($result | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        $fallbackCmd = "cast send $CastArgs --rpc-url $RpcUrl --gas-limit 5000000 --legacy"
        $fallbackResult = Invoke-Expression $fallbackCmd 2>&1
        return ($fallbackResult | Out-String).Trim()
    }
    return $output
}

function Invoke-CastCall {
    param([string]$CastArgs)
    $parsed = [regex]::Match($CastArgs, '^(?<to>0x[a-fA-F0-9]{40})\s+"(?<sig>[^"]+)"\s*(?<args>.*)$')
    if (-not $parsed.Success) {
        return (Invoke-Expression "cast call $CastArgs --rpc-url $RpcUrl" 2>&1 | Out-String).Trim()
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
    $rpcPayload = @{ jsonrpc = "2.0"; id = 1; method = "eth_call"; params = @(@{ to = $to; data = $calldata }, "latest") } | ConvertTo-Json -Compress
    $rpcResp = Invoke-RestMethod -Uri $RpcUrl -Method Post -Body $rpcPayload -ContentType "application/json"
    $rawHex = $rpcResp.result
    
    if ($sig -match '\)\(.*\)$' -and $rawHex -ne "0x") {
        return (& cast abi-decode $sig $rawHex 2>&1 | Out-String).Trim()
    }
    return $rawHex
}

function Invoke-CastRpc {
    param([string]$CastArgs)
    return (Invoke-Expression "cast rpc $CastArgs --rpc-url $RpcUrl" 2>&1 | Out-String).Trim()
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

function To-WordHex {
    param([decimal]$RawValue)
    $big = [System.Numerics.BigInteger]::Parse(([math]::Floor($RawValue)).ToString("0"))
    return ("0x" + $big.ToString("x").PadLeft(64, '0'))
}

Write-Host "============================================" -ForegroundColor Green
Write-Host "  SETUP MULTI-TOKEN SCENARIO" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green

$CONFIG = $MAINNET_CONFIG

# STEP 1: Mint tokens via storage
Write-Host "  [>] Minting tokens..." -ForegroundColor Cyan

function Set-Balance($Token, $Slot, $Address, $AmountWei) {
    $idx = Invoke-Expression "cast index address $Address $Slot" 2>&1
    $idx = ($idx | Out-String).Trim()
    $hex = To-WordHex $AmountWei
    Invoke-CastRpc "hardhat_setStorageAt $Token $idx $hex" | Out-Null
}

$ethWei = [System.Numerics.BigInteger]::Parse("100000000000000000000") # 100 ETH
$ethHex = "0x" + $ethWei.ToString("x").TrimStart('0')

foreach ($b in $BORROWERS) {
    Invoke-CastRpc "hardhat_setBalance $($b.Address) $ethHex" | Out-Null
}

# User 1: cbETH
Set-Balance $CONFIG.CBETH 9 $BORROWERS[0].Address 50000000000000000000  # 50 cbETH
# User 2: wstETH
Set-Balance $CONFIG.WSTETH 0 $BORROWERS[1].Address 50000000000000000000  # 50 wstETH
# User 3,4,5: wstETH + WBTC
for ($i=2; $i -lt 5; $i++) {
    Set-Balance $CONFIG.WSTETH 0 $BORROWERS[$i].Address 30000000000000000000  # 30 wstETH
    Set-Balance $CONFIG.WBTC 0 $BORROWERS[$i].Address 200000000  # 2 WBTC
}
Write-Host "  [OK] Tokens minted." -ForegroundColor Green

# STEP 2: Setup User 1 (cbETH -> USDC)
Write-Host "  [>] Setup User 1 (Unsupported Collateral: cbETH)..." -ForegroundColor Cyan
Invoke-CastSend "$($CONFIG.CBETH) `"approve(address,uint256)`" $($CONFIG.AAVE_POOL) $maxApproval --private-key $($BORROWERS[0].Key)" | Out-Null
Invoke-CastSend "$($CONFIG.AAVE_POOL) `"supply(address,uint256,address,uint16)`" $($CONFIG.CBETH) 50000000000000000000 $($BORROWERS[0].Address) 0 --private-key $($BORROWERS[0].Key)" | Out-Null
Invoke-CastSend "$($CONFIG.AAVE_POOL) `"setUserUseReserveAsCollateral(address,bool)`" $($CONFIG.CBETH) true --private-key $($BORROWERS[0].Key)" | Out-Null

$data = Invoke-CastCall "$($CONFIG.AAVE_POOL) `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $($BORROWERS[0].Address)"
$avail = [decimal](Parse-CastValues $data)[2]
$borrowAmt = [math]::Floor(($avail / 100) * 0.92) # USDC (6 dec)
Invoke-CastSend "$($CONFIG.AAVE_POOL) `"borrow(address,uint256,uint256,uint16,address)`" $($CONFIG.USDC) $borrowAmt 2 0 $($BORROWERS[0].Address) --private-key $($BORROWERS[0].Key)" | Out-Null

$data = Invoke-CastCall "$($CONFIG.AAVE_POOL) `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $($BORROWERS[0].Address)"
Write-Host "  [OK] User 1 setup. HF: $(Get-HealthFactor $data)" -ForegroundColor Green

# STEP 3: Setup User 2 (wstETH -> LUSD)
Write-Host "  [>] Setup User 2 (Unsupported Debt: LUSD)..." -ForegroundColor Cyan
Invoke-CastSend "$($CONFIG.WSTETH) `"approve(address,uint256)`" $($CONFIG.AAVE_POOL) $maxApproval --private-key $($BORROWERS[1].Key)" | Out-Null
Invoke-CastSend "$($CONFIG.AAVE_POOL) `"supply(address,uint256,address,uint16)`" $($CONFIG.WSTETH) 50000000000000000000 $($BORROWERS[1].Address) 0 --private-key $($BORROWERS[1].Key)" | Out-Null
Invoke-CastSend "$($CONFIG.AAVE_POOL) `"setUserUseReserveAsCollateral(address,bool)`" $($CONFIG.WSTETH) true --private-key $($BORROWERS[1].Key)" | Out-Null

$data = Invoke-CastCall "$($CONFIG.AAVE_POOL) `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $($BORROWERS[1].Address)"
$avail = [decimal](Parse-CastValues $data)[2]
$borrowAmtLusd = [math]::Floor(($avail / 100) * 0.90 * 1e12) # LUSD has 18 dec, avail is 8 dec USD. So * 1e10 roughly? 
# Wait, let's just borrow a fixed amount or max. Aave's avail is in 8 dec USD. LUSD price is $1. So LUSD wei = avail * 1e10.
$borrowAmtLusd = [math]::Floor($avail * 10000000000 * 0.85).ToString("0")
Invoke-CastSend "$($CONFIG.AAVE_POOL) `"borrow(address,uint256,uint256,uint16,address)`" $($CONFIG.LUSD) $borrowAmtLusd 2 0 $($BORROWERS[1].Address) --private-key $($BORROWERS[1].Key)" | Out-Null

$data = Invoke-CastCall "$($CONFIG.AAVE_POOL) `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $($BORROWERS[1].Address)"
Write-Host "  [OK] User 2 setup. HF: $(Get-HealthFactor $data)" -ForegroundColor Green

# STEP 4: Setup User 3,4,5 (wstETH+WBTC -> USDC+DAI)
Write-Host "  [>] Setup User 3, 4, 5 (Multi collateral/debt)..." -ForegroundColor Cyan
for ($i=2; $i -lt 5; $i++) {
    $b = $BORROWERS[$i]
    # Approve
    Invoke-CastSend "$($CONFIG.WSTETH) `"approve(address,uint256)`" $($CONFIG.AAVE_POOL) $maxApproval --private-key $($b.Key)" | Out-Null
    Invoke-CastSend "$($CONFIG.WBTC) `"approve(address,uint256)`" $($CONFIG.AAVE_POOL) $maxApproval --private-key $($b.Key)" | Out-Null
    
    # Supply
    Invoke-CastSend "$($CONFIG.AAVE_POOL) `"supply(address,uint256,address,uint16)`" $($CONFIG.WSTETH) 30000000000000000000 $($b.Address) 0 --private-key $($b.Key)" | Out-Null
    Invoke-CastSend "$($CONFIG.AAVE_POOL) `"supply(address,uint256,address,uint16)`" $($CONFIG.WBTC) 200000000 $($b.Address) 0 --private-key $($b.Key)" | Out-Null
    
    # Enable collat
    Invoke-CastSend "$($CONFIG.AAVE_POOL) `"setUserUseReserveAsCollateral(address,bool)`" $($CONFIG.WSTETH) true --private-key $($b.Key)" | Out-Null
    Invoke-CastSend "$($CONFIG.AAVE_POOL) `"setUserUseReserveAsCollateral(address,bool)`" $($CONFIG.WBTC) true --private-key $($b.Key)" | Out-Null
    
    # Borrow 1: USDC
    $data = Invoke-CastCall "$($CONFIG.AAVE_POOL) `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $($b.Address)"
    $avail = [decimal](Parse-CastValues $data)[2]
    $borrowUsdc = [math]::Floor(($avail / 100) * 0.45) # 45% in USDC
    Invoke-CastSend "$($CONFIG.AAVE_POOL) `"borrow(address,uint256,uint256,uint16,address)`" $($CONFIG.USDC) $borrowUsdc 2 0 $($b.Address) --private-key $($b.Key)" | Out-Null
    
    # Borrow 2: DAI
    $data = Invoke-CastCall "$($CONFIG.AAVE_POOL) `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $($b.Address)"
    $avail = [decimal](Parse-CastValues $data)[2]
    $borrowDai = [math]::Floor($avail * 10000000000 * 0.90).ToString("0") # 90% of remaining in DAI
    Invoke-CastSend "$($CONFIG.AAVE_POOL) `"borrow(address,uint256,uint256,uint16,address)`" $($CONFIG.DAI) $borrowDai 2 0 $($b.Address) --private-key $($b.Key)" | Out-Null

    $data = Invoke-CastCall "$($CONFIG.AAVE_POOL) `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $($b.Address)"
    Write-Host "  [OK] $($b.Label) setup. HF: $(Get-HealthFactor $data)" -ForegroundColor Green
}

# STEP 5: Setup Liquidator
Set-Balance $CONFIG.USDC 9 $LIQUIDATOR 2000000000000 # 2M USDC
Set-Balance $CONFIG.DAI 2 $LIQUIDATOR 2000000000000000000000000 # 2M DAI
Invoke-CastRpc "hardhat_setBalance $LIQUIDATOR $ethHex" | Out-Null
Invoke-CastSend "$($CONFIG.USDC) `"approve(address,uint256)`" $($CONFIG.AAVE_POOL) $maxApproval --private-key $LIQUIDATOR_KEY" | Out-Null
Invoke-CastSend "$($CONFIG.DAI) `"approve(address,uint256)`" $($CONFIG.AAVE_POOL) $maxApproval --private-key $LIQUIDATOR_KEY" | Out-Null

$snap = Invoke-CastRpc "evm_snapshot"
Write-Host "  [OK] Snapshot ID: $snap" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green
Write-Host "  Run: .\scripts\multi-users\crash_multi_tokens.ps1 -PriceDrop 8" -ForegroundColor Yellow
