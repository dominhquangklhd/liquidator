# Cast command cheat sheet (Aave v3 + Hardhat fork)

All commands below assume:
- RPC: http://127.0.0.1:8545
- Aave Pool: 0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2
- wstETH:   0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0
- USDC:     0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48
- aUSDC:    0x98C23E9d8f34FEFb1B7BD6a91B7FF122F4e16F5c
- Borrower: 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC
- Borrower key: 0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a

Note: On Hardhat/Anvil forks, `cast call` can fail with a duplicate `data` error. Use the `cast calldata` + `cast rpc eth_call` pattern below.

## Network checks

```bash
cast chain-id --rpc-url http://127.0.0.1:8545
cast rpc eth_accounts --rpc-url http://127.0.0.1:8545
```

## Aave user data + Health Factor (HF)

```bash
calldata=$(cast calldata "getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)" 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC)
cast rpc eth_call "{\"to\":\"0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2\",\"data\":\"$calldata\"}" "latest" --rpc-url http://127.0.0.1:8545
```
- The last value is `healthFactor` (divide by 1e18).

PowerShell helper (same pattern):

```powershell
$calldata = cast calldata "getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)" 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC
cast rpc eth_call "{`"to`":`"0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2`",`"data`":`"$calldata`"}" "latest" --rpc-url http://127.0.0.1:8545
```

## Token balances

```bash
calldata=$(cast calldata "balanceOf(address)(uint256)" 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC)
cast rpc eth_call "{\"to\":\"0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0\",\"data\":\"$calldata\"}" "latest" --rpc-url http://127.0.0.1:8545

calldata=$(cast calldata "balanceOf(address)(uint256)" 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC)
cast rpc eth_call "{\"to\":\"0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48\",\"data\":\"$calldata\"}" "latest" --rpc-url http://127.0.0.1:8545
```

## Pool liquidity (USDC in aUSDC)

```bash
calldata=$(cast calldata "balanceOf(address)(uint256)" 0x98C23E9d8f34FEFb1B7BD6a91B7FF122F4e16F5c)
cast rpc eth_call "{\"to\":\"0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48\",\"data\":\"$calldata\"}" "latest" --rpc-url http://127.0.0.1:8545
```

## Approvals

```bash
cast send 0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0 "approve(address,uint256)" 0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff --private-key 0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a --rpc-url http://127.0.0.1:8545
cast send 0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48 "approve(address,uint256)" 0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff --private-key 0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a --rpc-url http://127.0.0.1:8545
```

## Aave actions (deposit/borrow/repay/withdraw)

```bash
# UserDeposit (supply)
cast send 0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2 "supply(address,uint256,address,uint16)" 0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0 1000000000000000000 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC 0 --private-key 0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a --rpc-url http://127.0.0.1:8545

# Enable collateral
cast send 0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2 "setUserUseReserveAsCollateral(address,bool)" 0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0 true --private-key 0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a --rpc-url http://127.0.0.1:8545

# UserBorrow (USDC)
cast send 0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2 "borrow(address,uint256,uint256,uint16,address)" 0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48 100000000 2 0 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC --private-key 0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a --rpc-url http://127.0.0.1:8545

# UserRepay (USDC)
cast send 0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2 "repay(address,uint256,uint256,address)" 0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48 50000000 2 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC --private-key 0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a --rpc-url http://127.0.0.1:8545

# UserWithdraw (wstETH)
cast send 0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2 "withdraw(address,uint256,address)" 0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0 100000000000000000 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC --private-key 0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a --rpc-url http://127.0.0.1:8545
```

## Hardhat helpers (fork)

```bash
# Snapshot / revert
cast rpc evm_snapshot --rpc-url http://127.0.0.1:8545
cast rpc evm_revert 0x1 --rpc-url http://127.0.0.1:8545

# Set ERC20 balance via storage slot (example: USDC slot 9)
cast index address 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC 9
cast rpc hardhat_setStorageAt 0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48 <slot> <value> --rpc-url http://127.0.0.1:8545

# Set ETH balance
cast rpc hardhat_setBalance 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC 0x3635C9ADC5DEA00000 --rpc-url http://127.0.0.1:8545
```

## Example flow (all 4 Aave events)

```bash
# 1) Approve + supply (UserDeposit)
cast send 0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0 "approve(address,uint256)" 0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff --private-key 0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a --rpc-url http://127.0.0.1:8545
cast send 0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2 "supply(address,uint256,address,uint16)" 0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0 2000000000000000000 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC 0 --private-key 0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a --rpc-url http://127.0.0.1:8545

# 2) Borrow (UserBorrow)
cast send 0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2 "borrow(address,uint256,uint256,uint16,address)" 0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48 100000000 2 0 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC --private-key 0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a --rpc-url http://127.0.0.1:8545

# 3) Repay (UserRepay)
cast send 0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2 "repay(address,uint256,uint256,address)" 0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48 50000000 2 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC --private-key 0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a --rpc-url http://127.0.0.1:8545

# 4) Withdraw (UserWithdraw)
cast send 0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2 "withdraw(address,uint256,address)" 0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0 100000000000000000 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC --private-key 0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a --rpc-url http://127.0.0.1:8545
```
