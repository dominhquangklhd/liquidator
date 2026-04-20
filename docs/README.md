# Tài liệu Khóa luận — Liquidator System

## Mục lục Diagram PlantUML

### Tổng quan hệ thống

| File | Nội dung |
|------|---------|
| [00_system_overview.puml](diagrams/00_system_overview.puml) | Kiến trúc tổng thể — Component Diagram |

---

### Module 1 — Risk Engine (`src/risk/`)

**Chức năng:** Nhận events từ MPSC channel, tính toán Health Factor, phân loại rủi ro và phát hiện vị thế cần thanh lý.

| File | Loại diagram |
|------|-------------|
| [01_risk_engine_usecase.puml](diagrams/01_risk_engine_usecase.puml) | Use Case Diagram |
| [01_risk_engine_sequence.puml](diagrams/01_risk_engine_sequence.puml) | Sequence Diagram |

**Công thức Health Factor:**
```
HF = Σ(Collateral_i × LiquidationThreshold_i) / Σ(Debt_i)
```

---

### Module 2 — Oracle (`src/oracle/`)

**Chức năng:** Quản lý Chainlink price feeds, poll giá định kỳ, phát hiện sai lệch giá, emit PriceUpdate events và cung cấp price cache cho các module khác.

| File | Loại diagram |
|------|-------------|
| [02_oracle_usecase.puml](diagrams/02_oracle_usecase.puml) | Use Case Diagram |
| [02_oracle_sequence.puml](diagrams/02_oracle_sequence.puml) | Sequence Diagram |

---

### Module 3 — Profit (`src/profit/`)

**Chức năng:** Tính toán lợi nhuận ước tính cho mỗi cơ hội thanh lý, bao gồm gross profit, gas cost và slippage.

| File | Loại diagram |
|------|-------------|
| [03_profit_usecase.puml](diagrams/03_profit_usecase.puml) | Use Case Diagram |
| [03_profit_sequence.puml](diagrams/03_profit_sequence.puml) | Sequence Diagram |

**Công thức tính lợi nhuận:**
```
debt_to_cover        = total_debt × close_factor (50%)
collateral_received  = debt_to_cover × (1 + bonus%)
gross_profit         = debt_to_cover × bonus%
net_profit           = gross_profit − gas_cost − slippage
```

---

### Module 4 — Strategy (`src/strategy/`)

**Chức năng:** Quyết định chiến lược tối ưu cho Direct execution, ưu tiên hóa danh sách targets theo multi-factor scoring, quản lý circuit breaker.

| File | Loại diagram |
|------|-------------|
| [04_strategy_usecase.puml](diagrams/04_strategy_usecase.puml) | Use Case Diagram |
| [04_strategy_sequence.puml](diagrams/04_strategy_sequence.puml) | Sequence Diagram |

**Multi-Factor Score:**
```
score = 0.40 × norm(profit)
      + 0.35 × norm(urgency = 1/HF)
      + 0.15 × norm(ROI)
      + 0.10 × norm(1/debt_size)
```

---

### Module 5 — Executor (`src/executor/`)

**Chức năng:** Thực thi giao dịch thanh lý trực tiếp lên blockchain, quản lý nonce, approve ERC20 tokens, invoke `liquidationCall()` trên Aave Pool.

| File | Loại diagram |
|------|-------------|
| [05_executor_usecase.puml](diagrams/05_executor_usecase.puml) | Use Case Diagram |
| [05_executor_sequence.puml](diagrams/05_executor_sequence.puml) | Sequence Diagram |

---

### Module 6 — Storage (`src/storage/`)

**Chức năng:** Kiến trúc lưu trữ hybrid — Hot Cache in-memory cho tốc độ truy xuất < 1ms, SQLite cho persistence và lịch sử thanh lý.

| File | Loại diagram |
|------|-------------|
| [06_storage_usecase.puml](diagrams/06_storage_usecase.puml) | Use Case Diagram |
| [06_storage_sequence.puml](diagrams/06_storage_sequence.puml) | Sequence Diagram |

---

## Cách render PlantUML

### VS Code Extension
Cài extension **PlantUML** (jebbs.plantuml):
1. Mở file `.puml`
2. `Alt+D` để preview trong VS Code
3. `Ctrl+Shift+P` → "PlantUML: Export Current File Diagrams"

### PlantUML Online
Paste nội dung vào [https://www.plantuml.com/plantuml/uml/](https://www.plantuml.com/plantuml/uml/)

### Command Line
```bash
java -jar plantuml.jar docs/diagrams/*.puml
```
