# Kiến trúc Hệ thống Liquidator

## 📋 Tổng quan

Hệ thống Liquidator là một ứng dụng Rust theo dõi và thanh lý các vị thế có rủi ro trên Aave Protocol. Hệ thống sử dụng kiến trúc event-driven với async/await pattern (Tokio runtime).

---

## 🏗️ Kiến trúc tổng thể

```
┌─────────────────────────────────────────────────────────────────┐
│                        MAIN THREAD                               │
│                      (tokio::main)                               │
└────────────┬────────────────────────────────────────────────────┘
             │
             │ spawns
             ▼
┌────────────────────────────────────────────────────────────────────┐
│                     BACKGROUND WORKERS (async)                      │
├─────────────────────┬──────────────────┬───────────────────────────┤
│  Block Watcher      │  Event Watcher   │    Risk Engine           │
│  Worker             │  Worker          │    Worker                │
└─────────────────────┴──────────────────┴───────────────────────────┘
```

---

## 📦 Component Diagram

```
┌───────────────────────────────────────────────────────────────────┐
│                          BLOCKCHAIN LAYER                          │
│  ┌─────────────────────────────────────────────────────────────┐  │
│  │     Aave Fork (Local RPC: http://127.0.0.1:8545)            │  │
│  │  • Aave Pool Contract (0xE7EC...)                           │  │
│  │  • Supply/Borrow/Liquidation Events                         │  │
│  └─────────────────────────────────────────────────────────────┘  │
└──────────────────────────────┬────────────────────────────────────┘
                               │
                               │ alloy-rs / ethers-rs
                               ▼
┌───────────────────────────────────────────────────────────────────┐
│                         PROVIDER LAYER                             │
│  ┌─────────────────────────────────────────────────────────────┐  │
│  │                    AaveProvider                             │  │
│  │  • RPC Connection                                           │  │
│  │  • watch_blocks()      ─────┐                               │  │
│  │  • watch_aave_events() ─────┤                               │  │
│  └─────────────────────────────┼───────────────────────────────┘  │
└────────────────────────────────┼──────────────────────────────────┘
                                 │
                                 │ produces Events
                                 ▼
┌───────────────────────────────────────────────────────────────────┐
│                      EVENT CHANNEL (MPSC)                          │
│  ┌─────────────────────────────────────────────────────────────┐  │
│  │  Channel: mpsc::channel<Event>(buffer: 100)                 │  │
│  │                                                              │  │
│  │  Event Types:                                               │  │
│  │  • PriceUpdate { asset_id, new_price }                      │  │
│  │  • UserDeposit { user_id, asset_id, amount }                │  │
│  │  • UserBorrow  { user_id, asset_id, amount }                │  │
│  │  • UserRepay   { user_id, asset_id, amount }                │  │
│  │  • UserWithdraw{ user_id, asset_id, amount }                │  │
│  └─────────────────────────────────────────────────────────────┘  │
└────────────────────────────────┬──────────────────────────────────┘
                                 │
                                 │ consumes Events
                                 ▼
┌───────────────────────────────────────────────────────────────────┐
│                         RISK ENGINE                                │
│  ┌─────────────────────────────────────────────────────────────┐  │
│  │                      RiskEngine                             │  │
│  │                                                              │  │
│  │  Data Stores:                                               │  │
│  │  ├─ assets: HashMap<String, Asset>                          │  │
│  │  ├─ users: HashMap<String, User>                            │  │
│  │  └─ registry: Registry (user-asset mapping)                 │  │
│  │                                                              │  │
│  │  Core Logic:                                                │  │
│  │  ├─ run() ──────► Event Loop (async)                        │  │
│  │  ├─ handle_event() ──► Process each event type             │  │
│  │  ├─ check_user_risk() ──► Calculate Health Factor          │  │
│  │  └─ update_price() ──► Update asset prices                  │  │
│  │                                                              │  │
│  │  Risk Buckets:                                              │  │
│  │  ├─ 🟢 Safe (HF > 1.2)                                      │  │
│  │  ├─ 🟡 Warning (1.0 < HF <= 1.2)                            │  │
│  │  └─ 🔴 Danger (HF < 1.0) ─────► LIQUIDATE                   │  │
│  └─────────────────────────────────────────────────────────────┘  │
└────────────────────────────────┬──────────────────────────────────┘
                                 │
                                 │ triggers
                                 ▼
┌───────────────────────────────────────────────────────────────────┐
│                         EXECUTION LAYER (TODO)                     │
│  • Liquidation Executor                                            │
│  • Transaction Builder                                             │
│  • Gas Optimizer                                                   │
└────────────────────────────────────────────────────────────────────┘
```

---

## 🔄 Event Flow (Sequence Diagram)

```
┌─────────┐     ┌──────────┐     ┌─────────┐     ┌───────────┐
│Blockchain│     │Block     │     │Event    │     │Risk       │
│          │     │Watcher   │     │Channel  │     │Engine     │
└────┬─────┘     └────┬─────┘     └────┬────┘     └─────┬─────┘
     │                │                 │                │
     │  New Block     │                 │                │
     ├───────────────►│                 │                │
     │                │                 │                │
     │                │  PriceUpdate    │                │
     │                ├────────────────►│                │
     │                │                 │                │
     │                │                 │  Event         │
     │                │                 ├───────────────►│
     │                │                 │                │
     │                │                 │                │ calculate_hf()
     │                │                 │                ├──────────┐
     │                │                 │                │          │
     │                │                 │                │◄─────────┘
     │                │                 │                │
     │                │                 │                │ if HF < 1.0
     │                │                 │                │
     │                │                 │                │ mark_for_liquidation()
     │                │                 │                ├───────────┐
     │                │                 │                │           │
     │                │                 │                │◄──────────┘
     │                │                 │                │
     │  Aave Event    │                 │                │
     │  (Supply)      │                 │                │
     ├───────────────►│                 │                │
     │                │                 │                │
     │                │  UserDeposit    │                │
     │                ├────────────────►│                │
     │                │                 │                │
     │                │                 │  Event         │
     │                │                 ├───────────────►│
     │                │                 │                │
     │                │                 │                │ update_user()
     │                │                 │                ├────────────┐
     │                │                 │                │            │
     │                │                 │                │◄───────────┘
     │                │                 │                │
```

---

## 🧮 Health Factor Calculation

Health Factor (HF) xác định mức độ an toàn của một vị thế:

```
                  Σ (Collateral_i × Price_i × LiquidationThreshold_i)
Health Factor = ─────────────────────────────────────────────────────
                           Σ (Debt_j × Price_j)
```

### Ví dụ:

**User Safe:**
- Collateral: 10 ETH @ $1.0 = $10.0
- Debt: 5000 USDC @ $0.0005 = $2.5 ETH equivalent
- Liquidation Threshold: 85%

```
HF = (10 × 1.0 × 0.85) / 2.5 = 8.5 / 2.5 = 3.4 ✓ SAFE
```

**User Risky:**
- Collateral: 10 ETH @ $1.0 = $10.0
- Debt: 16000 USDC @ $0.0005 = $8.0 ETH equivalent
- Liquidation Threshold: 85%

```
HF = (10 × 1.0 × 0.85) / 8.0 = 8.5 / 8.0 = 1.0625 ⚠️ DANGER
```

Khi giá ETH giảm 10% → 0.9:
```
HF = (10 × 0.9 × 0.85) / 8.0 = 7.65 / 8.0 = 0.95625 🔴 LIQUIDATE!
```

---

## 🎯 Risk Buckets

Hệ thống phân loại users vào các buckets rủi ro:

| Bucket    | Health Factor | Status | Action |
|-----------|--------------|--------|---------|
| 🟢 **Safe**    | HF > 1.2     | An toàn | Monitor |
| 🟡 **Warning** | 1.0 < HF ≤ 1.2 | Cảnh báo | Alert |
| 🔴 **Danger**  | HF < 1.0     | Nguy hiểm | Liquidate |

---

## ⚙️ Async Workers

### 1. **Block Watcher Worker**
```rust
tokio::spawn(async move {
    provider.watch_blocks().await
});
```
- **Nhiệm vụ**: Poll blockchain để phát hiện blocks mới
- **Frequency**: Mỗi 2-3 giây (tùy vào block time)
- **Output**: Triggers price oracle updates

### 2. **Aave Event Watcher Worker**
```rust
tokio::spawn(async move {
    provider.watch_aave_events(pool_address, tx).await
});
```
- **Nhiệm vụ**: Subscribe vào Aave contract events
- **Events tracked**:
  - Supply (deposit collateral)
  - Borrow (vay)
  - Repay (trả nợ)
  - Withdraw (rút collateral)
  - Liquidation
- **Output**: Gửi events qua MPSC channel

### 3. **Risk Engine Worker**
```rust
tokio::spawn(async move {
    engine.run().await  // Event loop
});
```
- **Nhiệm vụ**: Xử lý tất cả incoming events
- **Process**:
  1. Nhận event từ channel
  2. Update state (prices, balances)
  3. Recalculate health factors
  4. Classify users vào risk buckets
  5. Mark positions for liquidation (if HF < 1.0)

---

## 🔧 Data Structures

### Asset
```rust
struct Asset {
    id: String,
    symbol: String,
    decimals: u8,
    ltv: f64,                    // Loan-to-Value
    liquidation_threshold: f64,  // Ngưỡng thanh lý
    price_in_eth: f64,          // Giá hiện tại
}
```

### User
```rust
struct User {
    id: String,
    collateral: HashMap<String, f64>,  // asset_id -> amount
    debt: HashMap<String, f64>,        // asset_id -> amount
}
```

### Registry
```rust
struct Registry {
    user_assets: HashMap<String, HashSet<String>>,  // user_id -> set of asset_ids
}
```

---

## 🚀 Startup Sequence

```
1. Initialize tracing logger
2. Connect to blockchain (AaveProvider::new)
3. Create MPSC channel (tx, rx)
4. Initialize RiskEngine with rx
5. Populate initial simulation data
   ├─ Add assets (ETH, USDC)
   └─ Add users (user_safe, user_risky)
6. Spawn workers:
   ├─ Risk Engine worker
   ├─ Block Watcher worker
   ├─ Aave Event Watcher worker
   └─ Simulation worker (test only)
7. Keep main thread alive
```

---

## 📊 Example Simulation

### Kịch bản: ETH Price Crash

**Initial State:**
```
ETH Price: 1.0
user_safe:  Collateral=10 ETH, Debt=5000 USDC  → HF = 3.4 ✓
user_risky: Collateral=10 ETH, Debt=16000 USDC → HF = 1.06 ⚠️
```

**Event: PriceUpdate(ETH, 0.9)**
```
ETH Price: 0.9 (-10%)
user_safe:  Collateral=10 ETH, Debt=5000 USDC  → HF = 3.06 ✓
user_risky: Collateral=10 ETH, Debt=16000 USDC → HF = 0.95 🔴
                                                    ↓
                                            LIQUIDATE!
```

---

## 🔮 Future Enhancements

1. **Oracle Integration**: Chainlink price feeds
2. **Mempool Monitoring**: Front-running protection
3. **Execution Layer**: Automated liquidation transactions
4. **Gas Optimization**: Flashbots/MEV integration
5. **Multi-chain Support**: Aave V3 trên Polygon, Arbitrum, etc.
6. **Database**: Persistent storage (PostgreSQL/Redis)
7. **API**: REST/WebSocket endpoints for monitoring
8. **Dashboard**: Web UI for visualization

---

## 📝 Notes

- Hệ thống hiện tại là **proof-of-concept** với dữ liệu simulation
- Trong production cần integrate với:
  - Real-time price oracles
  - On-chain transaction execution
  - Proper error handling & retry logic
  - Database persistence
  - Monitoring & alerting

---

**Created:** February 2, 2026  
**Author:** Liquidator System Documentation  
**Version:** 1.0
