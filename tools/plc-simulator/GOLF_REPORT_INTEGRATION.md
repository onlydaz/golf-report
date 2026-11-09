# Chạy Golf Report với PLC Simulator

Tại thư mục gốc dự án, mở hai PowerShell.

Terminal 1:

```powershell
.\tools\plc-simulator\.venv\Scripts\python.exe -B .\tools\plc-simulator\plc_simulator.py
```

Nếu chưa có môi trường Python:

```powershell
python -m venv tools/plc-simulator/.venv
.\tools\plc-simulator\.venv\Scripts\python.exe -m pip install -r .\tools\plc-simulator\requirements.txt
```

Terminal 2:

```powershell
npm run tauri dev
```

PostgreSQL phải đang chạy và có `database_config.toml` theo cấu hình sẵn của
ứng dụng. Migration mới tự chạy khi backend khởi tạo kết nối. Migration chỉ thêm
`plc_sources` và `plc_inbox`; không thay đổi hay xóa các phép đo hiện có.

1. Vào **Ghi nhận kết quả đo**; đợi PLC hiển thị **Đã kết nối**.
2. Nhập mã sản phẩm, nhấn **Bắt đầu ghi nhận**.
3. Tại simulator, nhập `1`, `2` hoặc `3` để tạo kết quả mới.
4. Đợi **Đã lưu thành công** và mã bản ghi trước khi tạo kết quả tiếp theo.
5. Kiểm tra bản ghi trong Lịch sử đo. Tip BỎ QUA có khoảng cách và số lần kiểm tra lại NULL.
6. Để đổi sản phẩm, nhập mã mới rồi xác nhận trước khi tạo phép đo tiếp theo.

Nếu simulator có kết quả trước khi xác nhận mã, kết quả đó được dùng làm mốc
và không lưu với mã mới. Chỉ xác nhận lại khi cần bắt đầu hoặc đổi sản phẩm;
không cần nhấn lại cho từng gậy cùng sản phẩm.

## Cơ chế lưu và thử lại

- Rust chỉ gửi FC03, Unit ID 1, đọc 100–117; hai lần đọc cách nhau 30 ms phải giống nhau.
- Chu kỳ nghỉ giữa các lượt polling là 500 ms; timeout đọc 2 giây. Tự nối lại mỗi lượt đọc.
- Dữ liệu được giải mã và kiểm tra bằng `MeasurementService::validate`.
- Khoảng cách/weight dùng số thập phân chính xác chia 100; SKIPPED chuyển thành NULL.
- Inbox lưu cả mã sản phẩm và thời gian nhận. Bản ghi phép đo, bốn Tip và ID hoàn tất
  inbox được commit chung một transaction. Chỉ sau commit mới báo lưu thành công.
- Retry khóa hàng inbox, trả lại ID đã lưu nếu commit trước đã thành công. Không tạo bản sao.
- Sau khi mở lại ứng dụng, worker tự hoàn tất inbox còn chờ theo mã sản phẩm đã chụp.
  Cần xác nhận mã sản phẩm lại để nhận các phép đo mới.
- Khi PostgreSQL lỗi trước lúc ghi inbox, dữ liệu đã nhận được giữ trong RAM và tự thử lại;
  không tắt ứng dụng lúc này. Chỉ dữ liệu đã ghi inbox mới khôi phục được sau khi tắt ứng dụng.

## Cấu hình và giới hạn register map

`src-tauri/src/plc/config.rs` chứa địa chỉ, Unit ID, source, timeout và chu kỳ.
`src-tauri/src/plc/register_map.rs` chứa địa chỉ và vị trí các trường.
`source` là định danh thiết bị, cần giữ ổn định khi chỉ đổi IP; đổi sang thiết bị
khác cần định danh source khác.

Sequence 16-bit được lưu bền vững. Khi quan sát sequence giảm (bao gồm về 0 khi
simulator khởi động lại), tăng generation; khóa chống trùng là source/generation/sequence.
Nếu PLC reset hoặc chạy trọn vòng sequence trong lúc ứng dụng mất kết nối và quay lại
đúng sequence cũ, register map hiện tại không đủ thông tin phân biệt. PLC thật nên
bổ sung boot/session ID hoặc counter lớn hơn trước khi dùng sản xuất.

PLC hiện chỉ giữ một kết quả, không có hàng đợi hoặc ACK. Kết quả bị ghi đè giữa các
lượt polling không thể khôi phục. Trong thử nghiệm, chờ lưu xong rồi tạo kết quả tiếp theo.

## Kiểm thử

```powershell
npm run build
npm test
cd src-tauri
cargo check
cargo test --lib plc::decode
cargo test --lib plc::tests::simulator_and_postgres_end_to_end -- --ignored --nocapture --test-threads=1
```

Kiểm thử tích hợp dùng simulator riêng trên cổng **11502**, không sửa file Python.
Nó đọc cấu hình PostgreSQL hiện có, tạo schema tên `plc_test_<timestamp>`, chỉ ghi
vào schema đó và dọn schema khi kết thúc (kể cả assertion thất bại).
Tài khoản kiểm thử cần quyền tạo schema. Các bảng dữ liệu thật không bị thay đổi.
Kiểm thử bao gồm ba loại kết quả, SKIPPED NULL, không có mã/không nhận kết quả cũ,
chống trùng, retry sau lỗi INSERT, mở lại worker, mất kết nối và simulator reset.
