# Defense Questions Map (4 UC chinh)

Tai lieu nay tong hop 4 use case (UC) chinh va noi dung co the bi hoi khi bao ve. Moi UC co 2 phan: (1) cau hoi goi y, (2) vi tri code de tra loi.

## UC01 - Bootstrap va dong bo du lieu

**Cau hoi goi y**
- Danh sach user bootstrap lay tu dau? Khi subgraph loi thi fallback the nao? Vi sao chon HF <= 2?
- Doc vi the on-chain theo tung reserve ra sao? Khi token khong ho tro thi xu ly the nao?
- Co kiem tra drift (sai lech) giua account data va reserve data khong? Neu drift lon thi xu ly ra sao?
- Cong thuc tinh HF ban dau, risk bucket la gi?
- Vi sao can hot cache + cold storage? Muc hot cache threshold y nghia gi?
- Re-bootstrap dinh ky duoc kich hoat o dau? Co tranh luc he thong dang liquidate khong?

**Goi y tra loi ngan**
- Lay danh sach user tu subgraph (HF <= 2), fallback DB + env; luon them hardhat accounts de test local.
- Doc vi the theo tung reserve qua ProtocolDataProvider, bo qua token khong ho tro.
- Co drift check giua account-level va reserve-level; drift lon thi skip user.
- HF = sum(collateral * liquidation_threshold) / total_debt; RiskBucket phan loai theo HF.
- Hot cache luu top targets HF thap de truy xuat nhanh, cold storage luu lich su va toan bo users.
- Re-bootstrap chay dinh ky qua TriggerDailyBootstrap khi he thong idle.

**Code lien quan**
- Bootstrap flow: [src/bootstrap/onchain.rs](src/bootstrap/onchain.rs)
- Doc reserve va vi the: [src/aave_v3/reader.rs](src/aave_v3/reader.rs)
- Tinh HF: [src/risk/health_factor.rs](src/risk/health_factor.rs)
- Risk bucket: [src/risk/bucket.rs](src/risk/bucket.rs)
- Hybrid storage (hot/cold): [src/storage/mod.rs](src/storage/mod.rs), [src/storage/cache.rs](src/storage/cache.rs), [src/storage/database.rs](src/storage/database.rs)
- Trigger re-bootstrap: [src/main.rs](src/main.rs), [src/risk/engine.rs](src/risk/engine.rs)

## UC02 - Xu ly su kien bien dong vi the (deposit/borrow/repay/withdraw)

**Cau hoi goi y**
- Event Aave bat bang polling hay WS? Parse log de ra event the nao?
- Mapping reserve address -> asset symbol va normalize decimals lam sao? Fallback neu khong map duoc?
- Khi nhan event, cap nhat user state va registry ra sao? Khi nao tinh lai HF?
- Xu ly event loi, user chua ton tai, asset khong ho tro the nao?
- Co test nao minh hoa pipeline event -> HF -> hot cache?

**Goi y tra loi ngan**
- Co 2 che do: WS (push) va polling; log duoc parse theo signature va topics.
- Reserve address duoc map sang asset symbol; neu khong map duoc thi fallback hex address.
- Event cap nhat so du collateral/debt, cap nhat registry, sau do tinh lai HF cho user bi anh huong.
- Su kien loi, user chua co trong state, asset khong ho tro thi bo qua va log canh bao.

**Code lien quan**
- Watch/poll Aave events: [src/provider/rpc.rs](src/provider/rpc.rs)
- Event type: [src/events/event.rs](src/events/event.rs)
- RiskEngine handlers: [src/risk/engine.rs](src/risk/engine.rs)
- Registry (asset -> users): [src/data/registry.rs](src/data/registry.rs)

## UC03 - Cap nhat gia tai san (oracle)

**Cau hoi goi y**
- Nguon gia (Chainlink) va logic poll/deviation/stale? Co retry va fallback khong?
- Khi gia thay doi, event PriceUpdate duoc emit va RiskEngine xu ly ra sao?
- Vi sao can registry de lay danh sach user bi anh huong theo asset?
- Khi HF giam thi hot cache cap nhat the nao?
- Co test E2E cho oracle khong?

**Goi y tra loi ngan**
- Chainlink feeds duoc poll theo interval; neu deviation >= threshold thi emit event.
- Feed co staleness check, retry khi RPC loi; co fallback khi can.
- PriceUpdate -> RiskEngine lay danh sach user theo asset tu registry, tinh lai HF va cap nhat cache.
- Hot cache chi giu user co HF nho hon threshold de toi uu hieu nang.

**Code lien quan**
- Oracle manager: [src/oracle/manager.rs](src/oracle/manager.rs)
- Oracle config: [src/oracle/config.rs](src/oracle/config.rs)
- Oracle workers: [src/oracle/worker.rs](src/oracle/worker.rs)
- Price types: [src/oracle/types.rs](src/oracle/types.rs)
- RiskEngine price handler: [src/risk/engine.rs](src/risk/engine.rs)
- Oracle test: [tests/oracle_integration.rs](tests/oracle_integration.rs)

## UC04 - Thuc thi thanh ly (liquidation pipeline)

**Cau hoi goi y**
- Lay target tu hot cache theo batch va threshold the nao?
- ProfitCalculator tinh profit, chon cap collateral/debt toi uu theo cong thuc nao?
- StrategyDecider quyet dinh direct vs skip, uu tien target, circuit breaker/exposure lam sao?
- Executor kiem tra preflight (HF, gas), simulate, nonce, pending tx the nao?
- Ghi nhan ket qua vao DB va snapshot nhu the nao?

**Goi y tra loi ngan**
- Executor worker lay top targets tu hot cache theo batch_size va HF < threshold.
- ProfitCalculator tinh net profit (gross - gas - slippage) va chon cap co score cao nhat.
- StrategyDecider tinh priority theo weights, gioi han exposure va circuit breaker neu fail lien tiep.
- Executor preflight HF on-chain < 1, check gas price, simulate neu bat, quan ly nonce/pending.
- Ket qua duoc ghi vao DB va snapshot de phuc vu audit/analytics.

**Code lien quan**
- Executor loop: [src/executor/worker.rs](src/executor/worker.rs)
- Profit calculator: [src/profit/calculator.rs](src/profit/calculator.rs)
- Profit config: [src/profit/config.rs](src/profit/config.rs)
- Strategy decider: [src/strategy/decider.rs](src/strategy/decider.rs)
- Strategy config/types: [src/strategy/config.rs](src/strategy/config.rs), [src/strategy/types.rs](src/strategy/types.rs)
- Executor core: [src/executor/executor.rs](src/executor/executor.rs)
- Storage persistence: [src/storage/database.rs](src/storage/database.rs)
- Strategy tests: [tests/strategy_integration.rs](tests/strategy_integration.rs), [tests/strategy_scenario.rs](tests/strategy_scenario.rs)
- Executor tests: [tests/executor_integration.rs](tests/executor_integration.rs)

## Entry points va luong tong quan

- Entry point he thong: [src/main.rs](src/main.rs)
- So do UC tong quan: [docs/diagrams/e2e_system_usecase.puml](docs/diagrams/e2e_system_usecase.puml)
- So do tung UC: [docs/diagrams/uc/e2e_uc01_bootstrap_usecase.puml](docs/diagrams/uc/e2e_uc01_bootstrap_usecase.puml), [docs/diagrams/uc/e2e_uc02_position_sync_usecase.puml](docs/diagrams/uc/e2e_uc02_position_sync_usecase.puml), [docs/diagrams/uc/e2e_uc03_price_update_usecase.puml](docs/diagrams/uc/e2e_uc03_price_update_usecase.puml), [docs/diagrams/uc/e2e_uc04_liquidation_pipeline_usecase.puml](docs/diagrams/uc/e2e_uc04_liquidation_pipeline_usecase.puml)
