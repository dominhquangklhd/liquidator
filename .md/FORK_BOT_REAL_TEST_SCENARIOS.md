# Fork Network Real-Bot Test Scenarios

## 1. Muc tieu

Tai lieu nay dinh nghia cac kich ban test thuc te khi chay bot tren mang fork de xac nhan:

- Luong du lieu event -> risk -> storage -> worker -> execute hoat dong dung.
- Cac co che an toan (preflight, simulate, gas guard, nonce handling) hoat dong dung.
- Ket qua thanh ly va hanh vi fallback dung voi ky vong.

## 2. Pham vi kiem tra

- Su dung Anvil fork (mainnet/sepolia fork tuy moi truong).
- Chay bot thuc te (khong chi unit test), theo doi log va trang thai on-chain.
- Khong can benchmark hieu nang cao cap o giai doan nay.

## 3. Dieu kien tien quyet

- Da khoi dong Anvil fork.
- Da setup scenario co borrower/co vi the.
- Da co private key liquidator va so du token can thiet.
- Da tao file .env tu .env.example va set cac bien can thiet.

Lenh tham khao:

1. scripts/start_anvil.ps1
2. scripts/setup_liquidation_scenario.ps1
3. scripts/crash_price.ps1
4. cargo run

## 4. Test matrix uu tien cao

### Scenario A - Smoke boot + ket noi

Muc tieu:

- Xac nhan bot khoi dong va ket noi duoc RPC/fork.

Buoc chay:

1. Khoi dong Anvil.
2. Chay bot voi cau hinh mac dinh.
3. Quan sat log startup.

Expected:

- Khong panic.
- Co log connected chain, block watcher, event watcher, oracle worker, executor worker.

Pass/Fail:

- Pass neu bot chay on dinh > 60s va khong co loi khoi tao critical.

---

### Scenario B - PriceUpdate pipeline

Muc tieu:

- Kiem tra event gia thay doi duoc day qua RiskEngine va cap nhat hot cache.

Buoc chay:

1. Chay bot.
2. Tao bien dong gia (co the dung script crash gia nhe).
3. Theo doi log bucket change va cap nhat target.

Expected:

- Co log PriceUpdate handling.
- User bi anh huong duoc tinh lai HF.
- Hot cache co thay doi phu hop.

Pass/Fail:

- Pass neu co it nhat 1 target duoc update va log khong cho thay pipeline bi nghen.

---

### Scenario C - PriceUpdate -> Block stability

Muc tieu:

- Xac nhan event Block khong lam sai lech ket qua da tinh tu PriceUpdate.

Buoc chay:

1. Chay bot voi scenario da co user gan nguong liquidation.
2. Tao PriceUpdate (vi du crash ETH nhe).
3. Quan sat user vao hot cache va HF thay doi.
4. Cho block tiep theo, quan sat trang thai van on dinh.

Expected:

- PriceUpdate phase: HF cap nhat dung theo gia moi, target duoc cap nhat vao cache.
- Block phase: khong tu y rollback HF neu khong co event gia moi.

Pass/Fail:

- Pass neu thay day du 2 pha va HF sau block giong HF sau PriceUpdate.

---

### Scenario D - Strategy route selection

Muc tieu:

- Kiem tra bot chon route dung (Direct/FlashLoan/Skip) theo cau hinh va context.

Buoc chay:

1. Chay bot voi flash loan tat.
2. Tao target liquidatable -> quan sat method Direct/Skip.
3. Bat flash loan + set LIQUIDATOR_CONTRACT.
4. Tao target moi -> quan sat route FlashLoan duoc chon.

Expected:

- Strategy log method ro rang.
- Worker truyen method dung vao executor.
- Neu FlashLoan chua tich hop full, phai fail co thong diep ro rang (khong silent).

Pass/Fail:

- Pass neu route duoc phan nhanh dung theo config.

---

### Scenario E - Preflight/Simulation guards

Muc tieu:

- Dam bao dieu kien an toan truoc khi gui tx hoat dong dung.

Buoc chay:

1. Chay voi EXECUTOR_SIMULATE_BEFORE_EXECUTE=true.
2. Tao 1 case khong the liquidate (HF >= 1) -> phai skip/fail preflight.
3. Tao 1 case liquidatable (HF < 1) -> preflight pass.
4. Thu EXECUTOR_SIMULATE_BEFORE_EXECUTE=false de so sanh hanh vi.

Expected:

- HF >= 1 bi chan tai preflight.
- HF < 1 moi di tiep execute.
- Toggle simulate lam thay doi buoc simulate nhung khong pha preflight HF check.

Pass/Fail:

- Pass neu khong co tx nao duoc gui cho target khong liquidatable.

---

### Scenario F - Real liquidation success path

Muc tieu:

- Xac nhan e2e thanh cong cho 1 vu thanh ly thuc su tren fork.

Buoc chay:

1. Setup user HF < 1.
2. Dam bao liquidator du token debt va allowance.
3. Chay bot de worker bat target va execute.
4. Kiem tra post-state account data.

Expected:

- Co tx hash va receipt success.
- Debt giam, HF tang (hoac position dong neu full liquidation).
- Target duoc remove khoi hot cache sau thanh cong.

Pass/Fail:

- Pass neu tx success va state sau thanh ly hop ly.

---

### Scenario G - Nonce contention and resilience

Muc tieu:

- Kiem tra bot xu ly dung khi gap nonce conflict / pending congestion.

Buoc chay:

1. Co tinh tao 2 luong gui tx gan nhau bang cung wallet.
2. Quan sat loi nonce too low/too many pending.
3. Theo doi nonce sync worker va hanh vi retry/skip.

Expected:

- Loi duoc log ro rang.
- Bot khong crash.
- Sau mot thoi gian, nonce sync dua he thong ve trang thai on dinh.

Pass/Fail:

- Pass neu bot tu phuc hoi ma khong can restart.

---

### Scenario H - Gas guard / profitability guard

Muc tieu:

- Xac nhan bot khong execute khi chi phi/gas vuot nguong.

Buoc chay:

1. Dat max_gas_price_gwei thap.
2. Tao dieu kien gas cao (hoac gia lap).
3. Tao target co profit bien mong.

Expected:

- Target bi skip do gas/profit threshold.
- Khong gui tx vo ich.

Pass/Fail:

- Pass neu khong co tx send khi vi pham guard.

## 5. Kich ban hoi quy toi thieu truoc moi release

1. Scenario A
2. Scenario C
3. Scenario E
4. Scenario F
5. Scenario G

## 6. Tieu chi hoan tat giai doan nay

- Cac scenario A-F co ket qua pass tai it nhat 2 lan chay lien tiep.
- Khong con false-positive "test pass nhung tx fail" cho case execute real.
- Co log/bao cao ngan cho moi scenario: input, ket qua, tx hash (neu co), ly do fail.

## 7. Ghi chu trien khai tiep theo

- Tach test read-only va write-tx de giam flakiness.
- Them lock toan cuc cho write-tx tests de tranh tranh chap nonce.
- Nang test execute_real_liquidation thanh strict assert (fail neu tx fail trong preconditions hop le).
