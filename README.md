# Golf Report

Ứng dụng Windows Tauri 2 + Rust + React, một laptop kết nối một PLC Delta AS200 qua Modbus TCP. Giữ nguyên các trang Giám sát, Lịch sử, Chi tiết, Báo cáo và Kết nối PLC.

## Luồng acquisition hiện tại

Một Worker đọc control ổn định. Khi DataReady=1, đọc riêng D25020–D25259, đúng 240 WORD; PLC tự quản lý buffer. Payload toàn 0, Status bit0=0 hoặc dữ liệu đo chưa hợp lệ vẫn được lưu nguyên trạng vào Inbox PostgreSQL. Không có chế độ TEST và không tạo Measurement giả.

Raw receipt và con trỏ bản ghi đang chờ được commit cùng transaction. UNIQUE `(source_id, publication_session, publication_count)` dùng TotalCount thật và phiên counter cấu hình; không hash payload. RecordSeq gốc, word order và SKU lúc nhận được giữ riêng. Payload giống nhau ở hai counter khác nhau tạo hai receipt khác nhau. Cùng định danh nhưng raw khác gây conflict, không ghi đè.

Chỉ payload giải mã và workflow hợp lệ mới được project thành Measurement để phục vụ UI/báo cáo. Lỗi projection không cấp hoặc thu hồi quyền ACK. Retry projection có chu kỳ riêng, xử lý batch nhỏ; raw không giải mã được có decode_error để tránh retry liên tục. SKU được đóng băng lúc nhận raw, không lấy SKU hiện tại để gán lại lịch sử.

## ACK và giới hạn phục hồi

PostgreSQL lưu một outstanding receipt, accepted/last_total, dấu Count có thể đã bắt đầu (`ack_attempted`), bằng chứng Done chưa bắt đầu/có thể đã gửi (`done_started`) và quan sát Done cao. `done_started=NULL` giữ nguyên sự chưa rõ của ACK cũ. Không có năm pha ACK trong DB.

Sau raw commit/read-back, Worker chuẩn bị socket Count, kiểm tra PLC, commit ack_attempted rồi gửi FC16 PC_ReadCount=TotalCount. Count được đọc lại trên cùng publication; socket Done được chuẩn bị trước khi commit done_started=true và gửi FC06 PC_ReadDone=1. Snapshot/heartbeat/stop được kiểm tra sát write. Response ghi không phải bằng chứng hoàn tất; vẫn cần Done cao rồi thấp, Count đúng target và Ready/counter tương thích trước khi tăng accepted.

Sau restart, attempted=true nhưng done_started=false cho phép gửi Done lần đầu khi Count read-back đúng, raw/identity không đổi và các guard đủ. Done true/NULL chỉ được quan sát, không resend. Count chưa rõ và chưa đọc lại tới target vẫn cần đối chiếu; bỏ lỡ Done cao cũng có thể cần kỹ thuật. Raw counter 0 được giữ nguyên và chỉ chuyển pending sang publication 1 khi DB chứng minh chưa attempted/không reset, Count=0/Done=0 và publication mới ổn định. Không đổi row 0 thành 1 hoặc tạo epoch mới.

Projection chia suất cho bản ghi mới và FAILED, có cursor quay vòng. Khi mở Worker, status được khôi phục từ receipt gần nhất/pending và Measurement trong DB; rawCapture mang trạng thái ACK của chính row đó, không mất ACKNOWLEDGED khi PLC idle.

## Cấu hình, kiểm tra và dừng

Dev đọc as200_config.toml ở root project; Release đọc cạnh EXE. Giữ SourceKey, confirmed_session_id và journal_path hiện có. Session chỉ là namespace counter được cấu hình rõ ràng; không đổi nó để vượt lỗi reset. journal_path hiện dùng để tìm bằng chứng cũ và khóa độc quyền tương thích binary cũ; Worker mới không ghi capture.json.

Kiểm tra kết nối chỉ mở TCP/FC03, không truy vấn hoặc ghi PostgreSQL, không tạo Worker và không ghi PLC. Kết nối và auto-launch dùng cùng Engine. Ngắt ghi marker dừng và join Worker; raw đã commit và ACK đang chờ nằm trong DB. Đổi SKU cần Worker đã dừng, không outstanding receipt và PLC idle khớp checkpoint.

Quyền ACK giữ opt-in trong TOML: technical_setup_confirmed, allow_plc_connection, ack_enabled, ack_protocol=as200, read_only=false và scope map phù hợp. Acquisition không ghi PC_Heartbeat. Công cụ thanh ghi kỹ thuật giữ policy riêng, không phải đường ghi ACK thay thế. Không đổi layout/routes/CSS hay công thức báo cáo.

## Chuyển đổi từ Journal v4

Migration `20261117120000_publication_receipts.sql` tạo cấu trúc publication. Migration tiếp theo `20261117130000_done_write_evidence.sql` bổ sung done_started, giữ NULL cho attempted cũ chưa rõ. Không sửa migration trước hoặc xóa row lịch sử; chưa chạy migration trong lượt sửa này.

Lần khởi tạo trạng thái mới đọc Journal cũ, không yêu cầu quyền ghi capture.json. Reader nhận sentinel source_id=0 của delivery cũ nhưng vẫn đối chiếu SourceKey/row/binding/session/raw/receipt/prefix. last_total=accepted+1 chỉ được giữ khi có receipt/intent ACK_CONFIRMED cũ hợp lệ và handshake publication kế tiếp tương thích. Không sửa file cũ. Pending ACK thật, lịch sử vượt checkpoint hoặc thiếu trạng thái ACK của backend mới vẫn chặn; không nhập remote Count làm accepted.

Đã xóa sáu module cũ `worker/{ack,capture,delivery,journal,recovery,retry}.rs` và kiểu ACK năm pha. Không có Worker cũ hoặc fallback. Chỉ `worker/legacy.rs` giữ reader cutover độc lập và `LegacyInboxInput` private để đối chiếu bằng chứng cũ; không chạy lại logic ACK cũ. Không xóa Journal, đổi Session, reset PLC hoặc chạy binary cũ sau khi đã chuyển sang trạng thái DB mới.

## Kiểm chứng

Sau cleanup, `cargo check` và `npm run build` đã thành công. Vite còn cảnh báo chunk lớn hơn 500 kB. Không chạy ứng dụng, test, migration, Simulator hoặc kết nối PLC/PostgreSQL; không sửa tests.rs. Test Worker, service Inbox và các test proof năm pha cũ cần chuyển sang API acquisition hiện tại trong lượt được cho phép riêng; build production thành công không xác nhận test target hoặc nghiệm thu máy thật. Chi tiết file/logic đã xóa và các dependency được giữ nằm trong `docs/PLC_BACKEND_REFACTOR.md`.

DB config vẫn đọc file bên ngoài binary; không log connection string hoặc credential. Dữ liệu raw chỉ được bảo đảm sau PostgreSQL commit; khi DB mất kết nối, PLC phải giữ publication cho đến ACK. Timing Done, retention, LOW/offset và hợp đồng publish slot 0 còn cần nghiệm thu với Ladder thực tế.
