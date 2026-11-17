# Golf Report

Ứng dụng Windows Tauri 2 + Rust + React đọc kết quả AS200 Published Record và lưu PostgreSQL.

## Một Worker, cấu hình môi trường qua TOML

PLC Delta AS200: **192.168.1.5:502** hoặc Python Simulator **127.0.0.1:1503**, Unit ID 1, Modbus TCP. Dừng Worker, thoát ứng dụng rồi sửa TOML và khởi động lại để đổi môi trường; không chuyển endpoint khi ứng dụng đang chạy. `as200_config.toml` là cấu hình hoạt động duy nhất; frontend lấy địa chỉ từ Rust và chỉ hiển thị read-only. Trang Kết nối PLC có **Kiểm tra kết nối**, **Kết nối/Ngắt kết nối**. Không còn chế độ sửa endpoint, draft, chuyển nguồn hoặc đăng ký Serial.

- Kiểm tra chỉ mở TCP và đọc FC03/snapshot AS200; không tạo Worker, Journal, truy vấn DB hoặc ghi PLC.
- Kết nối dùng cấu hình hiện có, kiểm tra map và khởi động một Worker.
- Ngắt dừng/join Worker, thử flush dữ liệu và giữ Journal khi DB lỗi. Sai cấu hình không ngăn việc dừng Worker đã chạy.
- Công cụ thanh ghi tùy chọn vẫn có guard riêng và mặc định ẩn; chỉ cho phép endpoint PLC cố định, không cấp quyền ACK Worker.

## Cấu hình và runtime

Dev dùng root project; NSIS/MSI Release dùng thư mục EXE. Nếu thiếu config và không có dấu vết lịch sử cục bộ, backend tạo cấu hình theo `as200_config.example.toml`: **enabled=false, read_only=true, ack_enabled=false**. Không tạo receipt hoặc checkpoint lịch sử giả. File ví dụ cũng là template mặc định, không chứa bí mật DB.

PLC dùng SourceKey `golf-report-plc-01`, session ban đầu `golf-report-plc-01-initial`, Journal `local-runtime/capture.json` (v4). Simulator loopback dùng SourceKey `golf-report-simulator`, session bắt đầu `simulator-` (ví dụ `simulator-test-01`), Journal `local-runtime/simulator/capture.json`. Mỗi Journal có khóa riêng. Session PLC không dùng tiền tố `simulator-`. Các ràng buộc này ngăn cấu hình Simulator dùng nhầm trạng thái PLC. Không suy danh tính vật lý từ IP. Session không đổi khi mất mạng/restart. Journal có binding, raw, receipt, retry và intent; không sửa Session/SourceKey để vượt reset hoặc thiếu dữ liệu.

Lần tạo Journal đầu tiên đòi hỏi PostgreSQL không có lịch sử của SourceKey đang cấu hình chưa đối chiếu, control TotalCount/PC_ReadCount/Done/DataReady=0 và vùng công bố rỗng, ổn định. Thiếu Journal nhưng DB/cục bộ còn lịch sử thì chặn, không nhập counter làm checkpoint. Journal tồn tại phải qua kiểm tra version/binding/receipt/DB prefix. RESET giữ pause và bằng chứng, cần kỹ thuật đối chiếu; không có nút reset hay tự mở phiên mới.

Runtime chỉ ghi trong thư mục ứng dụng **có quyền ghi**; không lưu vào thư mục chỉ đọc và không fallback AppData/ProgramData. Installer ACL hiện có cho phép tạo config/runtime, không cấp Everyone quyền sửa binary. Startup kiểm tra write/sync/rename/reparse point; không đủ quyền thì không thu/ACK. Với thư mục cài tùy chọn, kỹ thuật phải cấp quyền phù hợp cho tài khoản vận hành.

## Chạy Python Simulator

Dùng `as200_config.example.toml` làm tham chiếu, giữ các tham số map/quyền hiện tại; chỉ thay nhóm môi trường trong `as200_config.toml` khi ứng dụng đã thoát:

| Trường | Simulator | PLC thật |
| --- | --- | --- |
| `mode` | `"modbus"` | `"modbus"` |
| `address` | `"127.0.0.1:1503"` | `"192.168.1.5:502"` |
| `unit_id` | `1` | `1` |
| `source_key` | `"golf-report-simulator"` | `"golf-report-plc-01"` |
| `confirmed_session_id` | `"simulator-test-01"` | `"golf-report-plc-01-initial"` |
| `journal_path` | `"local-runtime/simulator/capture.json"` | `"local-runtime/capture.json"` |

Session trong bảng chỉ dành cho môi trường mới sạch. Khi quay lại môi trường đã có lịch sử, giữ chính xác Session/Journal cũ. Không sửa binding hoặc đổi Session để vượt RESET. Không sao chép Journal giữa hai môi trường. Không tự nâng quyền ghi: mặc định `read_only=true`, `ack_enabled=false`; điều kiện ACK ở mục bên dưới vẫn bắt buộc. Muốn thử chuỗi nhiều gậy, kỹ thuật phải chủ động cấp quyền ACK cho cấu hình Simulator đã kiểm tra; chế độ chỉ đọc sẽ giữ bản ghi đầu chờ ACK.

Hai cửa sổ PowerShell, từ thư mục project `golf-report`:

```powershell
# Cửa sổ 1: dùng virtualenv hiện có của Simulator
& "..\as200-simulator\.venv\Scripts\python.exe" "..\as200-simulator\as200_simulator.py" --published-record --host 127.0.0.1 --port 1503 --unit 1 --offset 0 --word-order low --ack-delay-ms 1000
```

```powershell
# Cửa sổ 2: chỉ chạy sau khi đã kiểm tra TOML và cấu hình database TEST
npm run tauri dev
```

Lần đầu, để Simulator ở baseline rỗng rồi nhấn **Kiểm tra kết nối → Kết nối**; sau khi Worker sẵn sàng mới dùng lệnh Simulator `1` (OK), `2` (TIP NG), `3` (BUTT NG). Fixture TIP chỉ có P1; BUTT NG tại P2, P3/P4 SKIPPED; mẫu OK có đủ bốn điểm BUTT. Lượt chưa thực hiện không được tạo dữ liệu. Published Record giữ vùng Bản ghi 1 đến khi nhận Count/Done hợp lệ.

Simulator khởi động lại sẽ trở về counter 0; Journal đã có lịch sử có thể chặn RESET đúng thiết kế. Không xóa Journal hoặc tạo Session mới để né lỗi. Dùng database TEST phù hợp trước khi chạy app; các lệnh trên không tự chuyển database khỏi Neon. Việc thử Simulator không xác minh Ladder/ACK trên PLC thật.

## Chuyển từ phiên bản nhiều nguồn

Không tự chuyển Journal hay đổi binding. Giữ nguyên config, `managed-sources`, `endpoint-links`, ACK intent/checkpoint và dữ liệu Neon cũ. Thiếu config trong khi còn runtime cũ cũng không tự tạo cấu hình thay thế. Cần dừng ứng dụng, sao lưu và đối chiếu lịch sử trước khi dùng cấu hình một PLC; không đổi version JSON hay tạo session để né lịch sử. Bản refactor chủ động chặn khởi tạo nếu chưa chứng minh được an toàn.

Code không còn dùng `plc_source_registry` để chọn nguồn hoặc cấp ACK. Bảng/migration cũ được giữ nguyên để bảo toàn dữ liệu; việc bỏ bảng chỉ nên là migration riêng sau kiểm kê, không DROP trực tiếp hoặc sửa migration đã triển khai. `plc_sources` và Inbox vẫn cần cho chống trùng/dữ liệu lịch sử.

## Xác minh ACK

Chế độ chỉ đọc có thể lưu bản ghi nhưng PLC giữ nó đến khi có ACK hợp lệ. Để sử dụng ACK, kỹ thuật phải xác minh Ladder/protocol và quyền ghi trước khi tự sửa config: `technical_setup_confirmed=true`, `read_only=false`, `ack_enabled=true`, `allow_plc_connection=true`, `ack_protocol="as200"`; vẫn chịu ACK scope và toàn bộ guard v4. Không bật chỉ vì TCP/FC03 PASS.

Giữ hai buffer 240 WORD do PLC quản lý, PC đọc Bản ghi 1; raw vẫn có bốn slot mỗi trạm, nghiệp vụ TIP chỉ P1 và BUTT P1–P4, mỗi điểm tối đa bốn lượt. DecideAttempt và kết quả PLC được giữ nguyên. Count chỉ sau receipt PROCESSED/transaction commit/read-back, Done theo pha durable; uncertain không tự phát lại. Thiếu setpoint không tạo giới hạn giả. WordOrder LOW, offset 0, timing và hành vi FIFO/Done phải nghiệm thu trên PLC thật.

## Database, phát triển và kiểm thử

DB Dev đọc `src-tauri/resources/database_config.toml`; Release đọc `resources/database_config.toml` cạnh EXE. Không nhúng mật khẩu vào Rust binary, không log connection string; resource trong installer vẫn có thể bị đọc nên phải kiểm soát credential phân phối.

Các lệnh: `npm test`, `cargo check --manifest-path src-tauri/Cargo.toml --locked`, `cargo test --manifest-path src-tauri/Cargo.toml --locked --lib`, `npm run build`. Test thường dùng mock loopback/tempdir; các test DB opt-in/ignored phải được xem xét riêng, không chạy trên Neon. Không chạy ứng dụng thật để kiểm tra code khi chưa kiểm soát config/PLC. `npm run tauri dev` khởi chạy ứng dụng, có thể truy cập DB/PLC theo cấu hình; đóng gói bằng `npm run tauri build` chỉ khi có yêu cầu.
