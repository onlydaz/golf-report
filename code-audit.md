# Golf Report — rà soát source code tĩnh

Ngày rà soát: 11/11/2026. Phạm vi: `src/`, `src-tauri/src/`, manifest dependency, cấu hình Tauri, migration và các tham chiếu trong dự án. Chỉ đọc source; không chạy ứng dụng, Simulator, compiler, build hoặc test. Repo hiện có nhiều thay đổi chưa commit, nên các nhận xét áp dụng cho **trạng thái file hiện tại**, không quy kết thay đổi cho một commit cụ thể. Không thay đổi source code, cấu hình vận hành hay database trong lần rà soát này.

## Tổng quan

Luồng chính vẫn có đường gọi rõ ràng: `src/App.tsx` khai báo các route; `src-tauri/src/lib.rs:33` đăng ký Tauri commands; `worker.rs` khởi chạy reader, heartbeat và sink tại `launch_under_gate` (khoảng dòng 2400). Repository, service, Inbox, Journal, ACK và recovery được dùng trực tiếp hoặc qua callback/task/serialization. Không có bằng chứng để xóa một cơ chế bảo vệ PLC chỉ vì ít gặp. Không phát hiện dependency runtime nào có thể khẳng định là dư bằng tìm kiếm tĩnh.

Các file cần chú ý về khả năng bảo trì: `worker.rs` khoảng 2.500 dòng, `connection_settings.rs` khoảng 760 dòng, `Reports.tsx` khoảng 555 dòng và `AddMeasurement.tsx` khoảng 507 dòng. Test của Worker khoảng 2.452 dòng nhưng là test riêng, không phải code runtime dư. `Cargo.lock` và icon nhị phân không tính là source cần tách.

## A — Có thể xóa an toàn sau khi duyệt

| Vị trí / symbol | Bằng chứng | Ảnh hưởng và đề xuất |
|---|---|---|
| `src/assets/react.svg`, `public/vite.svg`, `public/tauri.svg` | Không có tham chiếu tên file trong `src/`, `index.html`, cấu hình hoặc tài liệu; `index.html` chỉ nạp `/src/main.tsx`. Đây là asset mẫu. | Không ảnh hưởng UI theo đường gọi hiện tại. Có thể xóa các asset mẫu sau khi xác nhận không có tài liệu ngoài repo liên kết trực tiếp URL công khai của chúng. |
| `src/pages/ConnectionSettings/ConnectionSettings.css:19`, `.plc-settings-choice` | Không có class này trong TSX hoặc file khác; UI hiện dùng `.plc-settings-actions`. | Chỉ một rule CSS không áp dụng. Có thể xóa rule, không thay chức năng. |

Không xác nhận thêm unused import/variable/function cụ thể bằng độ tin cậy tương đương. Tìm kiếm text không thay thế được TypeScript/Rust compiler, đặc biệt với macro Tauri, serde, callback và test.

## B — Có thể tối ưu, chưa nên xóa hành vi

| Vị trí / symbol | Nhận xét, tác động và đề xuất |
|---|---|
| `src-tauri/src/plc/worker.rs:687–891`, `1157–1543`, `1566–2009`, `2010–2341` | Một file chứa capture/quarantine, sink/retry, vòng polling/ACK và kiểm tra chuyển nguồn. Đây là ranh giới trách nhiệm rõ nhất để cân nhắc tách **nội bộ module hiện có** khi có nhu cầu sửa tiếp. Ưu tiên tách theo nhóm chức năng sau khi có kiểm chứng hành vi; không đổi Journal format hay thứ tự persist → ACK. Rủi ro refactor cao. |
| `worker.rs:1737–2009` và `2095–2277` | `ack_step` và `verify_resume` dài, nhiều guard lồng nhau. Cách dựng ACK binding trong `ack_step` và `verify_resume` lặp lại; hai nơi phải giữ đúng cùng format `mode|address|unit|source|session|offset|word_order|as200`. Nếu sửa format ở một nơi sẽ chặn quay lại nguồn cũ. Có thể gom **cách dựng chuỗi** khi sửa liên quan, nhưng phải giữ khả năng đọc Journal/binding cũ. |
| `worker.rs:2120–2153` và `982–1054` | Kiểm tra hai snapshot FC03 và kiểm tra snapshot trong `reconcile_session` đều so control/buffer nhưng danh sách trường không hoàn toàn giống nhau. Nên ghi rõ chính sách so sánh theo từng luồng, chỉ hợp nhất nếu chứng minh cùng yêu cầu. Không so heartbeat thay đổi liên tục. |
| `worker.rs:2010–2080` và `2095–2277`; `connection_settings.rs:514–680` | Kiểm tra nguồn đóng, rồi kiểm tra lại Journal/snapshot trong `verify_resume`; điều này có mục đích khóa và chống thay đổi nhưng khiến luồng chuyển nguồn dài và khó theo dõi. Nên tài liệu hóa thứ tự/ownership của lock trước; không bỏ một lần kiểm tra chỉ vì thấy trùng. |
| `src/pages/AddMeasurement/AddMeasurement.tsx:196–378` | Một component gộp polling Worker/settings, Start/Stop/reconnect, diễn giải cảnh báo, retry Inbox và tải chi tiết phép đo. `displayNotice` cùng nhiều nhánh `systemNotice` làm nội dung lỗi khó kiểm soát; có thể tách hàm thuần trình bày trạng thái khi chỉnh UI tiếp. Không tách lệnh hoặc đổi trạng thái Worker. |
| `src/pages/Dashboard/Dashboard.tsx:72–145`, `Reports/Reports.tsx:101–239`, `MeasurementHistory/MeasurementHistory.tsx:37–91` | Ba trang tự quản lý request version, loading và dữ liệu bất đồng bộ theo cách khác nhau. Chỉ nên chuẩn hóa một helper rất nhỏ nếu tiếp tục gặp lỗi stale response; hiện hoạt động khác mục đích (Dashboard thống kê, Reports áp dụng bộ lọc/xuất, History polling 3 giây), không ép dùng chung hook lớn. |
| `src/pages/MeasurementHistory/MeasurementHistory.tsx:28–67` | `data`/`total` được giữ song song với `dataRef`/`totalRef`, so bằng `JSON.stringify` mỗi lần poll. Đây là cache để tránh cập nhật bảng không đổi, không phải dead code; có thể cân nhắc so khóa/các trường cần thiết nếu hiệu năng thực tế thành vấn đề. Việc thay đổi cần giữ nguyên chống response cũ bằng `requestVersion`. |
| `src/pages/Reports/Reports.tsx:67–239` | Trạng thái bộ lọc nháp, bộ lọc đã áp dụng, tải thống kê và xuất CSV/XLSX cùng nằm trong trang. Có thể tách hàm thuần tạo request/xuất file nếu tiếp tục phát triển; không đổi semantics khoảng ngày Việt Nam hoặc nội dung file. |
| `src-tauri/src/plc/as200/mod.rs:2,55–63` | `#![allow(dead_code)]` áp trên toàn module che cảnh báo unused; `PlcAcknowledgement`, `DisabledAcknowledgement`, `As200Error::AckDisabled` hiện chỉ được tham chiếu bởi test (`as200/tests.rs:393`). Comment TODO về “trước khi hiện thực ghi” không còn phản ánh `as200/ack.rs` hiện có. Có thể thu hẹp allow và cập nhật comment trong một thay đổi riêng; **chưa xóa API test-only** trước khi xác nhận mục đích tương thích PLC thật. |

## C — Phải giữ

| Vị trí | Lý do |
|---|---|
| `worker.rs:211–264`, `connection_settings.rs:514–680` | Dừng Worker, flush và khóa/marker `operator-stop`, `connection-pending` tạo ranh giới chuyển nguồn và phục hồi khi thao tác bị gián đoạn. Xóa marker hoặc bỏ guard có thể khởi động sai nguồn. |
| `worker.rs:390–554`, `687–891`, `1157–1255` | Journal persist, capture raw rồi lưu Inbox là đường chống mất dữ liệu và chống trùng qua restart. `session_archives`, quarantine, warnings và các field serde có thể cần đọc Journal cũ dù ít xuất hiện ở UI. |
| `worker.rs:1257–1543`, `worker/retry.rs`, `repositories/inbox_repository.rs` | Retry giữ Inbox ID, đọc lại commit sau lỗi chưa rõ, xử lý pending và quarantine. Không xóa retry hoặc coi lỗi SQL là đã xử lý. |
| `worker.rs:1737–2009`, `as200/ack.rs` | ACK intent bền vững, checkpoint và xác minh handshake bảo đảm không ACK trước khi raw có receipt bền vững; vẫn cần khi ACK được bật. |
| `worker.rs:2010–2277`, `connection_settings.rs:357–513` | Tìm profile endpoint cũ, đối chiếu SourceKey/Session/Journal/Inbox/counter/buffer, xử lý reset có xác nhận. Nhánh ACK sạch không yêu cầu raw đã giải phóng còn valid nhưng vẫn kiểm tra prefix receipt và hai snapshot; không được rút xuống chỉ so counter. |
| `src-tauri/migrations/20261109120000_initialize_as200.sql` và `database/migrations.rs` | Migration/schema là hợp đồng với dữ liệu đã lưu. Không xóa/sửa chỉ vì không có lời gọi trực tiếp trong React. |
| `src/utils/as200Export.ts`, `src/utils/reportTime.ts`, `MeasurementDetails.tsx` | Đang dùng trong báo cáo/hiển thị chi tiết; `exceljs`, `xlsx`, plugin dialog/fs, `urlencoding`, `recharts`, Ant Design và Tauri plugins có tham chiếu runtime hoặc test rõ ràng. |

## D — Chưa chắc chắn, cần xác minh trước khi xóa

| Vị trí / symbol | Bằng chứng hiện tại và việc cần kiểm tra |
|---|---|
| `src/services/measurementService.ts:118`, `commands/measurement_command.rs:8`, `lib.rs:34`: `checkDatabaseConnection` | Không có caller React hiện tại; lệnh vẫn được đăng ký trong Tauri. Có thể là API cho công cụ/phiên bản frontend khác. Kiểm tra consumer bên ngoài và chính sách API trước khi xóa cả service wrapper lẫn command. |
| `src/services/plcConnectionService.ts:18`, `connection_settings.rs:708`, `lib.rs:47`: `updatePlcConnectionSettings` | UI hiện gọi `connectPlcCollection`, không gọi wrapper update. Nhưng đây là Tauri command được đăng ký và có đường lưu cấu hình riêng; kiểm tra công cụ tích hợp/khả năng setup ban đầu trước khi quyết định. |
| `worker.rs:2453`, `lib.rs:49`: `get_as200_retry_diagnostics` | Không có `invoke` tương ứng trong React; vẫn là command phục vụ chẩn đoán. Xác minh quy trình hỗ trợ kỹ thuật trước khi loại bỏ. |
| `src-tauri/src/bin/as200-reader.rs`, `as200-db.rs` | Binary CLI riêng, không đi qua Tauri route/`generate_handler!`. Có thể phục vụ kỹ thuật chẩn đoán; xem quy trình phát hành/đóng gói rồi mới quyết định. |
| `src-tauri/src/plc/as200/mod.rs:55–63` | ACK interface test-only nêu ở B. Không đủ bằng chứng để kết luận bỏ an toàn; cần quyết định liệu đây còn là hợp đồng phát triển với PLC thật. |
| `src-tauri/src/plc/worker.rs:31` và các `PLC_SWITCH`/`eprintln!` | Có vẻ nhiều log, nhưng mã trace/guard đang hỗ trợ chẩn đoán chuyển nguồn; **không phải debug tạm có thể xóa ngay**. Chỉ giảm khi đã có logging tương đương và quy trình thu log release. |

## Mức độ rủi ro và thứ tự ưu tiên

1. **Thấp:** sau khi duyệt, bỏ asset mẫu và `.plc-settings-choice`; không động tới logic.
2. **Thấp–vừa:** xác nhận consumer của ba Tauri commands ở mục D; giữ command cho đến khi có bằng chứng không còn tích hợp ngoài frontend hiện tại.
3. **Vừa:** cập nhật comment ACK cũ, thu hẹp `allow(dead_code)` có chọn lọc sau khi có compiler diagnostics trong một giai đoạn được phép chạy check.
4. **Cao:** chỉ khi cần bảo trì luồng PLC tiếp theo, tách các phần thuần/nội bộ của `worker.rs` và chuẩn hóa ACK binding/snapshot theo từng luồng. Trước refactor phải có kịch bản kiểm chứng restart, ACK intent chưa rõ, buffer gap, quay lại nguồn cũ và reset counter; không thay Journal/database format.

Chưa có kết luận chắc chắn về unused import/variable toàn dự án hoặc dependency dư nếu không chạy compiler/linter. Phân loại A chỉ gồm các mục có bằng chứng không còn tham chiếu trong source hiện tại; các API gián tiếp và cơ chế bảo vệ được đặt ở C/D.
