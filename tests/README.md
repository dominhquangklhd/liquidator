# Testing Guide - Liquidation Bot

## Mục lục
- [Tổng quan](#tổng-quan)
- [Cài đặt công cụ](#cài-đặt-công-cụ)
- [Quick Start](#quick-start)
- [Chi tiết từng bước](#chi-tiết-từng-bước)
- [Cấu trúc test](#cấu-trúc-test)
- [Troubleshooting](#troubleshooting)

---

## Tổng quan

Để test Executor, chúng ta tạo một **mạng riêng (local fork)** từ Ethereum mainnet bằng **Anvil** (Foundry). Cách này cho phép:

1. **Sử dụng contract Aave V3 thật** - Không cần deploy lại
2. **Kiểm soát giá oracle** - Deploy mock oracle, muốn crash bao nhiêu tùy ý
3. **Không tốn gas thật** - Test accounts có 10,000 ETH miễn phí
4. **Snapshot & Rollback** - Quay lại bất kỳ thời điểm nào

```
┌──────────────────────────────────────────────────┐
│                  Anvil (Local Fork)                │
│                                                    │
│  ┌──────────────┐  ┌──────────────┐               │
│  │  Aave V3      │  │ Chainlink    │               │
│  │  Pool         │  │ Oracle       │               │
│  │  (từ mainnet) │  │ (từ mainnet) │               │
│  └──────┬───────┘  └──────┬───────┘               │
│         │                  │                        │
│         │    ┌─────────────┤                        │
│         │    │ MockPriceFeed│ ← CRASH GIÁ TẠI ĐÂY  │
│         │    │ (deploy mới) │                        │
│         │    └─────────────┘                        │
│         │                                           │
│  ┌──────▼───────┐  ┌──────────────┐               │
│  │ Borrower      │  │ Liquidator   │               │
│  │ (Account #0)  │  │ (Account #1) │               │
│  │ 50 WETH coll. │  │ 100k USDC    │               │
│  │ 95k USDC debt │  │              │               │
│  └──────────────┘  └──────────────┘               │
└──────────────────────────────────────────────────┘
                      │
            ┌─────────▼──────────┐
            │   Liquidator Bot   │
            │   (cargo run)      │
            └────────────────────┘
```

---

## Cài đặt công cụ

### 1. Foundry (Anvil + Cast + Forge)

**Windows (dùng WSL hoặc Git Bash):**
```bash
curl -L https://foundry.paradigm.xyz | bash
foundryup
```

**Thêm vào User PATH**
```powershell
[Environment]::SetEnvironmentVariable("Path", $env:Path + ";$env:USERPROFILE\.foundry\bin", "User")
```

**Kiểm tra:**
```powershell
anvil --version  # anvil 0.x.x
cast --version   # cast 0.x.x
forge --version  # forge 0.x.x
```

### 2. RPC URL (Alchemy - Miễn phí)

1. Đăng ký tại [alchemy.com](https://www.alchemy.com)
2. Tạo App → chọn **Ethereum Mainnet**  
3. Copy **HTTPS** URL

```powershell
$env:ETH_RPC_URL = "https://eth-mainnet.g.alchemy.com/v2/YOUR_API_KEY"
```

### 3. Rust toolchain (đã có)
```powershell
cargo --version
```

---

## Quick Start

Mở **5 terminal** PowerShell:

### Terminal 1: Khởi động Anvil
```powershell
$env:ETH_RPC_URL = "https://eth-mainnet.g.alchemy.com/v2/YOUR_KEY"
.\scripts\start_anvil.ps1
```

### Terminal 2: Setup Scenario
```powershell
.\scripts\setup_liquidation_scenario.ps1
```

### Terminal 3: Crash giá
```powershell
# Giảm 30% (mặc định)
.\scripts\crash_price.ps1

# Hoặc giảm 50%
.\scripts\crash_price.ps1 -DropPercent 50

# Hoặc set giá cụ thể
.\scripts\crash_price.ps1 -NewPriceUSD 1500
```

### Terminal 4: Chạy test
```powershell
# Chạy tất cả integration tests
cargo test --test executor_integration -- --nocapture

# Hoặc chạy từng test
cargo test --test executor_integration test_connect_anvil -- --nocapture
cargo test --test executor_integration test_dry_run_liquidation -- --nocapture
cargo test --test executor_integration test_execute_real_liquidation -- --nocapture
```

### Terminal 5: Chạy bot (tuỳ chọn)
```powershell
cargo run
```

---

## Chi tiết từng bước

### Bước 1: Khởi động Anvil Fork

```powershell
# Fork Ethereum mainnet tại block mới nhất
.\scripts\start_anvil.ps1 -RpcUrl "https://eth-mainnet.g.alchemy.com/v2/YOUR_KEY"

# Fork tại block cụ thể (reproducible)
.\scripts\start_anvil.ps1 -RpcUrl "YOUR_URL" -ForkBlock 19000000
```

**Kết quả:** 10 test accounts mỗi account có 10,000 ETH:
| Account | Address | Vai trò |
|---------|---------|---------|
| #0 | 0xf39Fd6...b92266 | Borrower (người vay) |
| #1 | 0x70997...c79C8 | Liquidator (bot của chúng ta) |

### Bước 2: Setup Scenario

Script `setup_liquidation_scenario.ps1` tự động thực hiện:

1. **Wrap ETH → WETH** (100 ETH)
2. **Approve** WETH cho Aave Pool
3. **Supply** 50 WETH vào Aave (collateral)
4. **Borrow** 95,000 USDC (gần max LTV)
5. **Fund Liquidator** với 100,000 USDC (từ Binance whale)
6. **Approve** USDC cho Aave Pool (cho liquidator)
7. **Snapshot** (để có thể rollback)

**Sau khi chạy:**
- Borrower: 50 WETH collateral, 95k USDC debt
- HF ≈ 1.05 - 1.15 (gần ngưỡng liquidation)

### Bước 3: Crash giá ETH

Script `crash_price.ps1` thực hiện:

1. **Deploy MockPriceFeed** với giá ETH thấp
2. **Impersonate** Aave Oracle owner
3. **Update** Aave Oracle để dùng MockPriceFeed
4. **Kết quả:** HF giảm xuống < 1.0 → Position liquidatable!

```powershell
# Ví dụ: ETH = $2500 hiện tại

.\scripts\crash_price.ps1 -DropPercent 20   # ETH → $2000, HF ≈ 0.85
.\scripts\crash_price.ps1 -DropPercent 30   # ETH → $1750, HF ≈ 0.75
.\scripts\crash_price.ps1 -DropPercent 50   # ETH → $1250, HF ≈ 0.53
.\scripts\crash_price.ps1 -NewPriceUSD 1800 # ETH = $1800
```

### Bước 4: Chạy Tests

```powershell
# Test kết nối
cargo test --test executor_integration test_connect_anvil -- --nocapture

# Test đọc data từ Aave
cargo test --test executor_integration test_read_account_data -- --nocapture

# Test đọc giá oracle
cargo test --test executor_integration test_read_oracle_price -- --nocapture

# Test phát hiện liquidation
cargo test --test executor_integration test_detect_liquidatable -- --nocapture

# Test DRY RUN (không gửi TX thật)
cargo test --test executor_integration test_dry_run_liquidation -- --nocapture

# Test SIMULATE (eth_call)
cargo test --test executor_integration test_simulate_liquidation -- --nocapture

# Test THẬT (gửi TX lên Anvil)
cargo test --test executor_integration test_execute_real_liquidation -- --nocapture

# Test Worker loop (chạy 1 giây)
cargo test --test executor_integration test_executor_worker_loop -- --nocapture
```

---

## Cấu trúc Test

```
tests/
  executor_integration.rs    ← 8 integration tests
    ├── test_connect_anvil           - Kiểm tra kết nối
    ├── test_read_account_data       - Đọc Aave position 
    ├── test_read_oracle_price       - Đọc giá từ oracle
    ├── test_detect_liquidatable     - Phát hiện HF < 1.0
    ├── test_dry_run_liquidation     - Dry run (log only)
    ├── test_simulate_liquidation    - Simulate (eth_call)
    ├── test_execute_real_liquidation - Execute thật
    └── test_executor_worker_loop    - Worker integration

scripts/
  start_anvil.ps1            ← Khởi động Anvil fork
  setup_liquidation_scenario.ps1  ← Setup position trên Aave
  crash_price.ps1            ← Crash giá ETH

contracts/
  MockPriceFeed.sol          ← Mock Chainlink oracle
```

---

## Flow Diagram

```
start_anvil.ps1                     Anvil chạy tại :8545
       │                                    │
       ▼                                    │
setup_liquidation_scenario.ps1              │
       │                                    │
       ├── 1. Wrap ETH → WETH              │
       ├── 2. Approve WETH                  │
       ├── 3. Supply 50 WETH to Aave       │
       ├── 4. Borrow 95k USDC              │  
       ├── 5. Fund liquidator USDC         │
       ├── 6. Approve USDC                  │
       └── 7. Snapshot ✓                    │
                                            │
crash_price.ps1                             │
       │                                    │
       ├── 1. Deploy MockPriceFeed          │
       ├── 2. Set Aave Oracle → Mock        │
       └── 3. HF drops below 1.0 🔴        │
                                            │
cargo test                                  │
       │                                    │
       ├── Detect HF < 1.0                 │
       ├── Build liquidation TX             │
       ├── Simulate (eth_call)              │
       ├── Execute (send TX)                │
       └── Verify HF improved ✅            │
```

---

## Thay đổi giá linh hoạt

Sau khi đã deploy MockPriceFeed, bạn có thể thay đổi giá bất kỳ lúc nào:

```powershell
# Set giá ETH = $2000 (giá * 1e8 = 200000000000)
cast send <MOCK_ADDRESS> "setAnswer(int256)" 200000000000 --unlocked --from <OWNER> --rpc-url http://127.0.0.1:8545

# Giảm 20% so với giá hiện tại
cast send <MOCK_ADDRESS> "dropPrice(uint256)" 20 --unlocked --from <OWNER> --rpc-url http://127.0.0.1:8545

# Tăng 50% (phục hồi)
cast send <MOCK_ADDRESS> "raisePrice(uint256)" 50 --unlocked --from <OWNER> --rpc-url http://127.0.0.1:8545

# Kiểm tra giá hiện tại
cast call <MOCK_ADDRESS> "latestAnswer()(int256)" --rpc-url http://127.0.0.1:8545

# Kiểm tra HF borrower
cast call 0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2 "getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)" 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266 --rpc-url http://127.0.0.1:8545
```

---

## Troubleshooting

### "Không thể kết nối Anvil"
→ Kiểm tra Anvil đang chạy: `curl http://127.0.0.1:8545`

### "Borrow bị revert"  
→ Borrow amount quá lớn. Giảm xuống `80000000000` (80k USDC)

### "Liquidation simulation revert"
→ HF vẫn >= 1.0. Cần crash giá nhiều hơn:
```powershell
.\scripts\crash_price.ps1 -DropPercent 50
```

### "Không đủ USDC"
→ Chạy lại setup để fund thêm USDC cho liquidator

### "Forge not found"
→ Cài Foundry: `curl -L https://foundry.paradigm.xyz | bash && foundryup`

### "Token approval"
→ Liquidator cần approve USDC cho Aave Pool trước khi liquidate:
```powershell
cast send 0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48 "approve(address,uint256)" 0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff --private-key 0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d --rpc-url http://127.0.0.1:8545
```

---

## Addresses Cheat Sheet

| Contract | Address |
|----------|---------|
| Aave V3 Pool | `0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2` |
| Aave Oracle | `0x54586bE62E3c3580375aE3723C145253060Ca0C2` |
| PoolAddressesProvider | `0x2f39d218133AFaB8F2B819B1066c7E434Ad94E9e` |
| WETH | `0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2` |
| USDC | `0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48` |
| Chainlink ETH/USD | `0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419` |
| Borrower (Anvil #0) | `0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266` |
| Liquidator (Anvil #1) | `0x70997970C51812dc3A010C7d01b50e0d17dc79C8` |
