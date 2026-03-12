# 🧪 Kế hoạch Test luồng Strategy Decider

## Tổng quan

Module **Strategy Decider** (`src/strategy/`) quyết định chiến lược liquidation tối ưu cho mỗi cơ hội.  
Luồng chính: **Targets → Profit Filter → Strategy Decision → Priority Sort → Execution Plan → Executor**.

---

## 1. Kiến trúc luồng cần test

```
                         Input
                           │
           Vec<(LiquidationTarget, ProfitEstimate)>
                           │
                    ┌──────▼──────┐
                    │ Circuit     │──── tripped? → Tất cả Skip
                    │ Breaker     │
                    └──────┬──────┘
                           │ OK
                    ┌──────▼──────┐
                    │ Normalize   │ (min-max cho profit, ROI, HF, debt)
                    │ Metrics     │
                    └──────┬──────┘
                           │
              ┌────────────▼────────────┐
              │ For each target:        │
              │  ├─ Check profitability │
              │  ├─ Check gas price     │
              │  ├─ Check exposure      │
              │  ├─ decide_method()     │
              │  │   ├─ Direct          │
              │  │   ├─ FlashLoan       │
              │  │   └─ Skip            │
              │  └─ Priority score      │
              └────────────┬────────────┘
                           │
                    ┌──────▼──────┐
                    │ Sort by     │ priority_score DESC
                    │ Priority    │
                    └──────┬──────┘
                           │
                    ┌──────▼──────┐
                    │ Apply       │ max_concurrent_liquidations
                    │ Limit       │
                    └──────┬──────┘
                           │
                    ExecutionPlan
                           │
                    ┌──────▼──────┐
                    │ Executor    │ execute Direct hoặc FlashLoan
                    │ Worker      │
                    └─────────────┘
```

---

## 2. Phân loại Test

### 2.1 Unit Tests (đã có 13+ tests, cần bổ sung)

| # | Test Case | Mô tả | Trạng thái |
|---|-----------|--------|-----------|
| U1 | `test_normalizer_basic` | Normalize [1,2,3] → [0.0, 0.5, 1.0] | ✅ Đã có |
| U2 | `test_normalizer_single` | Normalize 1 phần tử → 0.5 | ✅ Đã có |
| U3 | `test_normalizer_inverse` | Normalize inverse (nhỏ = tốt) | ✅ Đã có |
| U4 | `test_decide_method_direct` | Đủ ETH + token → Direct | ✅ Đã có |
| U5 | `test_decide_method_flash_loan` | Thiếu token, flash loan available → FlashLoan | ✅ Đã có |
| U6 | `test_decide_method_skip` | Thiếu token, no flash loan → Skip | ✅ Đã có |
| U7 | `test_decide_method_large_debt` | Debt > threshold → FlashLoan | ✅ Đã có |
| U8 | `test_create_plan_sorts_by_priority` | Kết quả sorted DESC by priority | ✅ Đã có |
| U9 | `test_plan_concurrent_limit` | Chỉ max N targets trong plan | ✅ Đã có |
| U10 | `test_plan_exposure_limit` | Dừng khi tổng exposure vượt limit | ✅ Đã có |
| U11 | `test_circuit_breaker_trips` | N failures liên tiếp → tất cả Skip | ✅ Đã có |
| U12 | `test_circuit_breaker_reset` | 1 success → reset failure count | ✅ Đã có |
| U13 | `test_stats_tracking` | Đếm đúng direct/flash_loan/skip counts | ✅ Đã có |

**Bổ sung cần thiết:**

| # | Test Case | Mô tả | Ưu tiên |
|---|-----------|--------|---------|
| U14 | `test_config_mainnet_preset` | Verify mainnet() config values | Thấp |
| U15 | `test_config_local_fork_preset` | Verify local_fork() config values | Thấp |
| U16 | `test_priority_score_weights` | Thay đổi weights, verify score changes tương ứng | Trung bình |
| U17 | `test_gas_price_too_high_skips` | Gas > max_gas_price → Skip | Trung bình |
| U18 | `test_negative_profit_skips` | Net profit < 0 → Skip | Trung bình |
| U19 | `test_empty_input` | Input rỗng → ExecutionPlan rỗng | Thấp |
| U20 | `test_wallet_balance_update` | `update_wallet_balance` → reflect trong decisions | Trung bình |
| U21 | `test_token_balance_update` | `update_token_balance` → ảnh hưởng Direct/FlashLoan | Trung bình |
| U22 | `test_circuit_breaker_cooldown` | Sau cooldown period → hoạt động trở lại | Cao |
| U23 | `test_per_liquidation_exposure_limit` | Một target vượt max_single_exposure → Skip | Trung bình |

---

### 2.2 Integration Tests (mới — cần viết)

Test tích hợp giữa Strategy Decider với các module khác.

| # | Test Case | Mô tả | Modules liên quan |
|---|-----------|--------|-------------------|
| I1 | **Profit → Strategy pipeline** | ProfitCalculator tạo estimates → StrategyDecider nhận → tạo ExecutionPlan | `profit`, `strategy` |
| I2 | **Strategy → Executor pipeline** | ExecutionPlan → executor_worker chọn đúng method | `strategy`, `executor` |
| I3 | **Full pipeline: Storage → Profit → Strategy → Executor** | End-to-end từ targets trong storage đến plan | `storage`, `profit`, `strategy`, `executor` |
| I4 | **Oracle price change → Strategy re-evaluation** | Giá thay đổi → HF thay đổi → priority thay đổi | `oracle`, `strategy` |
| I5 | **Multi-target batch ordering** | 5+ targets với profit/HF/debt khác nhau → verify sort order | `strategy` |
| I6 | **Circuit breaker → Executor stop** | Nhiều failures → circuit breaker trip → executor nhận Skip | `strategy`, `executor` |

---

### 2.3 Scenario Tests (end-to-end trên Anvil fork)

| # | Scenario | Mô tả | Setup cần thiết |
|---|----------|--------|------------------|
| S1 | **Happy path: Direct liquidation** | Wallet đủ tiền → chọn Direct → execute thành công | Anvil + Aave fork, fund wallet với debt token |
| S2 | **Flash loan fallback** | Wallet thiếu token → chọn FlashLoan → execute | Anvil + Aave fork, flash loan pool có liquidity |
| S3 | **Multi-target priority** | 3 users undercollateralized, khác HF/debt → verify order | 3 user positions setup |
| S4 | **Circuit breaker recovery** | 5 failed liquidations → breaker trips → cooldown → resume | Force failures + wait |
| S5 | **Gas spike → Skip** | Tăng gas price trên Anvil → strategy chọn Skip | `anvil_setNextBlockBaseFeePerGas` |
| S6 | **Exposure limit enforcement** | Tổng exposure gần limit → target mới bị skip | Large existing positions |
| S7 | **Concurrent execution cap** | 10 targets nhưng max_concurrent=3 → chỉ 3 trong plan | Nhiều undercollateralized users |

---

## 3. Kế hoạch thực hiện từng bước

### Phase 1: Chạy & verify Unit Tests hiện có

```bash
# Bước 1: Chạy toàn bộ unit tests của strategy module
cargo test strategy -- --nocapture

# Kết quả kỳ vọng: 13/13 tests passed
```

### Phase 2: Bổ sung Unit Tests thiếu (U14-U23)

**File:** `src/strategy/decider.rs` (thêm vào `#[cfg(test)] mod tests`)

```bash
# Bước 2: Viết thêm tests → chạy lại  
cargo test strategy -- --nocapture

# Kết quả kỳ vọng: 23/23 tests passed
```

**Ưu tiên viết trước:**
1. `test_circuit_breaker_cooldown` (U22) — Quan trọng cho production
2. `test_gas_price_too_high_skips` (U17) — Edge case thực tế
3. `test_negative_profit_skips` (U18) — Edge case thực tế
4. `test_wallet_balance_update` (U20) + `test_token_balance_update` (U21) — Verify state management

### Phase 3: Integration Tests

**File:** `tests/strategy_integration.rs` (tạo mới)

```rust
// Cấu trúc test:
use liquidator::strategy::{StrategyDecider, StrategyConfig};
use liquidator::profit::{ProfitCalculator, ProfitConfig, ProfitEstimate};
use liquidator::storage::{HybridStorage, LiquidationTarget};

#[tokio::test]
async fn test_profit_to_strategy_pipeline() {
    // 1. Tạo mock targets
    // 2. Chạy ProfitCalculator
    // 3. Feed kết quả vào StrategyDecider
    // 4. Verify ExecutionPlan
}

#[tokio::test]
async fn test_full_pipeline_with_executor() {
    // 1. Setup storage với mock targets
    // 2. ProfitCalculator filter
    // 3. StrategyDecider create plan
    // 4. Executor nhận plan → execute
}
```

```bash
# Bước 3: Chạy integration tests
cargo test --test strategy_integration -- --nocapture
```

### Phase 4: Scenario Tests trên Anvil

```powershell
# Bước 4a: Start Anvil fork
.\scripts\start_anvil.ps1

# Bước 4b: Setup liquidation scenarios  
.\scripts\setup_liquidation_scenario.ps1

# Bước 4c: Crash price → trigger liquidations
.\scripts\crash_price.ps1

# Bước 4d: Chạy bot, observe logs
cargo run 2>&1 | Select-String "Strategy|Direct|FlashLoan|Skip|priority"
```

**Verify checklist cho mỗi scenario:**
- [ ] Strategy decision log xuất hiện
- [ ] Method đúng (Direct/FlashLoan/Skip) theo điều kiện
- [ ] Priority score hợp lý (0-10)
- [ ] Concurrent limit được enforce
- [ ] Circuit breaker trip khi có nhiều failures
- [ ] Circuit breaker reset sau success

---

## 4. Acceptance Criteria

### ✅ PASS khi:
- [ ] Tất cả 13+ unit tests hiện tại pass
- [ ] Thêm ≥5 unit tests mới (U17-U22) và pass
- [ ] ≥2 integration tests pass (I1, I2)
- [ ] ≥1 scenario test pass trên Anvil (S1)
- [ ] `cargo clippy` không có warning mới
- [ ] `cargo build` thành công (0 errors)

### ❌ FAIL khi:
- Bất kỳ unit test nào fail
- Circuit breaker không trip sau N failures
- Priority sorting sai thứ tự
- Exposure limit không được enforce
- Memory leak (Arc cycle, unbounded growth)

---

## 5. Test Data mẫu

### Target mẫu cho tests:

| User | Health Factor | Debt (USD) | Collateral (USD) | Expected Priority |
|------|--------------|------------|-------------------|-------------------|
| user_A | 0.85 | 10,000 | 12,000 | Cao (HF thấp, debt vừa) |
| user_B | 0.95 | 50,000 | 55,000 | Trung bình (HF cao hơn, debt lớn) |
| user_C | 0.70 | 2,000 | 3,000 | Cao (HF rất thấp, debt nhỏ) |
| user_D | 0.99 | 100,000 | 105,000 | Thấp (HF gần 1.0) |

### Config presets cho tests:

| Preset | Flash Loan | Max Gas (gwei) | Max Concurrent | Use Case |
|--------|-----------|----------------|----------------|----------|
| `default()` | true | 100 | 3 | Unit tests |
| `mainnet()` | true | 50 | 3 | Production simulation |
| `local_fork()` | false | 500 | 5 | Anvil integration |

---

## 6. Lệnh nhanh

```bash
# Chạy tất cả tests
cargo test

# Chỉ strategy module
cargo test strategy

# Với output chi tiết
cargo test strategy -- --nocapture

# Integration tests
cargo test --test strategy_integration

# Check code quality
cargo clippy -- -W clippy::all

# Build check
cargo build
```
