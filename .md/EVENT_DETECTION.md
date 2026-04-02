# Phát hiện sự kiện từ Blockchain

## Cách hoạt động

Bot liquidator của bạn hiện có 2 cơ chế phát hiện sự kiện:

### 1. **Block Watcher** ✅
- Poll blockchain mỗi 12 giây để phát hiện block mới
- Tự động detect khi có block mới được mine
- File: `src/provider/rpc.rs` - method `watch_blocks()`

### 2. **Aave Event Watcher** 🆕
- Lắng nghe các events từ Aave Pool smart contract:
  - `Borrow` - Khi user vay tiền
  - `Withdraw` - Khi user rút collateral
  - `Repay` - Khi user trả nợ
  - `LiquidationCall` - Khi có liquidation xảy ra
  
- Poll blockchain mỗi 3 giây để query logs
- Tự động gửi event đến RiskEngine để kiểm tra health factor
- File: `src/provider/rpc.rs` - method `watch_aave_events()`

### 3. **PriceUpdate Source (Oracle)** ✅
- Oracle workers phát hiện lệch giá và emit `Event::PriceUpdate`
- Dùng chung event channel với Block/Aave events
- Không phụ thuộc pending transaction stream

## Setup

### Bước 1: Lấy địa chỉ Aave Pool Contract

Trong `main.rs`, cần thay địa chỉ Aave Pool contract:

```rust
// Mainnet Aave V3 Pool: 0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2
// Hoặc check trong fork của bạn
let aave_pool_address = "0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2".parse().unwrap();
```

**Cách tìm địa chỉ:**
```bash
# Nếu dùng Anvil fork từ mainnet
anvil --fork-url https://eth-mainnet.alchemyapi.io/v2/YOUR_KEY

# Check Aave deployment addresses
# https://docs.aave.com/developers/deployed-contracts/v3-mainnet
```

### Bước 2: Chạy local fork

```bash
# Fork Ethereum mainnet với Aave V3
anvil --fork-url https://eth-mainnet.alchemyapi.io/v2/YOUR_ALCHEMY_KEY

# Hoặc với Hardhat
npx hardhat node --fork https://eth-mainnet.alchemyapi.io/v2/YOUR_KEY
```

### Bước 3: Chạy bot

```bash
cargo run
```

## Luồng hoạt động

```
Blockchain (http://127.0.0.1:8545)
    │
    ├─── Block Watcher ───────┐
    │    (mỗi 12s)            │
    │                         │
    ├─── Aave Event Watcher ──┤ ──> mpsc::channel
    │    (mỗi 3s)            │       │
    │                         │       ▼
                          RiskEngine
                            │
                            ├─ Calculate Health Factor
                            ├─ Detect Liquidation Opportunities
                            └─ Send to Executor
```

## Events được phát hiện

### Event::PriceUpdate
- Từ Oracle hoặc price feed
- Trigger health factor recalculation cho tất cả users

### Event::Block
- New block detected
- Có thể dùng để trigger periodic tasks

## Testing

### Test với sự kiện thật:

1. Tạo transaction trên fork:
```javascript
// Sử dụng ethers.js hoặc foundry cast
const tx = await aavePool.borrow(
  assetAddress,
  amount,
  interestRateMode,
  referralCode,
  userAddress
);
```

2. Bot sẽ tự động detect event từ log

3. Xem logs:
```
📢 Detected Borrow event at block 12345
Health Factor for user 0x123...: 1.05 (Risky!)
```

### Test với simulation (như hiện tại):

Bot vẫn hỗ trợ manual events cho testing:
```rust
tx.send(Event::PriceUpdate {
    asset_id: "ETH".to_string(),
    new_price: 0.9,
}).await.unwrap();
```

## Tối ưu hóa

### 1. Giảm polling interval cho mainnet:
```rust
// Trong watch_aave_events()
let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(1)); // Nhanh hơn
```

### 2. Filter specific users:
```rust
// Chỉ watch users trong danh sách risky
let filter = Filter::new()
    .address(aave_pool_address)
    .topic2(risky_user_addresses); // Chỉ lọc users cụ thể
```

### 3. Sử dụng WebSocket thay vì HTTP polling:
```rust
// Thay Provider<Http> bằng Provider<Ws>
use ethers::providers::Ws;
let provider = Provider::<Ws>::connect("ws://127.0.0.1:8545").await?;
```

## Troubleshooting

### "Failed to fetch logs"
- Check RPC endpoint có hoạt động không
- Check địa chỉ Aave Pool contract đúng chưa

### "No events detected"
- Verify có transactions xảy ra trên blockchain không
- Check event signatures có đúng không (Aave V2 vs V3 khác nhau)
- Thử giảm polling interval

### "No events detected"
- Verify có transactions xảy ra trên blockchain không
- Check event signatures có đúng không (Aave V2 vs V3 khác nhau)
- Thử giảm polling interval
