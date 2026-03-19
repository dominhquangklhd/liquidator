# Ke Hoach 4 Buoc: Strict test_execute_real_liquidation + Tranh Chap Nonce

## Muc tieu

- Bien `test_execute_real_liquidation` thanh test nghiem ngat (khong false-positive).
- Loai bo tranh chap nonce giua cac test gui transaction that su.

## Buoc 1 - Tach nhom test read-only va write-tx

Muc dich:

- Giam race condition do chay song song.

Viec can lam:

1. Danh dau ro test nao chi doc chain state, test nao co gui tx.
2. Giu read-only chay song song de tiet kiem thoi gian.
3. Gom write-tx thanh nhom chay tuan tu.

Ap dung trong file hien tai:

- Write-tx can uu tien quan ly: `test_simulate_liquidation`, `test_execute_real_liquidation`.

Tieu chi pass:

- Khi chay nhieu lan, nhom write-tx khong con loi nonce ngau nhien.

## Buoc 2 - Them global async lock cho write-tx

Muc dich:

- Dam bao tai mot thoi diem chi co 1 test dung `LIQUIDATOR_KEY` de gui tx.

Viec can lam:

1. Tao lock toan cuc trong test module (vd. `OnceLock<tokio::sync::Mutex<()>>`).
2. Boc toan bo doan code `approve/send/liquidate` trong critical section.
3. Khong lock cho read-only test.

Tieu chi pass:

- Khong con `nonce too low` khi chay full test file.

## Buoc 3 - Nang test_execute_real_liquidation thanh strict assert

Muc dich:

- Test phai fail neu execute that bai trong dieu kien hop le.

Viec can lam:

1. Giu cac precondition gate ro rang:
   - debt > 0
   - HF < 1
   - liquidator du token debt
2. Neu da qua precondition ma `result.success == false` -> `panic!/assert!` fail test.
3. Neu success, assert them post-state:
   - debt giam hoac position dong
   - HF tang (voi partial liquidation)

Tieu chi pass:

- Khong con truong hop "log loi send tx nhung test van ok".

## Buoc 4 - Co che chay test on dinh cho CI/local

Muc dich:

- Dam bao test co tinh lap lai cao.

Viec can lam:

1. Lenh khuyen nghi cho suite write-tx:
   - `cargo test --test executor_integration -- --test-threads=1 --nocapture`
2. Co script rieng cho smoke e2e write-tx (de dev chay nhanh).
3. Bao cao ket qua theo mau: preconditions, tx hash, receipt status, ly do fail.

Tieu chi pass:

- 2-3 lan chay lien tiep deu cho ket qua on dinh, khong flaky.

## Checklist trien khai nhanh

1. Tao global lock va dung trong test write-tx.
2. Sua assert trong `test_execute_real_liquidation` de fail nghiem ngat.
3. Tach/ghi chu lenh run cho write-tx tests (threads=1).
4. Chay lai toan bo `executor_integration` it nhat 2 lan.

## Dinh nghia Done

- `test_execute_real_liquidation` fail dung khi giao dich that bai.
- Khong con nonce race khi chay full suite.
- Ket qua test phan anh dung chat luong execute that su tren fork.
