# Golf Report — Review sau refactor backend PLC

Ngày review: **17/11/2026, giờ Việt Nam**. Review working tree hiện tại, gồm thay đổi chưa commit. Chỉ tạo báo cáo này; không sửa production/UI/config/migration/tests.rs, không chạy test/build/app/Simulator/migration và không kết nối PLC/PostgreSQL.

## 1. Kết luận và phạm vi bằng chứng

**Luồng raw → PostgreSQL commit → ACK đã được tách khỏi chất lượng Measurement. Tuy nhiên chưa nên tiếp tục xóa source cũ trước khi sửa lỗi cutover và chốt cách phục hồi ACK.** Có lỗi tương thích Journal sạch trong `legacy.rs`; một dấu `ack_attempted` cho cả Count/Done làm mất thông tin cần thiết để tiếp tục nhiều lần gửi bị ngắt. Đó là đánh đổi khả năng tiếp tục chạy, không phải bằng chứng app đang ACK trước commit.

Các nhãn sử dụng:

- **Đã đáp ứng**: thấy cơ chế phù hợp trong source, không có nghĩa đã nghiệm thu runtime.
- **Chưa đáp ứng**: thiếu đường xử lý hoặc có nhánh code ngăn mục tiêu đã nêu.
- **Có nguy cơ lỗi**: chỉ rõ điều kiện kích hoạt và hậu quả; chưa khẳng định điều kiện đó đã xảy ra ở nhà máy.
- **Cần xác minh runtime**: phụ thuộc Ladder, timing, trạng thái DB/Journal thật hoặc biên dịch. Không thay nguồn bằng lời mô tả trong báo cáo refactor.

FACT là chi tiết thấy trực tiếp trong source. INFERENCE là hệ quả theo nhánh code, chưa chạy tái hiện. Không thu thập log runtime và không đọc credential DB. Số dòng là của working tree tại thời điểm review.

Đã đọc `docs/PLC_BACKEND_REFACTOR.md`; bốn module Worker mới; transport ACK, reader, decoder/map/heartbeat/config; luồng kết nối và startup; Inbox service/repository/model; ba migration và migration runner; các đường UI/service nhận Worker/raw/Measurement. Đọc thêm các đoạn source Worker cũ để kiểm tra định dạng dữ liệu legacy, không kích hoạt chúng.

### Tổng hợp phân loại

| Nội dung | Phân loại | Kết luận |
|---|---|---|
| Nhận raw toàn 0 hoặc Status bit0=0 | Đã đáp ứng có điều kiện | Payload không cản insert/ACK; control phải đọc ổn định và Engine phải vượt gate lịch sử. Chỉ nhận publication mới khi chưa có pending receipt. |
| Commit raw trước mọi ghi ACK | Đã đáp ứng | Raw và pending pointer cùng transaction; marker ACK là thao tác DB riêng sau read-back. |
| Không tạo Measurement giả | Đã đáp ứng | Projection có validation riêng; invalid raw không đi vào Measurement/KPI. |
| Chống trùng bằng identity PLC | Đã đáp ứng có điều kiện | UNIQUE nguồn/phiên/counter, không hash payload; phụ thuộc PLC không tái sử dụng counter trong cùng epoch. |
| Test kết nối chỉ đọc | Đã đáp ứng đối với DB/PLC write | Không lấy pool hoặc gọi receiver; vẫn có kiểm tra quyền/thư mục cục bộ qua `ready`. |
| Một Worker trong cùng cấu hình | Đã đáp ứng trong phạm vi này | Một task, gate và OS lock cùng capture.lock. Không phải lease độc quyền toàn máy cho mọi bản sao thư mục. |
| Worker Journal cũ không chạy production | Đã đáp ứng | Không còn module declaration/call graph tới sáu module cũ; còn gate đọc legacy và một số ràng buộc filesystem. |
| Cutover Journal v4 sạch đã ACK | Chưa đáp ứng | So sánh `input.source_id` không tương thích sentinel 0 của source cũ. |
| Tự phục hồi mọi `ack_attempted=true` | Chưa đáp ứng | Có nhiều trạng thái không thể tự kết thúc dù DB/raw còn nguyên. |
| Tiếp tục sau raw counter 0 chuyển sang counter 1 | Chưa đáp ứng | Receipt 0 chiếm pending pointer, không được thay bằng publication 1. Không được giải bằng cách đổi 0 thành 1. |
| Measurement mới luôn lên UI | Có nguy cơ lỗi | Retry batch có thể đói dữ liệu mới; status không được khôi phục đủ sau restart. Query báo cáo vẫn đọc Measurement bình thường. |
| Migration / Ladder / timing thực tế | Cần xác minh runtime | Chưa có kết quả chạy migration, compiler hoặc trace PLC. |

## 2. Phát hiện cần ưu tiên

### F1 — P1 — Cutover từ Journal sạch bị chặn bởi Source ID sentinel

**Phân loại: Chưa đáp ứng. FACT về source, INFERENCE về tác động triển khai.**

`worker/legacy.rs:153–154` yêu cầu cả `row.source_id == source` và `input.source_id == source`. Nhưng Worker cũ tạo cả raw delivery và measurement delivery với `source_id: 0` (`worker/capture.rs:45,362`). Khi lưu DB, `worker/delivery.rs:158–159` clone input rồi gán ID thật **chỉ cho bản sao**; phần cập nhật delivery trong Journal sau đó chỉ gán `inbox_id` và `terminal`, không sửa input.source_id. Khi xác nhận ACK, `worker/ack.rs:378` giữ nguyên `delivery.clone()` làm confirmed_receipt.

Vì vậy một Journal hoàn toàn bình thường có thể chứa `confirmed_receipt.input.source_id=0`, trong khi DB source ID dương. Gate mới từ chối trước khi kiểm tra xong anchor/prefix. Không thể coi lỗi này là “ACK cũ chưa rõ” chỉ vì thông báo chung nói vậy. Không khẳng định file Journal thực tế của máy đang có cấu trúc nào vì lượt này không đọc runtime.

**Thay đổi nhỏ nhất:** reader legacy nhận đúng sentinel 0 của định dạng cũ, đồng thời vẫn chứng minh nguồn bằng SourceKey → source row, Inbox ID thật, binding/session, key/raw/timestamp và prefix. Nếu input có ID khác 0 thì kiểm tra khớp DB. Không sửa JSON cũ, không đổi nguồn/phiên và không bỏ proof receipt.

### F2 — P1 — Marker chung làm kẹt cả trường hợp chưa gửi ACK

**Phân loại: Chưa đáp ứng về khả năng phục hồi; Đã đáp ứng về không replay mù.**

`acquisition.rs:393` commit `ack_attempted=true`, rồi mới gọi `writer.count` tại `:399`. `AckWriter::count` mở TCP bên trong `send` (`as200/ack.rs:153,206–213`). App chết hoặc connect Count thất bại trong khoảng này có thể để DB attempted=true dù PLC chưa nhận byte lệnh ghi nào.

Vòng sau, `acquisition.rs:349` đi thẳng `observe_ack`. Hàm này không có đường gửi Count hoặc Done; chỉ có đường thấy Done cao rồi thấp để hoàn tất (`:243–254`). Nếu PLC vẫn chờ Count/Done, nó không tự tạo chuyển mức này. Chờ lâu hơn, reconnect hoặc restart không bổ sung được bằng chứng đã mất. Đóng app sau Count nhưng trước Done cũng mắc cùng vấn đề.

**Kẹt ở đây là không có đường tự thoát trong code**, không nhất thiết thread bị deadlock. Worker vẫn poll, UI vẫn có thể hiển thị lỗi và projection vẫn có thể chạy. Chi tiết phương án tối thiểu ở mục 7.

### F3 — P1/P2 — Quan sát Done yêu cầu raw cũ ổn định trong cả một snapshot

**Phân loại: Có nguy cơ lỗi; Cần xác minh runtime.**

Không chỉ yêu cầu Done=1. `observe_ack` tại `acquisition.rs:238–244` còn yêu cầu TotalCount=target, Ready=1 và raw trùng receipt. Reader dùng snapshot stable control/raw với 9 FC03 khi Ready=1 (`as200/reader.rs:225`, chia block ở `:166–178`). Nếu PLC xóa raw/Ready hoặc công bố kế tiếp ngay khi Done cao, PC có thể không có một snapshot thỏa predicate này, dù từng đọc được một control đơn lẻ có Done=1. Nếu PLC xóa Done trong một scan thì cửa sổ còn hẹp hơn.

Sau `done_prepared(...).await?`, mất response còn khiến Worker bỏ qua lần đọc ngay sau ghi (`acquisition.rs:416–417`); phải chờ vòng kế tiếp. Khi Done đã thấp và `done_observed=false`, Count đúng + Ready=0 vẫn không được code coi là hoàn tất.

Không kết luận guard này sai khi chưa có Ladder. Nhưng báo cáo refactor cần nói rõ yêu cầu là **snapshot raw cũ + Done cao cùng ổn định**, không chỉ “PC nhìn thấy Done cao”. Chỉ đổi proof sau khi xác nhận PLC giữ/latch trạng thái nào và trường nào chứng minh đã tiêu thụ đúng target.

### F4 — P2 — Batch projection có thể chặn dữ liệu hợp lệ mới khỏi thống kê vô hạn

**Phân loại: Có nguy cơ lỗi với trigger cụ thể.**

`project_pending` (`acquisition.rs:202`) luôn lấy 10 row cũ nhất, không loại FAILED đang chờ retry và không có cursor/backoff cho từng row. Nếu 10 row đầu decode được nhưng `InboxService::process` liên tục thất bại, chúng giữ `measurement_id=NULL`, `decode_error=NULL` và tiếp tục đứng đầu batch. Row hợp lệ thứ 11 không được chọn. Đây không chỉ là tối ưu hiệu năng: raw mới có thể ACK xong nhưng không bao giờ có Measurement trong khi nhóm lỗi cũ chưa được giải quyết.

Invalid decode được gắn decode_error nên thường rời queue; trigger trên là nhóm qua được preflight nhưng lỗi khi tạo/commit Measurement. `retry_max_attempts` không còn giới hạn nhóm này. Count blocked/waiting hiện cũng chỉ là ảnh chụp tối đa 10 row trước xử lý, không phải tổng backlog toàn DB.

**Thay đổi nhỏ nhất:** dành suất riêng cho raw/PENDING mới và FAILED; chọn FAILED theo lượt công bằng bằng cursor nhỏ hoặc thời điểm retry. Không cần khôi phục retry Journal/generic trait. Đồng thời cập nhật diagnostics sau xử lý thay vì giữ số đếm cũ. Chỉ ưu tiên PENDING mà không quay vòng FAILED chưa giải hết vấn đề đói hàng.

### F5 — P2 — Raw và status không được khôi phục đầy đủ sau restart

**Phân loại: Có nguy cơ lỗi hiển thị; không phải mất dữ liệu PostgreSQL.**

`Engine::open` (`acquisition.rs:66–76`) kiểm tra anchor nhưng không gọi show_receipt cho anchor hoặc pending row. Có hai tình huống rõ:

1. Raw đã ACK, đã có decode_error, pending đã được giải phóng. Restart: poll idle không show anchor, projection loại row có decode_error. `rawCapture` trên Giám sát vẫn rỗng; Lịch sử raw vẫn query ra row này.
2. Pending raw có blocked_reason. Restart: `acquisition.rs:295` trả lỗi trước `.receipt/show_receipt` tại `:340`. Nếu projection cũng bỏ qua do decode_error đã có, alert raw không được khôi phục dù pending row vẫn tồn tại.

**Thay đổi nhỏ nhất:** sau xác minh binding, khôi phục status từ pending receipt nếu có, nếu không thì từ anchor/latest receipt phù hợp; sau đó áp lại trạng thái blocked/uncertain. Khôi phục hiển thị không được xóa block hoặc cấp quyền ACK. `lastSavedId` cũng cần khôi phục khi row đã có Measurement để UI có tín hiệu nhất quán.

### F6 — P2 — Counter 0 chiếm hàng chờ và không nhận publication hợp lệ kế tiếp

**Phân loại: Chưa đáp ứng về tiếp tục thu sau tình huống khởi đầu này.**

`store.rs:79` đặt receipt counter 0 vào pending_inbox_id. `acquisition.rs:288` chỉ nhận raw khi pending đang rỗng. Khi PLC về sau công bố counter 1, Worker vẫn giữ row target=0 rồi trả lỗi ở điều kiện target<=0 trước khi so raw (`:356–365`). Không ghi row counter 1, không chuyển pending và không ACK. Bật/tắt kết nối không đổi điều này.

Không ACK counter 0 là đúng với yêu cầu không tạo identity giả. Nhưng giữ một quan sát chưa có publication identity như một **outstanding ACK receipt bắt buộc hoàn tất** làm chặn cả publication sau đã có identity. Nếu raw tại counter 0 đổi từ toàn 0 sang chỉ D25200=12 trong cùng lần chạy, cũng chỉ row chụp trước được giữ; nhánh target<=0 che cả đối chiếu thay đổi raw phía sau. Đây là giới hạn cụ thể của câu “chấp nhận mọi payload”.

**Hướng nhỏ, có điều kiện:** sau khi chốt PLC counter 1 là publication mới, có thể giữ row 0 như quan sát chưa xác nhận và atomically gắn pending sang row 1 mới, chỉ khi accepted=0, pending target=0, chưa attempted/Done observed, không reset/block và Count=0/Done=0/snapshot mới ổn định. Không relabel row 0 thành 1, không xóa row 0, không tăng accepted để bỏ qua ACK và không áp dụng khi counter đã reset từ lịch sử dương. Đây là đề xuất cần hợp đồng PLC, chưa phải guard được phép bỏ ngay.

### F7 — P2 — Legacy gate còn loại các snapshot sạch khác và vẫn yêu cầu quyền ghi Journal

**Phân loại: Có nguy cơ lỗi/chưa đơn giản hóa hết.**

- `legacy.rs:137` chỉ nhận last_total=accepted (hoặc NULL khi accepted=0). Nhưng code cũ có thể xác nhận ACK k nhờ snapshot đã công bố k+1; `worker/ack.rs:380` lưu last_total từ snapshot đó trong khi accepted=k và delivery k đã compact. Đây là Journal hợp lệ theo đường cũ, chưa có intent cho k+1, nhưng gate mới vẫn từ chối. Chỉ nới sau khi đối chiếu PLC/prefix; phải mang theo last_total cũ để không quên counter cao đã quan sát.
- `lib.rs:77–81` vẫn mở capture.json với quyền write trong check_plc_directory. Một archive cũ chỉ đọc có thể chặn cả startup mới dù DB progress đã tồn tại và acquisition không ghi Journal nữa. Đây là phụ thuộc filesystem cũ, **không phải Worker cũ vẫn chạy**.
- Gate chặn mọi pause/retry/quarantine và file lạ trong thư mục runtime (`legacy.rs:35–65,109`). Chặn bằng chứng chưa hiểu là có chủ đích, nhưng thông báo chung chưa phân biệt pending ACK thật với retry Measurement hoặc artifact không liên quan. Không nên hướng dẫn xóa artifact để vượt gate.

### F8 — P2 — Stop/liveness có khoảng trống trước Count và sau chuẩn bị socket Done

**Phân loại: Có nguy cơ lỗi vận hành, không phải đã quan sát write sai.**

Trước marker ACK có kiểm tra stopping/heartbeat (`acquisition.rs:380`), nhưng sau await UPDATE marker tại `:393` không kiểm tra lại trước `writer.count`. Nếu stop được yêu cầu trong lúc DB chậm, Count vẫn có thể được gửi; stop_collection đặt stopping rồi join task (`worker.rs:308–320`). Trước Done, sau prepare_done chỉ kiểm tra stopping (`acquisition.rs:412–416`), không làm mới heartbeat/snapshot sau thời gian connect socket.

Hợp đồng PLC giữ publication giúp hạn chế thay đổi raw trong các cửa sổ này, nhưng không thay thế stop/liveness. Bổ sung kiểm tra sát write là thay đổi nhỏ; nếu dừng sau marker thì vẫn cần xử lý trạng thái “chắc chắn chưa gửi” để không đổi một lần stop bình thường thành lỗi cần can thiệp vô hạn.

### F9 — P2 — Một số trạng thái UI còn mô tả ACK/backlog không chính xác

**Phân loại: Có nguy cơ lỗi thông tin, không phải schema DTO bị đứt.**

- Raw được ACK xong có ackStatus=ACKNOWLEDGED. Vòng idle tiếp theo đổi về WAITING (`acquisition.rs:333`), nhưng rawCapture vẫn là row vừa ACK. Alert hiện hữu ở `AddMeasurement.tsx:578` vì vậy đổi sang “ACK đang chờ hoặc cần đối chiếu” cho raw đã ACK. Không nên dùng status ACK của vòng idle làm trạng thái ACK của rawCapture đang hiển thị.
- Query FAILED batch được đếm trước khi process; blockedInboxIds có thể còn chứa row vừa xử lý thành công cho tới batch sau. Warning projection được giữ mà không có đường xóa tương ứng khi lỗi đã giải quyết.
- Trang Kết nối chủ động giấu control snapshot khi Worker ERROR/PAUSED (`ConnectionSettings.tsx:120–123`). Counter 0 và ACK đang đối chiếu thường vào ERROR nên có thể nhìn thấy dấu “—” dù backend vừa đọc control hợp lệ. Có raw alert nhưng thiếu counter ngay lúc cần kỹ thuật xem.
- `manualRetryResults` không còn được ghi bởi endpoint retry mới; endpoint thực hiện process đồng bộ (`worker.rs:677–709`). Nút hiện có vẫn gọi được, nhưng không còn lifecycle request/result như DTO cũ gợi ý. Nút retry Measurement không giải ACK.

Có thể sửa backend để khôi phục và giữ đúng status theo receipt mà không đổi layout. Những chỗ UI tự hardcode thông điệp hoặc giấu telemetry cần một lượt được cho phép sửa đúng logic/service tương thích; không cần thiết kế lại trang. Không “sửa” bằng cách để ACKNOWLEDGED cho raw mới chưa ACK.

## 3. Ma trận các tình huống được yêu cầu

Điều kiện chung để phân tích raw mới: cấu hình/map cho phép kết nối, Engine qua gate lịch sử, DB schema mới tồn tại, Ready/control/raw ổn định. Với ACK bản ghi đầu: accepted=0, PC_ReadCount=0, PC_ReadDone=0, không blocked_reason và heartbeat RUNNING. Nếu thiếu điều kiện đó phải xét thêm nhánh chặn, không suy từ TotalCount một mình.

| Tình huống | PostgreSQL / Measurement | ACK và kết quả sau đó | Phân loại |
|---|---|---|---|
| Ready=1, Total=0, 240 WORD toàn 0 trên môi trường khởi tạo hợp lệ | Lưu một raw receipt publication_count=0, record_seq=0 và pending pointer; projection ghi decode_error do Version=0; không Measurement | target<=0: không ghi Count/Done; giữ receipt và báo RECONCILIATION_REQUIRED. Không tự thoát bằng restart | Đã đáp ứng giữ raw/không identity giả; chưa có lối tiếp tục sang publication hợp lệ sau đó |
| Ready=1, Total=0, chỉ D25200=12, khảo sát độc lập trên trạng thái ban đầu tương tự | Lưu đúng một WORD 12 tại raw offset 180; raw RecordSeq ở offset 22 vẫn 0; Version=0 nên không Measurement | Giống trường hợp trên; 12 không ảnh hưởng quyền ACK | Đã đáp ứng lưu nguyên trạng; chưa giải counter 0 |
| Hai payload trên xuất hiện nối tiếp trong cùng pending counter 0 | Chỉ raw đầu đã nhận là receipt hiện hành. Không insert một row khác cho phiên bản thứ hai, không overwrite | Lỗi counter 0 xảy ra trước so raw; không có latch conflict cụ thể cho thay đổi đó | Có nguy cơ hiểu nhầm “mọi payload đều được lưu”; cần thông báo rõ giới hạn |
| Ready=1, Total=1, raw toàn 0, accepted=0 và handshake đúng | Raw commit, record_seq gốc=0; không Measurement; raw có thể hiện trong Giám sát và card raw Lịch sử | Không đợi decode. Khi heartbeat/quyền đủ, marker → Count=1 → Done=1. Chỉ hoàn tất nếu quan sát được proof Done cao→thấp | Đã đáp ứng đường raw ACK; timing cần xác minh runtime |
| Raw commit thành công, app đóng trước commit marker attempted | Raw + pending còn DB, attempted=false; UNIQUE không tạo bản sao khi restart | Nếu snapshot/handshake vẫn khớp, restart có thể gửi lần ACK đầu tiên sau liveness. Đây là phục hồi tự động hợp lệ | Đã đáp ứng có điều kiện PLC giữ publication |
| Raw commit, marker attempted cũng commit, app đóng trước Count | Cùng raw/pending, attempted=true, done_observed=false | Restart chỉ observe; PLC chưa có tác nhân làm Done cao→thấp. Có thể chờ vô hạn | Chưa đáp ứng khả năng tự phục hồi; không được gọi đây là “Count đã gửi” |
| App đóng sau Count nhưng trước Done | Count có thể bằng target; DB vẫn chỉ biết attempted=true, không biết Done chắc chắn chưa gửi | Không gửi tiếp Done dù Count read-back đúng. Nếu PLC cần Done mới tiêu thụ, Worker không tự hoàn tất | Chưa đáp ứng; cần thêm một bit bằng chứng hoặc đối chiếu thủ công |
| PLC nhận Done nhưng PC mất response | Raw vẫn commit. `done_prepared` trả Err, không đọc snapshot ngay sau ghi | Nếu vòng sau còn thấy snapshot raw cũ + Done cao, persist done_observed rồi có thể hoàn tất khi thấp. Nếu đã bỏ lỡ cao, không tự xác nhận dù Ready=0/Count đúng | Có nguy cơ kẹt; cần xác minh timing/PLC receipt |
| Restart với attempted=true | Không tạo lại raw hoặc Measurement đã link; chỉ dùng pending receipt và DB progress | done_observed=true + snapshot hoàn tất: có thể tăng accepted. false + còn quan sát được cao: có thể tiến tiếp. false + PLC chờ lệnh hoặc đã thấp: có thể kẹt | Không phải mọi attempted=true đều kẹt; nhiều cửa sổ không có recovery |
| Hai publication khác nhau có cùng 240 WORD, counters 1 rồi 2 trong cùng phiên | Có hai row với publication_count khác nhau nếu ACK 1 được xác nhận và PLC giữ bản 2 cho tới lúc nhận. Không hash để gộp | Mỗi counter có ACK riêng. Nếu ACK 1 kẹt, bản 2 chưa đi vào receiver vì pending còn chiếm chỗ | Đã đáp ứng chống gộp theo nội dung, có điều kiện tiến trình ACK |

D25200 theo map hiện tại: vùng TIP bắt đầu offset 128, P3 thêm 2×22, Bend A1 thêm 8 → offset 180; cộng D25020 ra D25200 (`as200/map.rs:38`, decoder bố cục point/station). Một WORD bằng 12 không tự là Bend=12 mm. Với các WORD khác bằng 0, Version vẫn 0 nên không có Measurement; không cần giải mã số đo để lưu raw.

Nếu 240 WORD giống nhau và có RecordSeq=1, publication counter 2 vẫn phải là raw-only khi RecordSeq không khớp 2 (`inbox_service.rs:131`). Hai raw receipt không bị gộp; không được sửa RecordSeq để ép tạo Measurement thứ hai.

## 4. Migration, transaction và idempotency

### Đã đáp ứng theo source

- Migration `20261117120000_publication_receipts.sql` thêm cột nullable, index và bảng plc_acquisition; không sửa hai migration cũ, không DELETE/TRUNCATE hoặc reset sequence/history. Các row cũ có metadata mới NULL nên không tham gia UNIQUE publication mới.
- UNIQUE `(source_id, publication_session, publication_count)` tách identity khỏi RecordSeq payload; index cũ `(source_id,session_id,record_seq)` vẫn tồn tại cho lịch sử. Raw mới session_id=NULL nên raw RecordSeq=0 lặp giữa nhiều counter không va index cũ.
- CHECK mới bỏ ràng buộc session/status cũ đối với publication mới; nếu không làm vậy raw có session SQL NULL sẽ không chuyển sang PENDING/FAILED/PROCESSED được. CHECK PROCESSED ↔ measurement link/timestamp và FAILED ↔ error_message trong migration khởi tạo vẫn còn.
- `store.rs:53–86`: source lock, lookup publication, so raw immutable, insert và cập nhật pending cùng transaction. Commit lỗi/mất response không chạy tới lệnh ghi PLC trong iteration đó. Sau reconnect, pending đã commit hoặc lookup UNIQUE giữ cùng row; không tự đổi timestamp/SKU của row cũ.
- Projection lock Inbox, kiểm tra link có sẵn, insert toàn bộ cây Measurement và cập nhật PROCESSED trong transaction riêng (`inbox_service.rs:218–246`). Nếu commit đã thành công nhưng response mất, đánh dấu FAILED có WHERE chống ghi đè PROCESSED (`inbox_repository.rs:163`), retry đọc link hiện có thay vì insert lại.
- Source upsert chỉ ở Engine open; full prefix query chỉ trong reader legacy/baseline, không scan mỗi poll. Poll thực tế có thêm read-back receipt, projection query theo lịch và error diagnostics; không phải mọi vòng chỉ có đúng một SQL request.

### Có nguy cơ lỗi hoặc cần tăng ràng buộc nhỏ

1. Migration tìm và DROP **mọi** CHECK có định nghĩa chứa cả `session_id` và `processing_status`, không xác minh đúng predicate cũ hoặc số constraint khớp. Trên schema đúng hai migration cũ, nó nhắm đúng invariant cần thay. Trên schema có CHECK tùy chỉnh, phạm vi có thể rộng hơn dự định. Nên xác định constraint/predicate rõ và fail nếu schema khác, không kết luận migration hiện đã chạy lỗi khi chưa thực thi.
2. Schema chưa ràng buộc publication_count=total_count, hoặc pending/anchor phải cùng source/session; FK chỉ bảo đảm row ID tồn tại. Receiver bình thường tạo dữ liệu khớp và pending receipt được app kiểm tra. Tuy nhiên `open` kiểm tra anchor mới chỉ source + total, chưa kiểm tra publication_session/count/word order tương ứng (`acquisition.rs:66–72`). Đây là hardening với dữ liệu mâu thuẫn, không phải chứng cứ app đang tự tạo sai anchor trong luồng thường.
3. UPDATE lấy quyền gửi ACK chỉ có pending ID và NOT attempted; không kèm accepted/session/block làm CAS. Với một Engine và không sửa DB ngoài luồng, gate tuần tự hóa được. Nếu muốn dựa DB làm hàng rào nhiều process/copy thư mục, nên kiểm tra đầy đủ snapshot trạng thái ngay tại CAS; không coi OS lock theo đường dẫn là khóa toàn máy cho mọi bản cài.
4. Raw conflict trong `store.rs:65` commit latch mà không overwrite row cũ: đúng bảo vệ. Nhưng nội dung raw mâu thuẫn mới không được lưu làm một publication thứ hai. Muốn giữ mọi phiên bản vi phạm hợp đồng là yêu cầu archive kỹ thuật riêng, không được giả identity/hash để nhập vào bảng production.
5. Migration runner kiểm tra version và sự có mặt bảng plc_acquisition (`database/migrations.rs:115`), không chứng nhận cột/index/CHECK thực tế. Chưa biết migration đã triển khai ở DB nào. Nếu cần thêm bit/schema, dùng migration tiếp theo khi có khả năng bản hiện tại đã được áp dụng; không sửa checksum file đã áp dụng.

Startup gọi initialize/migrate trước khi mở Engine (`lib.rs:151–157`). Vì vậy migration DB có thể hoàn tất rồi cutover bị gate từ chối; gate từ chối không có nghĩa DB vẫn ở schema cũ. Không đề xuất rollback bằng xóa bảng hoặc chạy binary cũ.

## 5. Production còn dùng Journal cũ ở đâu?

**Đã đáp ứng việc ngừng Worker cũ:** `worker.rs:26–29` chỉ khai báo acquisition, connection_log, legacy, store. Không có declaration/include/call graph production nạp `worker/{ack,capture,delivery,journal,recovery,retry}.rs`. `worker.rs:767` nạp tests dưới cfg(test). Giữ file trên disk không tự làm Rust compile/chạy file đó.

`plc/as200/mod.rs` vẫn khai báo `pub mod ack` — đây là **transport đang dùng**, không phải `worker/ack.rs` cũ. Trong `as200/ack.rs`, các kiểu PublishedAckPhase/Intent và predicate proof năm pha chỉ còn dưới cfg(test); production mới dùng AckWriter/transport. Không tìm thấy pipeline song song phát lại Journal.

**Vẫn có phụ thuộc có chủ đích hoặc còn sót:** đọc binding/receipt/prefix legacy để cutover, journal_path trong config để định vị bằng chứng/capture.lock, check filesystem trong lib.rs, parser SKU key lịch sử và DTO/command UI cũ. `InboxService::receive`/một số repository helper cũ vẫn nằm trong module được compile dù không phải receiver của Worker mới. Không nên nói “đã xóa mọi logic Session/Journal/compatibility”.

`confirm_session` hiện chặn session NULL từ đầu (`inbox_service.rs:197`), còn session Some thì trả thành công nếu trùng hoặc trả lỗi nếu khác. Nhánh UPDATE bên dưới không có đường tới theo các nhánh hiện tại. Đây là ứng viên dọn nhỏ sau rà call site; không phải công cụ khôi phục raw hay ACK.

Test source cũ vẫn gọi Engine::open ba tham số và `.unwrap()` trực tiếp, trong khi API mới async/bốn tham số (`worker/tests.rs:12` so với `acquisition.rs:6`). `connection_settings/tests.rs:124` vẫn gọi prepare_collection đã bỏ. Đây là bất tương thích source test nhìn thấy được; không phải kết quả compiler vừa chạy. Không sửa hay chạy tests trong review.

## 6. Dữ liệu mới có xuất hiện trên UI giữ nguyên không?

### Đã đáp ứng đường dữ liệu chính

- DTO camelCase và tên command Worker/raw vẫn tương thích service TypeScript hiện tại. MainLayout sở hữu polling trạng thái (`MainLayout.tsx:39–71`); frontend không tự đọc Modbus.
- Sau commit raw mới, show_receipt gắn rawCapture có Inbox ID/thời điểm/240 WORD (`acquisition.rs:93–135`). Giám sát có alert raw riêng (`AddMeasurement.tsx:576`); raw không làm tăng KPI, biểu đồ hoặc bảng Measurement. Đây là hành vi đúng với “không Measurement giả”.
- Lịch sử gọi `get_plc_raw_snapshots` với trigger rawCapture.inboxId/lastSavedId/reload (`MeasurementHistory.tsx:98–106`). Repository mới lấy row chưa có Measurement, có publication_session hoặc session NULL (`inbox_repository.rs:98`). Raw-uninitialized và raw legacy session NULL không còn bị loại chỉ vì prefix.
- Projection thành công set lastSavedId, kể cả khi row cũ không được thay vào latest do timestamp (`acquisition.rs:103–117`). Giám sát/KPI/bảng gần nhất và Lịch sử Measurement dùng signal đó để query lại (`AddMeasurement.tsx:277–321`, `MeasurementHistory.tsx:86,96,120`). Các query vẫn đọc bảng measurements; session_id SQL NULL của Inbox mới không làm query Measurement tự mất dữ liệu.
- Chi tiết/Báo cáo chỉ có cho Measurement thật. Báo cáo vẫn cần thao tác áp filter/tạo báo cáo theo cơ chế hiện hữu, không tự live-refresh mọi raw.

### Chưa bảo đảm hoặc còn thông điệp lệch

- F4 có thể khiến raw valid mới mãi không được project; F5 khiến alert Giám sát thiếu sau restart. Không thể bảo đảm “giữ UI nguyên thì mọi dữ liệu mới luôn xuất hiện đầy đủ” chỉ từ việc DTO không đổi.
- Raw history vẫn giới hạn mặc định 20 row mới nhất, không filter SKU/thời gian/source; phân trang frontend không phải phân trang toàn bộ raw DB. Row đã có Measurement rời query raw và xuất hiện ở bảng Measurement — không phải bị xóa.
- Thiếu SKU lúc capture hoặc payload chỉ hợp lệ về kiểu nhưng không đủ workflow/identity vẫn không có Measurement (`inbox_service.rs:129–151`). assigned_sku=NULL không được thay bằng SKU hiện tại sau restart; đúng chống gán lại lịch sử, nhưng cần người vận hành biết lý do không lên thống kê. Backend lưu decode_error nhưng DTO PlcRawSnapshot hiện không mang lý do này ra card raw (`measurement_command.rs:118`).
- Giám sát đang lọc theo SKU đã lưu; Measurement của SKU khác không vào chart/KPI đó. Đây là filter hiện hữu, không lỗi DB.
- F9 làm status ACK/counter khó hiểu dù raw DB vẫn còn. Cần sửa thông tin đi qua backend hoặc service contract trước; chưa có lý do thiết kế lại layout/routes/CSS.

## 7. Phương án phục hồi ACK tối thiểu đề xuất

### Giới hạn có tính thông tin

Chỉ với attempted=true, raw receipt, Count=target, Done=0, Ready=1 và raw còn nguyên, DB hiện không phân biệt được: (A) Count đã gửi nhưng Done chưa từng được gọi; (B) Done đã gửi/mất response nhưng PLC chưa xử lý; (C) các trạng thái trung gian/reset khác. Không thể vừa bảo đảm tự tiếp tục mọi trường hợp vừa tuyệt đối không resend Done bằng cách chỉ đổi một câu if. Chờ timeout lâu hơn hoặc dựa write echo không giải quyết thiếu bằng chứng.

### Gói nhỏ nhất nên chọn nếu giữ nguyên hợp đồng PLC hiện tại

1. **Giảm cửa sổ đánh dấu khi chưa gửi:** chuẩn bị socket Count trước marker giống prepare_done, không gửi byte Modbus trước raw commit/marker; kiểm tra stop/liveness sát write. Lỗi connect trước marker có thể retry bình thường. Điều này không tự giải crash sau marker.
2. **Thêm một bit bền vững `done_started` hoặc `done_may_have_been_sent`, không thêm năm pha.** Fresh pending receipt khởi đầu biết chắc false. Commit true ngay trước lần FC06 duy nhất. Với attempted=true và done_started=false, nếu DB/raw/binding/counter/Count read-back đúng target, Done=0, Ready=1 và liveness đủ, có thể gửi **Done lần đầu**, sau khi CAS false→true thành công. Không phải phát lại Done. Nếu Count chưa read-back tới target thì tiếp tục quan sát hoặc chặn; chưa tự retry Count khi chưa chứng minh idempotency/timing.
3. **Coi dữ liệu attempted cũ là unknown:** nếu thêm cột bằng migration, không default false cho các pending attempted=true đã tồn tại. Dùng NULL/unknown cho chúng và giữ gate đối chiếu. Không được suy “chưa Done” từ cột mới chưa có lịch sử.
4. **Giữ proof hoàn tất hiện tại trong lượt sửa nhỏ đầu:** done_started=true mà chưa đủ Done proof vẫn chỉ quan sát, không tự clear bit hoặc viết Done lại. Trường hợp thật sự chưa rõ còn cần một thao tác kỹ thuật được thiết kế riêng, có precondition và bằng chứng được lưu; không cung cấp lệnh SQL reset flag để chạy tiếp.

Gói này bổ sung đúng một sự thật còn thiếu — Done chắc chắn chưa bắt đầu hay có thể đã gửi — và tận dụng nguyên raw receipt/checkpoint hiện có. Không cần deliveries queue, local Journal hoặc CountVerified/Prepared/Confirmed phase machine. Tuy vậy nó **không hứa** giải crash trước Count đã tới PLC, hoặc Done đã bị bỏ lỡ mức cao.

Một phương án còn ít trạng thái hơn chỉ phù hợp **sau khi PLC xác nhận** Count là phép gán idempotent, không tự tiêu thụ record và không làm sai bản kế tiếp khi request cũ tới trễ: dùng marker duy nhất ngay trước Done, còn Count phục hồi bằng read-back đúng target trên cùng raw/epoch. Khi đó Count=target cho phép bỏ qua lần ghi Count đã áp dụng; Count còn cũ không tự là bằng chứng lệnh trước chưa tới. Vì chưa có hợp đồng này, review không chọn dời marker như một sửa chữa an toàn ngay lập tức. Các row attempted=true theo nghĩa cũ cũng không được tự diễn giải lại thành “Done đã bắt đầu”; cần phân biệt phiên bản ngữ nghĩa/cutover rõ ràng.

### Điều kiện để giảm tiếp yêu cầu Done cao

Muốn phục hồi trường hợp PLC đã tiêu thụ nhưng PC không thấy cao, cần xác nhận bằng Ladder/trace một proof bền thay thế, ví dụ Count=target + Ready=0/target không đổi chỉ có thể xuất hiện sau khi tiêu thụ Done của target trong cùng epoch, hoặc PLC có trường “last consumed/acknowledged publication” giữ được qua reconnect. Nếu Reset hay logic khác cũng tạo tuple đó thì tuple không đủ.

Một latch/ack counter ở PLC có thể đơn giản hơn bắt PC theo dõi xung ngắn và giảm trạng thái phía app. Đây là thay đổi hợp đồng PLC, không tự thêm register vào map hoặc tự chấp nhận tuple khi chưa xác minh. Chỉ nhìn thấy publication kế tiếp cũng cần bảo đảm PLC không được phép advance/overwrite vì lý do khác. Không dùng heartbeat đơn lẻ làm chứng cứ không có reset.

## 8. Những trường hợp vẫn cần can thiệp thủ công

| Nhóm | Lý do hiện tại | Có thể đơn giản hóa thế nào |
|---|---|---|
| attempted=true, PLC chưa nhận Count | Marker commit trước network; không có proof chưa gửi sau restart | Preconnect giảm tần suất; chỉ auto retry khi có bằng chứng NotSent đủ chắc hoặc idempotency PLC được xác nhận |
| Count đã tới, Done chưa gửi | Bit hiện tại gộp cả hai lệnh | Thêm done_started để lần Done đầu tiên có proof; không backfill false cho lịch sử chưa rõ |
| Done đã tới, không lưu được quan sát cao | Response mất, xung ngắn hoặc DB outage trong lúc lưu done_observed | Latch/consumption proof do PLC xác nhận; hiện chưa được bỏ high guard |
| INITIAL_HANDSHAKE, RESET, SNAPSHOT_CHANGED, PUBLICATION_CONFLICT | Latch DB tồn tại qua restart, chưa có đường giải | Phân biệt lỗi transport tự hồi với mâu thuẫn thật; thao tác đối chiếu có điều kiện, không nút clear chung |
| Counter 0 hoặc gap | Không có target kế tiếp chắc chắn, receipt đang chiếm pending | Giữ raw; xử lý quan sát counter 0 theo F6 khi có hợp đồng PLC; không giả target/epoch |
| Thiếu Journal nhưng còn history, Journal pending/quarantine/phiên khác | Không có đủ proof cũ để khởi tạo accepted | Sửa bug reader legacy F1; còn pending thật phải đối chiếu/cutover riêng, không bỏ gate |
| Raw không đủ workflow hoặc thiếu assigned SKU | Không có dữ liệu/assignment đủ để làm Measurement | Hiển thị decode_error; chỉ reprocess khi có sửa decoder hoặc assignment được xác nhận rõ, không bịa dữ liệu |
| Nhiều FAILED projection cố định | Không phải lỗi ACK nhưng làm đói batch | Fairness/bounded retry riêng; giữ receipt ID/link và retry thủ công hiện hữu |

Lỗi mạng/DB trước ACK attempt hoặc heartbeat OBSERVING/STALE có thể tự hồi nếu cấu hình cho reconnect và PLC giữ raw; không mặc định mọi lỗi đều cần người can thiệp. Reconnect endpoint hiện không giải blocked_reason hay unknown ACK (`worker.rs:721`).

## 9. Danh sách thay đổi nhỏ nhất tiếp theo

Thứ tự đề xuất, **chưa thực hiện trong review**:

1. **Sửa reader cutover F1**: hỗ trợ sentinel Source ID cũ bằng proof SourceKey/row; phân biệt lỗi binding, pending ACK và lỗi compatibility. Rà last_total=accepted+1 của F7, bảo toàn last_total cũ và kiểm tra PLC tương ứng. Chưa xóa sáu file Worker cũ.
2. **Sửa cửa sổ ACK F2/F8**: preconnect Count, stop/liveness sát write; thêm một bit Done-started bằng migration tiếp theo nếu cần. Trạng thái attempted cũ phải unknown. Không mở đường resend Done hoặc clear attempted chung.
3. **Khôi phục status từ DB F5 và giữ ACK gắn đúng raw F9**: đọc pending/anchor lúc open, status blocked vẫn có raw hiển thị; không làm mới trạng thái ACK của raw đã xác nhận bằng WAITING của vòng idle.
4. **Làm batch projection công bằng F4**, diagnostics đúng sau xử lý; giữ nguyên transaction/link idempotency. Trả decode_error qua DTO/service nếu cần để người dùng biết vì sao không có Measurement; không đổi layout.
5. **Chốt quy tắc quan sát counter 0 F6 và proof Done F3 với PLC**. Chỉ sau đó mới thay điều kiện nhận publication tiếp theo hoặc bỏ yêu cầu snapshot/high nào. Bảo toàn mọi row và không sửa counter/Session để vượt lỗi.
6. **Dọn phần không còn nhiệm vụ sau các bước trên**: quyền write với archive Journal trong lib.rs, helper/nhánh confirm_session unreachable, DTO thông báo/retry cũ. Siết phạm vi migration DROP CHECK và xác minh anchor theo source/session/counter; không scan full prefix mỗi poll.

Khi được cho phép ở lượt khác, cập nhật test theo các cửa sổ crash/commit/Done nêu trong bảng rồi type-check và kiểm chứng trên môi trường kiểm soát. Test Worker cũ không thể được coi là xác nhận backend mới chỉ vì file tests.rs còn giữ nguyên. Review này không chạy hoặc sửa test/build để thay thế bước đó.

## 10. Những điều cần xác minh runtime trước quyết định giảm guard

1. PLC có giữ cùng publication/raw/TotalCount cho tới Done, và mỗi publication hợp lệ có counter riêng không? Counter 0 là khởi tạo hay publication được PLC cho phép ACK?
2. Done cao kéo dài bao lâu, có thể cùng tồn tại với raw cũ/Ready=1 qua đủ snapshot không; PLC có cập nhật Ready/raw trước khi xóa Done không?
3. Sau FC06 mất response, control nào chứng minh chắc chắn tiêu thụ đúng target; có giữ qua mất điện/reset không? Count có chỉ là register PC ghi hay là receipt PLC xác nhận?
4. Retry cùng Count có hoàn toàn idempotent không, request cũ có thể được áp dụng trễ sau khi đã sang publication mới không? Không trả lời bằng echo Modbus.
5. DB đang có migration/index/CHECK nào, pending row/Journal thật đang ở tình trạng gì? Chưa kết nối để kiểm kê nên không kết luận cutover máy cụ thể an toàn.
6. Compile production/test target và độ trễ DB/FC03 thực tế. Outer timeout hiện `12×timeout_ms+8000` (`worker.rs:508`) bao cả nhiều snapshot và projection; độ trễ cao có thể hủy iteration sau marker. Không tăng timeout để thay thế proof phục hồi.

**Kết luận review:** nền tảng raw commit/UNIQUE và việc ngừng Worker cũ đã đạt mục tiêu chính. Các thiếu sót ưu tiên là cutover sentinel Source ID, khả năng tiếp tục sau marker ACK, fairness projection và khôi phục status. Chưa có căn cứ giảm guard chống ACK sai; cũng không cần dựng lại Journal v4 để sửa những điểm này.
