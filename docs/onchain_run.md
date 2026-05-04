# Hướng dẫn chuyển chạy lên Testnet / Mainnet

Tài liệu này tóm tắt các thay đổi cần thực hiện khi chuyển từ môi trường "local fork" (Anvil/Hardhat) sang chạy trên testnet hoặc mainnet thật.

## 1. Mục tiêu
- Thay đổi cấu hình runtime (`.env`) và một số preset trong code để phù hợp môi trường thật.
- Kiểm tra các địa chỉ hợp đồng, endpoint RPC/WS, và khóa riêng.
- Chạy thử trên testnet (ví dụ: Sepolia) trước khi chạy mainnet.

## 2. Thay đổi trong code
Thay các preset cấu hình mặc định dùng cho local fork sang preset `mainnet()` để bật các tham số an toàn hơn.

- File: [src/main.rs](src/main.rs#L267)

Thay:

```rust
let mut oracle_config = OracleConfig::local_fork();
```

Bằng:

```rust
let mut oracle_config = OracleConfig::mainnet();
```

- File: [src/main.rs](src/main.rs#L321)

Thay:

```rust
let mut profit_config = ProfitConfig::local_fork();
```

Bằng:

```rust
let mut profit_config = ProfitConfig::mainnet();
```

- File: [src/main.rs](src/main.rs#L354)

Thay:

```rust
let strategy_config = StrategyConfig::local_fork();
```

Bằng:

```rust
let strategy_config = StrategyConfig::mainnet();
```

> Lưu ý: Những vị trí line number ở trên là tham chiếu trong workspace hiện tại; nếu code được refactor, tìm các chỗ gọi `.local_fork()` và thay thế tương ứng.

## 3. Thay đổi chính trong `.env`
Các biến chính cần cập nhật khi chạy trên testnet/mainnet:

- RPC / WS endpoints
```dotenv
RPC_URL=https://eth-mainnet.g.alchemy.com/v2/YOUR_API_KEY
# hoặc testnet
# RPC_URL=https://eth-sepolia.g.alchemy.com/v2/YOUR_API_KEY
ORACLE_WS_URL=wss://eth-mainnet.g.alchemy.com/v2/YOUR_API_KEY
AAVE_WS_URL=wss://eth-mainnet.g.alchemy.com/v2/YOUR_API_KEY
```

- Khóa riêng (sử dụng ví testnet, giữ an toàn, KHÔNG commit vào repo)
```dotenv
PRIVATE_KEY=0xYOUR_TESTNET_PRIVATE_KEY
```

- Ngưỡng lợi nhuận (thực tế yêu cầu cao hơn)
```dotenv
PROFIT_MIN_USD=50.0
PROFIT_MIN_ROI_PCT=200.0
PROFIT_VERBOSE=false
```

- Tham số thực thi
```dotenv
EXECUTOR_DRY_RUN=false
EXECUTOR_SIMULATE_BEFORE_EXECUTE=true
EXECUTOR_PARALLEL_EXECUTION=false
EXECUTOR_MAX_CONCURRENT=3
```

- Polling / rate-limit (giảm tần suất để tránh rate limit trên mainnet)
```dotenv
ORACLE_POLL_INTERVAL_MS=12000
ORACLE_VERBOSE_LOGGING=false
```

- Kiểm tra các địa chỉ hợp đồng Aave / Oracle: nếu sử dụng mạng khác (testnet), cần verify `AAVE_POOL_ADDRESS`, `AAVE_ORACLE_ADDRESS`, `AAVE_ADDRESSES_PROVIDER` tương ứng với network đó.

## 4. Checklist trước khi chạy trên testnet/mainnet
- [ ] Sửa các `.local_fork()` → `.mainnet()` trong source (hoặc tạo flag runtime để chọn preset)
- [ ] Cập nhật `RPC_URL` và `ORACLE_WS_URL` trong `.env` (dùng provider có quota)
- [ ] Thay `PRIVATE_KEY` bằng tài khoản testnet có ETH để trả gas
- [ ] Điều chỉnh `PROFIT_MIN_USD` và `PROFIT_MIN_ROI_PCT` phù hợp môi trường thật
- [ ] Kiểm tra `AAVE_*` addresses đúng với network
- [ ] `cargo build` thành công
- [ ] Chạy trên Sepolia (hoặc testnet lựa chọn) và quan sát logs, dry-run trước khi bật execute
- [ ] Mở chế độ giám sát nonce / pending txs (nonce manager) để tránh double-spend
- [ ] Backup / bảo mật private key (không commit vào VCS)

## 5. Ghi chú về `RESERVE_CATALOG` và cấu hình reserve
- Trong `.env` có biến tùy chọn `RESERVE_CATALOG`:
```dotenv
# RESERVE_CATALOG=WETH=0xYourWeth,USDC=0xYourUsdc,WBTC=0xYourWbtc
```
- Dùng khi bạn triển khai pool riêng hoặc muốn override địa chỉ token mặc định. Trên mainnet thường không cần thay, nhưng nếu dùng private deployment hoặc testnet fork, hãy cấu hình cho phù hợp.

## 6. Về Block Watcher (tại sao vẫn cần)
- Mặc dù hệ thống đã watch Aave events và Oracle price updates, `block watcher` vẫn có vai trò: heartbeat, phát hiện RPC disconnect, làm trigger fallback để re-scan trạng thái on-chain khi event bị miss, thực hiện periodic reconciliation (ví dụ daily bootstrap), và đồng bộ nonce.
- Đảm bảo `Block watcher` được bật và `handle_block()` trong `RiskEngine` không chỉ log mà thực hiện các bước reconcile/call kiểm tra theo chu kỳ.

## 7. Lời khuyên vận hành
- Luôn kiểm tra logs và số dư ví (ETH) trước khi bật executor thực tế.
- Test toàn bộ flow trên testnet, ghi nhận các edge-case (reorg, RPC error, feed stale).
- Sử dụng API provider có fallback (Alchemy/Infura/QuickNode) hoặc tự quản lý node để giảm rủi ro rate-limit.

---

Tệp này được lưu tại `docs/onchain_run.md` trong workspace. Nếu bạn muốn, tôi có thể:
- Thêm đoạn lệnh shell để deploy/test nhanh trên Sepolia.
- Thêm ví dụ cấu hình `.env` cho Sepolia.
- Hoặc commit file này vào git với message bạn chọn.

