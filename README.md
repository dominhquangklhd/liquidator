# Hệ thống Liquidator

Hệ thống giám sát rủi ro và gợi ý thanh lý cho giao thức cho vay DeFi, xây dựng bằng Rust theo kiến trúc event-driven.

## Giới thiệu nhanh

- Theo dõi giá từ oracle, cập nhật vị thế người dùng theo sự kiện.
- Tính Health Factor (HF) và phân loại rủi ro theo mức độ.
- Tối ưu hiệu năng bằng cập nhật cục bộ theo từng tài sản bị ảnh hưởng.

## Cấu trúc module chính

- `src/events`: Định nghĩa `Event` (Price, Block) và `Dispatcher` bất đồng bộ.
- `src/risk`: Logic core tính rủi ro.
    - `engine.rs`: Vòng xử lý sự kiện.
    - `health_factor.rs`: Công thức tính HF.
    - `bucket.rs`: Phân loại rủi ro (Safe, Watch, Risk, Danger, Liquidate).
- `src/data`: Mô hình dữ liệu (`User`, `Asset`) và `Registry` (Asset -> User index).
- `src/executor`: Khung thực thi thanh lý (placeholder).

## Tính năng chính

- Event-driven, phản ứng ngay khi giá cập nhật.
- Tra cứu O(1) người dùng bị ảnh hưởng theo tài sản.
- Cập nhật cục bộ, không quét toàn bộ người dùng.
- Có thể mở rộng đa luồng trong tương lai (hiện chạy tuần tự để đảm bảo ordering).

## Yêu cầu môi trường

- Rust stable
- Windows: MSVC C++ Build Tools (có `link.exe`)
- Node.js + npm (để chạy Hardhat fork)
- Foundry (cần `cast`, `forge`) để chạy các script on-chain
- PowerShell (các script test là `.ps1`)

## Cấu hình môi trường

Các cấu hình liên quan RPC và executor:

- [src/provider/rpc.rs](src/provider/rpc.rs)
- [src/oracle/config.rs](src/oracle/config.rs)
- [src/executor/config.rs](src/executor/config.rs)

Biến môi trường thường dùng:

```text
RPC_URL=...
PRIVATE_KEY=...
CHAIN_ID=...
ETH_RPC_URL=...
```

`ETH_RPC_URL` dùng để fork mainnet khi chạy Hardhat.

## Build và chạy mô phỏng cơ bản

```bash
cargo build
cargo run
```

Chạy ví dụ storage (nếu cần):

```bash
cargo run --example storage_example
```

## Chạy Hardhat local fork (mainnet fork)

1) Cài dependencies cho Hardhat:

```bash
cd fork-blockchain
npm install
```

2) Thiết lập RPC nguồn để fork (PowerShell):

```powershell
$env:ETH_RPC_URL="https://mainnet.your-provider.com/your-key"
```

3) Chạy Hardhat fork:

```powershell
\scripts\start_hardhat.ps1
```

Tùy chọn nâng cao:

```powershell
\scripts\start_hardhat.ps1 -RpcUrl "https://..." -ForkBlock 24700000 -Port 8545
```

Sau khi chạy, RPC local sẽ ở `http://127.0.0.1:8545`.

## Chạy các kịch bản test bằng script

Tất cả script yêu cầu Hardhat đang chạy và Foundry đã cài đặt.

### Kịch bản single-user (wstETH)

```powershell
\scripts\single-user\setup_liquidation_scenario_wstETH.ps1
\scripts\single-user\crash_price_wstETH.ps1 -PriceDrop 15
cargo run
```

### Kịch bản multi-user (nhiều borrower)

```powershell
\scripts\multi-users\setup_multi_liquidation_wstETH.ps1
\scripts\multi-users\crash_price_multi_wstETH.ps1 -PriceDrop 30
cargo run
```

### Kịch bản multi-token (wstETH + WBTC)

```powershell
\scripts\multi-users\setup_multi_tokens.ps1
\scripts\multi-users\crash_multi_tokens.ps1 -PriceDrop 8 -DebtPump 5
cargo run
```

### Benchmark latency

```powershell
\scripts\single-user\benchmark_latency_wstETH.ps1
```

## Chạy dashboard UI

Dashboard là web tĩnh, đọc dữ liệu SQLite cục bộ.

1) Mở server tĩnh (khuyến nghị):

```bash
cd dashboard-ui
python -m http.server 5173
```

Sau đó mở `http://127.0.0.1:5173` và chọn file DB từ máy.

Nếu không có Python, có thể dùng:

```bash
cd dashboard-ui
npx serve . -l 5173
```

2) Tên DB mặc định theo cấu hình `db_path` của storage. Xem thêm tại [src/storage/mod.rs](src/storage/mod.rs).

## Tài liệu bổ sung

- Diagram PlantUML: xem [docs/README.md](docs/README.md)
- Hướng dẫn storage: xem [STORAGE_QUICKSTART.md](STORAGE_QUICKSTART.md)

