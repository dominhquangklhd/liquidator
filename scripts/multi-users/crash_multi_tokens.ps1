# ============================================================================
# CRASH PRICE MULTI-TOKENS (wstETH & WBTC)
# ============================================================================
#
# Script nay crash gia 2 token cung luc (wstETH va WBTC) de day cac user 3,4,5
# vao trang thai liquidatable. Cac user 1,2 co chua token bot khong ho tro
# se bi bot bo qua.
#
# Cach dung:
#   .\scripts\multi-users\crash_multi_tokens.ps1
#   .\scripts\multi-users\crash_multi_tokens.ps1 -PriceDrop 8 -DebtPump 5
#
# ============================================================================

param(
    [string]$RpcUrl = "http://127.0.0.1:8545",
    [int]$PriceDrop = 8,
    [int]$DebtPump = 5
)

$MAINNET_CONFIG = @{
    AAVE_POOL               = "0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2"
    AAVE_ORACLE             = "0x54586bE62E3c3580375aE3723C145253060Ca0C2"
    WSTETH                  = "0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0"
    WBTC                    = "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599"
    USDC                    = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"
    DAI                     = "0x6B175474E89094C44Da98b954EedeAC495271d0F"
}

$DEPLOYER_KEY = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"

$BORROWERS = @(
    @{ Address = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"; Label = "User-1 (cbETH coll)" },
    @{ Address = "0x90F79bf6EB2c4f870365E785982E1f101E93b906"; Label = "User-2 (LUSD debt)" },
    @{ Address = "0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65"; Label = "User-3 (Multi)" },
    @{ Address = "0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc"; Label = "User-4 (Multi)" },
    @{ Address = "0x976EA74026E726554dB657fA54763abd0C3a0aa9"; Label = "User-5 (Multi)" }
)

function Invoke-CastRpc {
    param([string]$CastArgs)
    return (Invoke-Expression "cast rpc $CastArgs --rpc-url $RpcUrl" 2>&1 | Out-String).Trim()
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

function Strip-CastAnnotation {
    param([string]$Value)
    return ($Value -replace '\[.*?\]', '').Trim()
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

function Replace-PriceFeed {
    param([string]$TokenAddr, [string]$TokenName, [int]$ChangePercent)
    
    $sign = if ($ChangePercent -lt 0) { "" } else { "+" }
    Write-Host "  [>] Thay doi gia cho $TokenName ($sign$ChangePercent%)..." -ForegroundColor Cyan
    
    $priceRaw = Invoke-CastCall "$($MAINNET_CONFIG.AAVE_ORACLE) `"getAssetPrice(address)(uint256)`" $TokenAddr"
    $price = try { [decimal](Strip-CastAnnotation $priceRaw) } catch { 0 }
    
    $sourceRaw = Invoke-CastCall "$($MAINNET_CONFIG.AAVE_ORACLE) `"getSourceOfAsset(address)(address)`" $TokenAddr"
    $source = (Strip-CastAnnotation $sourceRaw).Trim()
    
    $newPrice = [long]([math]::Floor($price * (100 + $ChangePercent) / 100))
    $newPriceHex = "0x" + ([Convert]::ToString([long]$newPrice, 16)).PadLeft(64, '0')
    
    # 1. Update bytecode MockPriceFeed
    $mockJsonPath = "out\MockPriceFeed.sol\MockPriceFeed.json"
    if (-not (Test-Path $mockJsonPath)) {
        Invoke-Expression "forge build contracts/MockPriceFeed.sol 2>&1" | Out-Null
    }
    $mockJson = Get-Content $mockJsonPath | ConvertFrom-Json
    $deployedBytecode = $mockJson.deployedBytecode.object
    
    Invoke-CastRpc "hardhat_setCode $source $deployedBytecode" | Out-Null
    
    # 2. Update decimals = 8
    Invoke-CastRpc "hardhat_setStorageAt $source `"0x0000000000000000000000000000000000000000000000000000000000000001`" `"0x0000000000000000000000000000000000000000000000000000000000000008`"" | Out-Null
    
    # 3. setAnswer
    $tx = Invoke-CastSend "$source `"setAnswer(int256)`" $newPrice --private-key $DEPLOYER_KEY"
    if ($tx -match "0x[a-fA-F0-9]{64}") {
        Write-Host "  [OK] $TokenName setAnswer() thanh cong. Gia moi: $([math]::Round($newPrice/1e8, 2)) USD" -ForegroundColor Green
    } else {
        Invoke-CastRpc "hardhat_setStorageAt $source `"0x0000000000000000000000000000000000000000000000000000000000000000`" $newPriceHex" | Out-Null
        Write-Host "  [OK] $TokenName storage fallback. Gia moi: $([math]::Round($newPrice/1e8, 2)) USD" -ForegroundColor Green
    }
}

Write-Host "============================================" -ForegroundColor Green
Write-Host "  CRASH PRICE MULTI-TOKENS (wstETH, WBTC, USDC, DAI) " -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green

Replace-PriceFeed -TokenAddr $MAINNET_CONFIG.WSTETH -TokenName "wstETH" -ChangePercent (-$PriceDrop)
Replace-PriceFeed -TokenAddr $MAINNET_CONFIG.WBTC -TokenName "WBTC" -ChangePercent (-$PriceDrop)
Replace-PriceFeed -TokenAddr $MAINNET_CONFIG.USDC -TokenName "USDC" -ChangePercent $DebtPump
Replace-PriceFeed -TokenAddr $MAINNET_CONFIG.DAI -TokenName "DAI" -ChangePercent $DebtPump

Invoke-CastRpc "evm_mine" | Out-Null

Write-Host "============================================" -ForegroundColor Green
Write-Host "  CRASH COMPLETE" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green

Write-Host "  [*] Kiem tra Health Factor sau khi crash:" -ForegroundColor Cyan
foreach ($user in $BORROWERS) {
    $accountData = Invoke-CastCall "$($MAINNET_CONFIG.AAVE_POOL) `"getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)`" $($user.Address)"
    $hf = Get-HealthFactorFromAccountData $accountData
    $color = if ($hf -ne "N/A" -and $hf -ne "Infinity" -and [decimal]$hf -lt 1.0) { "Red" } else { "Yellow" }
    Write-Host "      $($user.Label): $hf" -ForegroundColor $color
}

Write-Host ""
Write-Host "============================================" -ForegroundColor Green
Write-Host "  --> Chay bot liquidator: cargo run" -ForegroundColor Yellow
