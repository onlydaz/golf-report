# Báo cáo refactor backend PLC / PostgreSQL

Cập nhật ngày **17/11/2026, giờ Việt Nam**. **Số đo bằng 0 và kiểm chứng mới nhất ở mục 12; kết quả 11 PostgreSQL integration test trên Neon ở mục 11: 11 PASS trong lượt trước**. Mục 9 ghi lại lượt offline trước đó, mục 10 là audit migration trước khi chạy integration, cleanup source ở mục 8. Sáu module Worker cũ đã xóa thật; chưa nghiệm thu PLC thật/Ladder hoặc mọi cửa sổ mất điện/COMMIT. Mục 1–7 ghi lại các lượt refactor/sửa lỗi/kiểm tra trước cleanup; các câu “giữ sáu file cũ” trong các mục đó là trạng thái lịch sử. Báo cáo post-refactor vẫn được giữ làm snapshot các lỗi trước lượt sửa.

## 1. File sửa/thêm trong lượt ổn định backend

| File | Nội dung |
|---|---|
| `src-tauri/src/plc/worker/legacy.rs` | Đọc sentinel source_id=0; giữ last_total cũ; nhận accepted+1 có proof ACK_CONFIRMED; vẫn kiểm tra anchor/receipt/prefix/binding/session; từ chối history ngoài checkpoint hoặc native receipt bị mất ACK state. |
| `src-tauri/src/plc/worker/store.rs` | Thêm done_started nullable vào model progress; transaction khóa progress và kiểm tra điều kiện chuyển quan sát counter 0 sang publication 1, giữ nguyên row 0. |
| `src-tauri/src/plc/worker/acquisition.rs` | Tách bằng chứng Count/Done, chuẩn bị transport trước marker, CAS/guard trước write, tiếp tục Done lần đầu khi có proof; khôi phục display từ DB; projection có quota/cursor và diagnostics sau batch. |
| `src-tauri/src/plc/worker.rs` | Thêm cursor projection và rawCapture.ackStatus; manual retry refresh lại status từ DB; tách timeout projection khỏi kết quả ACK và đánh thức loop khi stop. Vẫn chỉ một task/Engine. |
| `src-tauri/src/plc/as200/ack.rs` | Thêm prepare_count/count_prepared; dùng đúng socket đã chuẩn bị, giữ FC16 Count rồi FC06 Done, timeout/echo validation. |
| `src-tauri/migrations/20261117130000_done_write_evidence.sql` **mới** | Thêm cột done_started. ACK attempted cũ giữ NULL; chỉ row chưa attempted được gán false. Thêm CHECK cho quan sát Done. |
| `src-tauri/src/lib.rs` | Bằng chứng capture.json cũ chỉ cần mở đọc, không yêu cầu quyền ghi file. |
| `src-tauri/src/services/inbox_service.rs` | Bỏ helper confirm_session không có call site production và có nhánh UPDATE unreachable; không thêm đường sửa Session. |
| `src-tauri/src/repositories/inbox_repository.rs` | Projection thành công xóa decode_error cũ cùng transaction đánh dấu PROCESSED. |
| `src/services/as200Service.ts` | Type bổ sung ackStatus của rawCapture, có fallback cho response cũ. |
| `src/pages/AddMeasurement/AddMeasurement.tsx` | Alert hiện hữu dùng ACK của đúng rawCapture. Chỉ sửa logic trạng thái. |
| `src/pages/ConnectionSettings/ConnectionSettings.tsx` | Alert hiện hữu dùng ACK của raw; counter đọc hợp lệ vẫn hiển thị khi lỗi là ACK/pause. |
| `README.md`, `docs/PLC_BACKEND_REFACTOR.md` | Cập nhật semantics và giới hạn phục hồi. |

Không sửa layout/routes/CSS, bảng/biểu đồ/bộ lọc, query nghiệp vụ báo cáo hoặc thiết kế trang. Không sửa tests.rs, config PLC/DB, Journal runtime hoặc migration cũ. Sáu file `worker/{ack,capture,delivery,journal,recovery,retry}.rs` giữ nguyên và tiếp tục không được production Worker nạp. Không xóa dữ liệu PostgreSQL/Neon.

## 2. Lỗi đã khắc phục

### Cutover

- Sentinel input.source_id=0 là định dạng delivery cũ, không phải DB source thật. Reader cho phép sentinel này nhưng vẫn buộc row.source_id khớp SourceKey đã resolve, Inbox ID/key/session/raw/word order/timestamp, binding và prefix receipt.
- `last_total=accepted+1` chỉ được nhận khi Journal có ACK_CONFIRMED cùng confirmed receipt hợp lệ. Checkpoint accepted giữ nguyên; last_total được mang sang DB. Snapshot PLC cần còn đúng publication kế tiếp, Count=accepted và Done=0; nếu không thì giữ INITIAL_HANDSHAKE, không xóa bằng chứng hoặc nhập Count remote làm checkpoint.
- Marker legacy rỗng mà reader v4 cũ từng chấp nhận được đọc tương thích; marker chứa lịch sử/pending vẫn bị từ chối. Không viết lại JSON cũ.
- Nếu thiếu state DB nhưng đã có receipt native, hoặc DB còn publication ngoài checkpoint Journal, không tự dựng lại state từ Journal cũ.

### Counter 0

Raw 240 WORD tại TotalCount=0 vẫn lưu nguyên trạng, RecordSeq không bị sửa. Khi PLC công bố 1, một transaction khóa source/progress/row cũ và chỉ nhận row 1 nếu: accepted=0, chưa anchor, last_total không vượt 1, pending là đúng row publication 0 cùng nguồn/phiên/map, chưa attempted, done_started=false, chưa quan sát Done cao, không block, và snapshot mới Ready=1/Total=1/Count=0/Done=0 ổn định.

Transaction giữ row 0, insert hoặc đối chiếu UNIQUE row 1 và đổi pending pointer cùng commit. Không ACK row 0, không tăng accepted để bỏ qua nó, không đổi identity/SKU/timestamp của row 0. Crash/mất response commit được đối chiếu bằng pointer/UNIQUE ở lần sau. Trường hợp reset từ accepted dương, gap hoặc pending đã attempted không được đi nhánh này.

### Projection và hiển thị

Batch có tối đa 5 row mới/PENDING và 5 FAILED, xử lý xen kẽ; mỗi nhóm có cursor theo ID và quay vòng. Cursor được cập nhật trước khi thử xử lý; một row lỗi không dừng cả batch. Không còn 10 FAILED cố định chiếm hết suất mới. Transaction khóa Inbox và unique Measurement vẫn giữ nguyên; retry dùng lại link đã có, không tạo bản đo trùng.

Diagnostics được đọc lại sau batch; warning MEASUREMENT đã giải quyết được dọn theo batch. ID lỗi trả ra UI giới hạn 100, tổng lỗi/chờ lấy từ DB. Raw decode_error vẫn không bị tự ép thành Measurement.

Projection có timeout riêng: chậm/lỗi projection chỉ tạo warning và retry cùng Inbox ID, không biến một lần ACK đã xác nhận thành ACK thất bại. Timeout vòng ACK được tính đủ cho các snapshot bảo vệ và hai socket/write; từng thao tác transport vẫn có timeout riêng. Đây không phải cơ chế thay thế proof ACK.

Engine mở lại đọc receipt gần nhất, pending receipt và ID Measurement gần nhất. Pending raw có block vẫn được hiển thị trước khi áp pause. rawCapture mang ACK riêng được suy từ pending/accepted prefix cùng identity; row 0 chưa xác nhận không thành ACKNOWLEDGED khi accepted tăng lên 1. Idle không ghi đè ACKNOWLEDGED của receipt cũ thành WAITING. lastSavedId được khôi phục để UI tiếp tục truy vấn dữ liệu thật.

## 3. Trình tự ACK hiện tại

State nhỏ tiếp tục dùng một pending receipt, accepted/last_total, anchor và block reason. Thêm **một cột nullable done_started**, không có Prepared/CountVerified hoặc máy trạng thái năm pha:

| Bằng chứng DB | Ý nghĩa |
|---|---|
| ack_attempted=false, done_started=false | Có raw receipt; Worker chưa bắt đầu lần ghi Count. |
| ack_attempted=true, done_started=false | Count có thể đã gửi; Done chắc chắn chưa bắt đầu. Count chỉ được coi là đã đối chiếu khi đọc lại đúng target trên cùng publication. |
| ack_attempted=true, done_started=true | Done có thể đã gửi; chỉ quan sát, không resend. |
| ack_attempted=true, done_started=NULL | Lịch sử attempted theo backend trước; không biết Done đã bắt đầu chưa. Chỉ quan sát. |

1. Đọc stable snapshot khi DataReady=1, lưu nguyên 240 WORD và pending pointer cùng transaction PostgreSQL, read-back receipt.
2. Kiểm tra publication counter kế tiếp >0, Count=accepted, Done=0, raw/identity/binding/map/quyền/heartbeat và không block.
3. Chuẩn bị socket Count **không gửi Modbus**, đọc lại PLC, commit marker ack_attempted bằng CAS nguồn/phiên/binding/checkpoint/pending.
4. Kiểm tra snapshot/stop/liveness sau DB await, rồi FC16 Count=TotalCount trên socket đã chuẩn bị. Không resend Count nếu call đã bắt đầu mà kết quả chưa rõ.
5. Chuẩn bị socket Done, đọc lại Count=target và cùng publication. Chỉ khi done_started=false mới CAS sang true, kiểm tra sát write rồi gửi FC06 Done=1 **lần đầu**.
6. Giữ nguyên yêu cầu quan sát snapshot Done cao với raw cũ phù hợp, sau đó Done thấp + Count đúng + Ready/counter tương thích. Khi đủ proof mới tăng accepted, giữ anchor và giải phóng pending.

Có một rollback marker rất hẹp trong cùng task: khi chính task vừa claim marker nhưng kiểm tra trước transport thất bại và chưa gọi count_prepared/done_prepared, nó có thể trả marker về trạng thái chắc chắn chưa gửi. CAS giữ đúng receipt/checkpoint. Không có lệnh UI/reset chung; không dùng nhánh này sau transport timeout, response error hoặc restart. Block reason vẫn được giữ.

Migration mới không gán false cho ACK attempted cũ. Lỗi/mất response khi commit marker cũng không được diễn giải là chưa gửi.

## 4. Các tình huống tự phục hồi sau restart

| Tình huống | Hành vi |
|---|---|
| Raw commit, chưa attempted | Khôi phục receipt, đối chiếu PLC; nếu publication/handshake/liveness đủ thì gửi Count lần đầu. |
| Count đã tới nhưng app đóng trước Done, done_started=false | Đọc Count đúng target + raw/identity còn nguyên; chuẩn bị socket, claim Done và gửi Done lần đầu. Không ghi Count lại. |
| Count response mất nhưng PLC thực sự đã ghi Count, done_started=false | Lần đọc sau chứng minh Count=target; có thể tiếp tục Done lần đầu như trên. |
| Socket Count connect thất bại trước marker | Chưa có ACK attempt; có thể thử kết nối lại theo lifecycle hiện hữu. |
| Socket Done connect thất bại trước marker Done | Done vẫn false; khi Count/publication đúng có thể thử chuẩn bị socket và gửi Done lần đầu sau đó. |
| Đã lưu done_observed=true, PLC về trạng thái hoàn tất phù hợp | Chỉ quan sát để xác nhận accepted, không gửi Count/Done lại. |
| Counter 0 chưa attempted rồi PLC công bố 1 đúng handshake | Giữ row 0; atomically tiếp nhận row 1 theo điều kiện mục 2, kể cả sau restart. |
| Measurement commit thành công nhưng mất response | Retry đọc link có sẵn; restore lastSavedId và status từ DB, không tạo Measurement trùng. |

Mọi phục hồi còn phụ thuộc PLC giữ publication và không reset/đổi epoch; heartbeat/quyền phải đủ khi cần ghi. Payload toàn 0 hoặc chỉ một WORD khác 0 không làm các đường raw ACK này phụ thuộc TIP/BUTT/Overall/Status bit0.

## 5. Còn cần đối chiếu vì thiếu bằng chứng

- Count attempt đã đánh dấu nhưng Count vẫn chưa đọc lại tới target: không biết request chưa tới, tới trễ hay có reset. Không resend Count mù. Crash giữa commit marker và transport vẫn có thể rơi vào nhóm này.
- done_started=true hoặc NULL và chưa thấy/lưu được Done cao: không resend Done. Nếu PLC xóa Done trong một scan hoặc thay raw/Ready trước khi PC lấy được snapshot phù hợp, có thể vẫn phải đối chiếu thủ công. Không dùng write echo hoặc chỉ Ready=0 để giả ACK thành công.
- INITIAL_HANDSHAKE/RESET/SNAPSHOT_CHANGED/PUBLICATION_CONFLICT; pending/counter gap; Journal hoặc DB history không đủ proof. Không tự clear latch, sửa checkpoint/Session hay reset PLC.
- Counter 0 còn giữ nguyên mức 0, hoặc raw thay đổi mà PLC không cung cấp identity mới: giữ raw đã commit và thông báo đúng giới hạn; không tạo publication giả cho mọi phiên bản RAM.
- Raw thiếu workflow hoặc assigned SKU vẫn không vào Measurement. Không tự gán SKU hiện tại vào raw lịch sử. Điều này không chặn ACK raw khi publication/handshake đã đủ.

Chưa có bằng chứng Ladder/trace cho timing Done, retention, atomicity/late request và word order trên thiết bị thật. Không thêm thanh ghi hoặc nới high-proof để che những giới hạn này.

## 6. Kiểm chứng trong lượt sửa

Chỉ đọc source/call site, rà SQL/nhánh crash và định dạng/parse cú pháp Rust bằng rustfmt; kiểm tra diff. Không chạy test/build/app/migration/Simulator, không kết nối PLC/PostgreSQL. Không sửa tests.rs hoặc sáu module Worker cũ. Chưa xác nhận type-check, SQL runtime, khả năng chịu tải hoặc factory readiness.

Hai migration trước và `20261117120000_publication_receipts.sql` không bị sửa trong lượt này. `20261117130000_done_write_evidence.sql` là migration bổ sung, chưa được chạy bởi agent. Startup vẫn có migration runner hiện hữu khi người dùng chủ động mở ứng dụng. Các test Worker cũ phụ thuộc API Journal vẫn cần được cập nhật trong một lượt cho phép riêng; không coi việc giữ nguyên file test là bằng chứng backend mới đã được kiểm thử.

## 7. Kiểm tra biên dịch sau refactor — 17/11/2026

Lượt kiểm tra tiếp theo được người dùng cho phép chạy compiler/build. Các giới hạn “không chạy test/build” ở mục 6 mô tả lượt sửa trước đó.

| Kiểm tra | Kết quả |
|---|---|
| `cargo check` trong `src-tauri` | Thành công, exit 0; không có diagnostic lỗi/cảnh báo Rust. Kiểm tra các target production mặc định, không kiểm tra target test. |
| `npm run build` | Thành công, exit 0; `tsc` và Vite hoàn tất. Có cảnh báo một số chunk sau minify vượt 500 kB; không sửa chia bundle trong phạm vi này. |
| Migration PostgreSQL | Rà tĩnh cú pháp DDL/DO, thứ tự và CHECK/UNIQUE với schema cũ; chưa thấy lỗi trực tiếp. Không chạy PostgreSQL parser/server hoặc áp dụng migration. |
| Phụ thuộc Worker | Production chỉ khai báo acquisition, connection_log, legacy, store; không có include/path/call tới sáu module Worker cũ. legacy vẫn đọc bằng chứng JSON để cutover. |
| Phạm vi Frontend | Diff hiện có chỉ ở as200Service và logic alert/telemetry tại Giám sát/Kết nối. Routes, layout, CSS, bảng/biểu đồ/bộ lọc, các trang Lịch sử/Chi tiết/Báo cáo và export không đổi. Chưa kiểm tra render hoặc dữ liệu runtime. |

Thứ tự migration là `20261109120000` → `20261111120000` → `20261117120000` → `20261117130000`, được SQLx nhúng bằng `migrate!` và nhận diện qua danh mục migrator. Migration publication thêm cột nullable để giữ row cũ, UNIQUE nguồn/phiên/counter chỉ áp cho publication mới; CHECK trạng thái cũ tiếp tục áp với row không có publication_session. Migration Done giữ NULL cho ACK attempted cũ, chỉ backfill false khi chưa attempted; default false áp cho state mới. Các CHECK mới tương thích các trạng thái hợp lệ của schema trước đó. Việc nhận diện CHECK cần thay thế trong migration publication dựa trên biểu thức catalog; cần xác minh trên schema đích nếu schema đã bị chỉnh ngoài migration.

Không phát hiện lỗi biên dịch/tương thích trực tiếp cần sửa source trong lượt kiểm tra này; chỉ bổ sung kết quả vào báo cáo. Không sửa tests.rs, migration hoặc cấu hình; không chạy test, ứng dụng, Simulator hoặc kết nối PLC/Neon/PostgreSQL. Không xóa/reset dữ liệu hay Journal. Build tạo/cập nhật artifact cục bộ trong target/dist theo công cụ hiện hữu.

Những giới hạn tại mục 5 vẫn còn: build thành công không xác nhận truy vấn SQL động, migration trên schema đích, timing/retention PLC hoặc proof Done cao → thấp. Các test Worker cũ chưa được cập nhật/chạy; cảnh báo kích thước bundle vẫn tồn tại. Chưa nghiệm thu luồng dữ liệu và UI với thiết bị/DB thật.

## 8. Clean code sau refactor — trạng thái hiện tại

### File đã XÓA thật

| File | Logic được loại bỏ |
|---|---|
| `src-tauri/src/plc/worker/ack.rs` | Điều phối năm pha ACK cũ, pause/quarantine và compaction delivery. |
| `src-tauri/src/plc/worker/capture.rs` | Các đường capture/RAW_TEST/archive/key/recovery cũ. |
| `src-tauri/src/plc/worker/delivery.rs` | Queue delivery, persist receipt và retry/manual retry Journal cũ. |
| `src-tauri/src/plc/worker/journal.rs` | Journal v4 writer/load/validate/persist/replace, intent/progress và các struct delivery/quarantine cũ. |
| `src-tauri/src/plc/worker/recovery.rs` | Recovery/receipt/prefix và resume pause của Worker cũ. |
| `src-tauri/src/plc/worker/retry.rs` | Generic retry store/trait/backoff cũ. |
| `src/pages/Dashboard/Dashboard.tsx` | Dashboard không còn import/route; `/dashboard` vẫn redirect `/monitor`. |
| `src/pages/Dashboard/Dashboard.css` | CSS chỉ được Dashboard đã bỏ import. |

Tám file trên chứa tổng 2.684 dòng trước khi xóa (sáu Worker: 2.140; Dashboard/CSS: 544). Không chuyển các implementation đó sang file khác; không tạo source file mới trong lượt cleanup. File tests.rs trong thư mục worker giữ nguyên.

### File sửa và API/dead code đã bỏ

| File | Thay đổi trong cleanup |
|---|---|
| `src-tauri/src/repositories/inbox_repository.rs` | Xóa find_optional, receive, pending, pending_for_source, scheduled_for_source, processed_for_ids, existing_delivery, session_total. Chỉ giữ resolve/lock nguồn và find/lock/raw_snapshots/processed/failed đang có consumer. |
| `src-tauri/src/services/inbox_service.rs` | Xóa InboxService::receive và validation/transaction ingest cũ không còn caller; giữ transaction projection và retry cùng Inbox ID. |
| `src-tauri/src/models/inbox.rs` | Bỏ public InboxInput; giữ InboxRecord, StoredWordOrder, PlcSource và DTO raw history đang dùng. |
| `src-tauri/src/plc/worker/legacy.rs` | Nhận đúng shape JSON lịch sử bằng LegacyInboxInput private chỉ Deserialize; không cung cấp API nhận raw cũ. |
| `src-tauri/src/plc/worker.rs` | Bỏ import InboxInput và clear danh sách task đã rỗng; lifecycle/ACK không đổi. |
| `src-tauri/src/plc/as200/ack.rs` | Xóa PublishedAckPhase, PublishedAckIntent cùng same_published_record/check_before_write/confirmed_by cũ. send chỉ nhận socket đã chuẩn bị, bỏ nhánh tự connect không caller; count convenience wrapper chỉ biên dịch cho test transport inline. Count/Done production vẫn qua prepare_count/count_prepared/prepare_done/done_prepared. |
| `src-tauri/src/plc/connection_settings.rs` | Xóa command save_plc_poll_interval không có consumer; gom helper save_connection_options vào command save_plc_connection_options đang dùng. Bỏ raw_save_attempted/raw_saved luôn false khỏi inspection DTO vì không có consumer. Probe vẫn chỉ đọc. |
| `src-tauri/src/commands/measurement_command.rs` | Xóa check_database_connection không có UI caller; giữ get_database_status/retry_database_connection đang dùng. |
| `src/services/measurementService.ts`, `src/services/plcConnectionService.ts` | Xóa hai wrapper export không được import: checkDatabaseConnection, savePlcPollInterval. UI vẫn lưu poll/local IP bằng savePlcConnectionOptions. |
| `src-tauri/src/lib.rs` | Bỏ đăng ký hai command đã xóa, bỏ plugin opener và sửa comment startup về acquisition. |
| `src-tauri/src/plc/simulator_config.rs` | Sửa log “receipt PROCESSED trước ghi” thành raw receipt PostgreSQL đã commit; ghi rõ option retry/queue cũ chỉ được parse để đọc TOML hiện hữu. Không đổi config hoặc quyền ACK. |
| `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` | Bỏ tauri-plugin-opener và dependency bắc cầu không còn cần; Cargo cập nhật lock khi check. |
| `package.json`, `package-lock.json`, `src-tauri/capabilities/default.json` | Bỏ @tauri-apps/plugin-opener và opener:default. Lock npm cập nhật bằng uninstall --package-lock-only --ignore-scripts. |
| `README.md`, `docs/PLC_BACKEND_REFACTOR.md` | Cập nhật trạng thái xóa thật, kiến trúc còn lại và kết quả kiểm chứng. |

Đã rà call site production, CLI, Tauri registration và Frontend trước khi bỏ. Không còn RAW_TEST/RAW_ZERO_TEST/raw-zero-test/raw-invalid hay kiểu ACK năm pha trong production source. Các tên lịch sử còn trong tests.rs/tài liệu không phải implementation production. Không phát hiện thêm declaration hàm public không có consumer production ngoài helper map chỉ cfg(test). Không coi số lần xuất hiện identifier là bằng chứng duy nhất: command/service/DTO được đối chiếu với caller thực tế và build.

### Cấu trúc và dependency còn lại

- `worker.rs`: một task, stop/join/reconnect, telemetry/DTO và command đang được Frontend gọi.
- `worker/acquisition.rs`: snapshot → receipt commit/read-back → Count/Done → quan sát; projection công bằng và khôi phục status.
- `worker/store.rs`: transaction publication, UNIQUE identity, pending pointer và trạng thái ACK tối thiểu.
- `worker/legacy.rs`: gate cutover chỉ đọc khi chưa có plc_acquisition; binding còn dùng để kiểm tra state khi startup/poll/đổi SKU. Không load/write Journal mỗi vòng và không fallback sang Worker cũ.
- `worker/connection_log.rs`: log kết nối/dừng phục vụ màn Kết nối hiện hữu, không phải Journal receipt.
- `as200/{reader,decode,map,model,heartbeat,ack}.rs`: transport/kiểu/map/snapshot/liveness/ACK whitelist; hai CLI reader và DB audit còn dùng và được giữ.
- Inbox/Measurement service, repositories, models và commands: raw history, projection idempotent và query/export báo cáo hiện hữu.

Cargo còn Tauri + plugin dialog/fs, Tokio/tokio-modbus, SQLx PostgreSQL, serde/serde_json/TOML, Chrono và BigDecimal; chúng đều có consumer production/CLI/build. npm còn React/React DOM/router, Ant Design/icons, Recharts, ExcelJS/XLSX, Tauri API/dialog/fs; toolchain TypeScript/Vite/Vitest và plugin React/type packages còn dùng cho build hoặc test hiện hữu. Không xóa dev dependency chỉ vì lượt này không chạy test. Không thêm dependency/framework.

### Compatibility thật sự còn cần

LegacyInboxInput chỉ giữ 10 trường cần đối chiếu delivery với Inbox; cùng gate SourceKey/binding/session/anchor/prefix, nó thay cho việc giữ cả Journal implementation. Gate vẫn từ chối pending intent, pause, quarantine/retry không rỗng hoặc state/history mâu thuẫn. Không tự nhận checkpoint từ PLC, sửa capture.json hoặc giải ACK chưa rõ.

`raw-published-v1:ack` và parser `published-sku-v1` còn consumer để chứng minh cutover, đọc SKU/ACK lịch sử. Không tạo key cũ mới. Không được bỏ parser chỉ vì tên có “raw” hoặc “Session”. Ba option TOML retry_max_attempts/retry_max_delay_ms/max_queue vẫn được deserialize vì file hiện hữu có thể chứa chúng và parser deny_unknown_fields; không còn queue/backoff tương ứng. journal_path và capture.lock còn cần để tìm evidence và khóa tương thích với binary cũ.

ManualRetryRequest/manualRetryResults, volatileCount và command acknowledge_as200_buffer_gap vẫn nằm trong contract UI hiện hữu. Command gap chỉ từ chối, không cho bỏ sequence; không xóa command đang được component import/gọi hoặc sửa markup để dọn một nhánh legacy trong lượt yêu cầu giữ nguyên UI này. Các helper map/transport cfg(test) còn dùng cho test thanh ghi/transport được giữ; không giữ máy ACK năm pha chỉ vì test cũ.

### Kiểm chứng và giới hạn

- `cargo check` sau các thay đổi production cuối: **exit 0**, không có lỗi/cảnh báo Rust.
- `npm run build` sau các thay đổi Frontend service cuối: **exit 0**, tsc/Vite hoàn tất; vẫn có cảnh báo chunk >500 kB. Output bundle có cùng tên/hash/kích thước với trước cleanup, phù hợp việc không đổi UI đang dùng.
- Đối chiếu SHA-256 trước/sau của 31 file được bảo vệ: các tests.rs, migration, TOML được kiểm kê, các TSX/CSS đang dùng không đổi. Tám file được chỉ định xóa không nằm trong nhóm UI được bảo vệ. Không sửa acquisition/store hoặc guard ACK trong lượt cleanup.
- Không chạy test/test target, ứng dụng, Simulator, migration hoặc kết nối PLC/Neon/PostgreSQL; không xóa dữ liệu, Journal runtime hay cấu hình. SQL runtime/Ladder/timing Done và các giới hạn ở mục 5 vẫn cần nghiệm thu riêng.

Các test cần cập nhật sau (giữ nguyên nội dung trong lượt này):

| File test | Phụ thuộc API cũ cần chuyển |
|---|---|
| `src-tauri/src/plc/worker/tests.rs` | Journal/Engine/Delivery/retry cũ, PublishedAckIntent/Phase, InboxInput, InboxService::receive và scheduled_for_source. Cần viết lại quanh state/receipt/Done-started của acquisition hiện tại. |
| `src-tauri/src/services/tests.rs` | Fixture InboxInput và API receive cũ; cần tạo publication fixture phù hợp store mới và kiểm tra projection/link cùng Inbox ID. |
| `src-tauri/src/plc/as200/tests.rs` | Các test proof dùng PublishedAckIntent/Phase đã xóa; proof hiện thuộc acquisition, transport/map/decoder test vẫn có nhiệm vụ riêng. |

Không xác nhận cargo test biên dịch. Không vô hiệu hóa module test hoặc giữ dead production API để che các tham chiếu cũ.

## 9. Clean Tests & Regression Verification

Ngày thực thi: **17/11/2026, giờ Việt Nam**. **Trạng thái tổng thể: PARTIAL** — toàn bộ test target biên dịch, các test offline chạy thành công; chưa có PostgreSQL test được xác minh để chạy SQL/acquisition/ACK tích hợp. Các kết luận “test chưa cập nhật/chưa biên dịch” ở mục 1–8 là lịch sử trước lượt này.

### A. Cleanup

Chỉ sửa sáu file test/helper và báo cáo này:

| File | Nội dung |
|---|---|
| `src-tauri/src/plc/worker/tests.rs` | Thay test Journal/Delivery/retry/backoff năm pha cũ bằng test WorkerStatus, binding, stop/join, gate legacy, status receipt và DB-unavailable. Thêm integration test gọi Engine/open/poll/receive/project_pending thật với PostgreSQL và peer Modbus loopback. |
| `src-tauri/src/services/tests.rs` | Fixture InboxRecord dùng metadata publication/assigned_sku hiện tại; test projection TIP P1/BUTT P1–4, loads/attempts, NULL/0, SKU, metadata/workflow và legacy SKU. Integration test transaction rollback, concurrency/idempotency và history. |
| `src-tauri/src/plc/as200/tests.rs` | Bỏ ba test PublishedAckIntent/Phase đã bị xóa. Giữ test scalar/map/FC03/response validation; thêm raw 0/một WORD, counter 0→1, 1/2/5/10 lần đọc, reconnect/unstable, FC03 timeout và mất response Count. Sửa test reader không còn áp validation của projection. |
| `src-tauri/src/plc/connection_settings/tests.rs` | Bỏ test clean_new_baseline/prepare_collection/read_new_baseline không tồn tại. Giữ chống ghi đè config/history và probe read-only; thêm connection-refused. Sửa giả định Unit ID 2 bị cấm: cấu hình đó hợp lệ, nhưng same_environment phải phát hiện thay đổi để ngăn đổi identity khi đang chạy. |
| `src-tauri/src/database/test_support.rs` | Fixture raw/InboxRecord dùng chung; pool test chỉ nhận URL test rõ ràng và tên golf_report_test trên loopback. Bỏ helper đọc config ứng dụng và test cũ tạo/migrate DB/audit theo schema một migration, tám điểm cũ; không tự provision/reset/migrate. |
| `src/utils/as200Report.test.ts` | Đổi fixture điểm chưa đo từ TIP P2 ngoài workflow sang BUTT P2. Giữ assertion export điểm chưa đo, decimal text, NULL/0, CSV/Excel và decision thực tế; thêm assertion TIP P2–P4 padding bị loại nhưng BUTT chưa đo vẫn giữ. Không sửa code export/UI. |

Không còn test tham chiếu Worker Journal cũ, PublishedAckIntent/Phase, public InboxInput, InboxService::receive, retry trait hoặc repository API đã bỏ. Không khôi phục production API, vô hiệu hóa module, hay chuyển logic Worker cũ vào mock. Giữ test scalar, map, heartbeat, transport ACK hiện hữu và các test report/monitor phù hợp.

Helper còn dùng: fixture 240 WORD hoàn tất **TIP NG tại P1 sau bốn attempt, BUTT P1–4 OK**, InboxRecord publication có SKU đóng băng, thư mục tạm riêng, closed lazy pool và peer Modbus loopback. Peer chỉ mô phỏng register/FC03/FC16/FC06 cùng mất response có kiểm soát; không thay SQL bằng mock trả Ok. Engine constructor trực tiếp trong unit test chỉ phục vụ hàm hiển thị/telemetry; không được tính là đã kiểm chứng DB/restart. Integration test dùng Engine::open thật.

### B. Kết quả thực thi

| Lệnh | Kết quả cuối | Số test / bằng chứng |
|---|---|---|
| `cargo check --all-targets` trong src-tauri | PASS, exit 0 | Production, CLI và test target biên dịch; không phải bằng chứng chạy test. |
| `cargo test` trong src-tauri | PASS, exit 0 | **61 PASS, 0 FAIL, 11 ignored = NOT RUN**; 72 test thư viện. Main/2 CLI/doc-test mỗi target 0 test. |
| `cargo check` trong src-tauri | PASS, exit 0 | Không lỗi/cảnh báo Rust. |
| `npm test -- --run` tại root | PASS, exit 0 | **23 PASS, 0 FAIL**, 3 file: connection service 4, MonitorPanels 9, report/export 10. |
| `npm run build` tại root | PASS, exit 0 | tsc/Vite thành công; còn cảnh báo chunk >500 kB. |

Tổng test thực thi cuối: **84 PASS, 0 FAIL; 11 test DB NOT RUN**. Không dùng --ignored. Tám test DB mới/viết lại cần PostgreSQL test; ba test DB opt-in hiện hữu (health read-only, point summary và report read-only) cũng không chạy. Không thêm ignore cho test offline lỗi.

Lần chạy chẩn đoán đầu: cargo check --all-targets báo API test cũ không tồn tại. Sau rewrite, lần chạy test đầu có Rust 55 PASS/1 FAIL/8 ignored và Frontend 18 PASS/4 FAIL. Lỗi Rust là assertion Unit ID 2 luôn bị cấm; bốn lỗi export cùng do fixture TIP P2 không còn là business point. Đã sửa fixture/expectation theo source hiện tại và giữ assertion bảo vệ tương đương, rồi chạy lại thành kết quả cuối ở trên. Không sửa production để làm test PASS. Không có timeout treo toàn suite; các timeout socket được kiểm tra là lỗi chủ động trong test offline.

Không tìm thấy psql/postgres/initdb trong PATH; không có biến môi trường AS200_/PUBLISHED_TEST_ cung cấp DB test đã xác minh ở đầu lượt. Đây là thiếu dependency kiểm thử, không phải kết luận PostgreSQL không được cài ở bất kỳ nơi nào. Helper mới yêu cầu `AS200_TEST_DATABASE=golf_report_test` và `AS200_TEST_DATABASE_URL` chỉ tới IP loopback, database đúng tên; xác minh server/database và migration history trước DML. Không đọc config/credential ứng dụng, không kết nối Neon, không tạo hay chạy migration tự động. Integration test chỉ insert/update fixture thuộc source test riêng, không xóa dữ liệu để cleanup.

### C. Regression Matrix

PASS dưới đây chỉ áp dụng đúng tầng đã chạy. NOT RUN là test tích hợp đã viết/biên dịch nhưng thiếu DB test. NOT VERIFIED là tình huống chưa có phép thử đủ để kết luận, dù có bằng chứng đơn vị liên quan.

| Nhóm | Tình huống | Test name | Kết quả | Bằng chứng/Giới hạn |
|---|---|---|---|---|
| A | Connection refused, IP sai định dạng | `tcp_connect_refused_or_bad_address_is_error_without_panic`, `inspection_connection_refused_is_not_a_valid_modbus_snapshot` | PASS | Chỉ endpoint loopback hoặc IP không parse được; không thử IP PLC thật. |
| A | Sai Unit ID/function/transaction/byte count | `tcp_fc03_wrong_unit_byte_count_function_or_short_data_is_rejected` | PASS | Phản hồi TCP giả bị từ chối đúng lớp protocol. |
| A | FC03 response timeout | `fc03_response_timeout_is_not_a_measurement_or_successful_snapshot` | PASS | Server giữ request không trả; snapshot trả Timeout. TCP connect timeout ngoài mạng chưa được chứng minh bằng ca này. |
| A | Mất kết nối lúc đọc, đọc lại sau lỗi | `read_failure_recovers_and_counter_change_mid_snapshot_is_unstable`, `tcp_fc03_exception_response_is_protocol_error_and_reconnects` | PASS | Fake read và TCP reconnect thực trên loopback. Không chứng minh auto-reconnect toàn Worker khi DB outage. |
| A | TCP thành công, PLC idle, heartbeat chưa đổi | `live_idle_snapshot_is_connected_but_requires_heartbeat_change_before_writing` | PASS | Hai lần đọc thật; CONNECTED khác OBSERVING/RUNNING. Không có write. |
| A | Heartbeat stale/wrap/phục hồi | `normal_stalled_wrap_and_recovery` | PASS | Đồng hồ monotonic giả lập bằng Instant, không sleep chờ Ladder. |
| A | Không publication mới | `published_record_not_ready_reads_only_control_then_observes_ready_level` | PASS | Ready=0 chỉ đọc control, không lấy RAM cũ làm measurement. |
| A | Đường auto-reconnect toàn Worker, TCP connect timeout/route thật | — | NOT VERIFIED | Có test reader reconnect; chưa có test đủ của lifecycle/mạng thật cho toàn đường này. |
| B | 240 WORD, vùng D25020, FC03≤100, không đọc slot 2 | `published_record_reads_only_first_region_and_accepts_ready_held_high`, `published_snapshot_over_tcp_never_reads_second_region` | PASS | Fake + FC03 TCP loopback; kiểm tra địa chỉ/số request/raw. |
| B | INT16/DINT32/REAL32, LOW/HIGH, ASCII | `scalar_decoding_both_word_orders_and_signed_boundaries`, `ascii_uses_low_byte_first_independent_of_numeric_word_order`, `full_record_preserves_all_fields_without_legacy_conversion` | PASS | Có signed boundary, NaN/Infinity, load và fields raw; không phải nghiệm thu word order thiết bị. |
| B | Ready=1 total=0: toàn 0 hoặc chỉ D25200=12; sau đó total=1 | `arbitrary_raw_total_zero_then_one_keeps_all_words_and_plc_identity` | PASS | Reader giữ đủ raw và counter thật, không tự tạo sequence/measurement. Không suy ra SQL commit/ACK từ test này. |
| B | Status bit0=0 / workflow/metadata invalid | `published_record_preserves_raw_without_enforcing_projection_validation`, `invalid_raw_zero_one_word_and_status_zero_are_preserved_without_fake_measurement` | PASS | Raw còn nguyên, projection từ chối; không lấy kết quả decode làm quyền ACK. |
| B | Snapshot/control/raw/counter đổi giữa hai lần đọc | `published_record_rejects_changing_raw_control_and_short_reads`, `read_failure_recovers_and_counter_change_mid_snapshot_is_unstable` | PASS | Err Unstable hoặc lỗi short read, không trả snapshot giả ổn định. |
| B/D | Counter 0 không ACK rồi tiếp nhận/ACK counter 1, raw toàn 0/một WORD | `postgres_raw_zero_one_word_counter_zero_to_one_and_distinct_identical_publications` | NOT RUN | Cần SQL state/transaction thật; test đã biên dịch. |
| C | Dữ liệu workflow hợp lệ, SKU, TIP/BUTT/load/attempt/time | `projection_preserves_frozen_sku_results_loads_attempts_and_time` | PASS | Tạo draft thật và kiểm tra đầy đủ 5 business point; không coi draft là row Measurement đã commit. |
| C | Thiếu SKU/session, seq/counter không khớp | `missing_sku_session_and_publication_identity_have_specific_projection_errors` | PASS | Kiểm tra lỗi cụ thể, không tự bổ sung dữ liệu. |
| C | Workflow sai/điểm TIP thừa/Overall sai | `workflow_rejects_extra_tip_points_and_inconsistent_overall_without_rewriting_raw` | PASS | Raw không bị sửa; không tạo OK/NG giả. |
| C | NG dừng sớm, unused NULL, zero/negative/decimal | `ng_stops_at_first_butt_point_and_keeps_unused_projection_values_null`, `unmeasured_slots_are_null_and_measured_zero_and_negative_values_remain_real_values` | PASS | Kiểm tra projection và numeric thật. |
| C | Metadata/NaN/thiếu ProductCode/attempt sai | `projection_rejects_metadata_nan_invalid_product_and_invalid_attempt_structure` | PASS | Validation phục vụ measurement, không được suy ra cấm raw ACK. |
| C | Tạo Measurement tree, rollback, concurrent retry cùng Inbox, history | `postgres_projection_transaction_retry_idempotency_and_history` | NOT RUN | Có assert rollback/link/unique/query; chưa thực thi SQL. |
| C | Commit thành công nhưng mất response ở wire PostgreSQL | — | NOT VERIFIED | Test DB dự kiến bỏ kết quả thành công rồi retry cùng ID chỉ mô hình hóa caller mất kết quả; chưa inject mất response tại COMMIT thật. |
| C | Lỗi projection rồi phục hồi do DB transient | — | NOT VERIFIED | Test transaction rollback/retry đã viết nhưng chưa chạy; chưa inject transient outage/COMMIT wire. |
| D | DB không dùng được thì không đọc PLC/ghi ACK | `database_unavailable_prevents_snapshot_and_any_ack_write` | PASS | Closed lazy pool, không mở kết nối PostgreSQL; peer PLC không nhận write. Không thay cho test rollback DB runtime. |
| D | Chuẩn bị Done chưa gửi byte, dùng đúng socket | `prepared_done_does_not_send_until_explicitly_consumed`, `prepared_stream_is_used_for_done_without_new_connection` | PASS | Test transport hiện hữu chạy; CAS PostgreSQL chưa được kiểm chứng bằng các ca này. |
| D | FC16 Count word order/FC06 Done, echo sai/exception/mất response | `fc16_word_order_offset_and_fc06_echo_are_checked`, `fc16_wrong_quantity_address_or_exception_is_rejected`, `fc06_wrong_echo_value_or_address_is_rejected`, `missing_or_bad_response_never_counts_as_success` | PASS | Transport TCP loopback; response lỗi không được trả Ok. Không chứng minh accepted đã tiến. |
| D | Count đã nhận nhưng response mất, không tự replay transport | `count_response_lost_returns_error_and_transport_does_not_replay_request` | PASS | Server nhận đúng một FC16 rồi cắt socket; client trả Err. State restart vẫn cần SQL test. |
| D | Receipt commit/read-back, CAS attempted, Count→Done, Done cao→thấp | `postgres_raw_zero_one_word_counter_zero_to_one_and_distinct_identical_publications` | NOT RUN | Test chạy API acquisition thật, nhưng cần PostgreSQL test. Thứ tự CAS/network chưa có trace runtime. |
| D | Count response mất, restart trước Done, Done response mất | `postgres_count_loss_first_done_restart_and_unknown_done_never_resends` | NOT RUN | Peer drop response có kiểm soát; chỉ được coi PASS sau khi chạy SQL. |
| D | attempted=true, Done true/NULL, không thấy Done cao | `postgres_restart_with_count_uncertain_or_done_unknown_never_replays_or_invents_proof` | NOT RUN | Khôi phục fixture durable state; assert không resend/accepted không tăng. Không mô phỏng kill process đúng microsecond. |
| D/E | Gap, raw đổi cùng identity, counter giảm, binding DB sai | `postgres_gap_changed_payload_reset_and_binding_do_not_ack_wrong_publication` | NOT RUN | Assert raw cũ/checkpoint không đổi và không write. |
| D | Count timeout riêng trong state machine; stop tại từng cửa sổ COMMIT/write | — | NOT VERIFIED | Có timeout transport Done/FC03 và Count disconnect; chưa inject đủ từng cửa sổ Count timeout/cancellation. Không suy PASS từ source. |
| E | Session/source/endpoint/unit/order/layout đổi phải đổi binding | `binding_changes_for_source_session_endpoint_unit_order_or_layout` | PASS | Kiểm tra binding thật; DB mismatch runtime thuộc test tích hợp. |
| E | Legacy version/session/source/binding/pending/pause/quarantine thiếu proof | `unsafe_legacy_evidence_is_rejected_without_database_network_or_file_rewrite` | PASS | Gate từ chối trước truy cập DB; JSON tạm không đổi. Không xóa bằng chứng runtime. |
| E | Sentinel source_id=0, last_total=accepted+1, anchor/prefix DB thật | `postgres_legacy_sentinel_and_next_total_require_real_anchor_and_complete_prefix` | NOT RUN | Reader cutover thực + SQL fixture; không khôi phục Journal implementation. |
| E | DB mất lịch sử/native receipt nhưng state ACK thiếu | — | NOT VERIFIED | Guard có trong source; chưa có test đã thực thi đủ để chứng minh trường hợp này. |
| E | Receipt hiển thị ACK riêng, row0 vẫn unconfirmed | `raw_receipt_display_uses_own_ack_identity_and_preserves_zero_unconfirmed` | PASS | Gọi hàm display thật với Progress fixture; không chứng minh hydrate từ DB sau restart. |
| E | lastSavedId/link, không ghi đè bằng receipt cũ | `projection_display_restores_measurement_link_and_never_overwrites_with_older_receipt` | PASS | Giới hạn ở display; hydration DB cần test tích hợp. |
| E | Stop/join/marker không gửi ACK dọn queue | `stop_joins_task_and_keeps_durable_operator_marker_without_ack` | PASS | Task kiểm soát bằng stop flag/channel, file nằm trong thư mục test riêng. |
| F | 1/2/5/10 snapshot liên tiếp, payload giống nhau, counter khác nhau | `repeated_identical_payloads_have_distinct_control_counters_for_one_two_five_ten_reads` | PASS | Chứng minh reader không gộp payload; chưa chứng minh UNIQUE DB/không mất measurement. |
| F | 1/2/5/10 measurement commit, retry, restart giữa batch | `postgres_one_two_five_ten_measurements_retry_and_restart` | NOT RUN | Đếm Measurement riêng từng mốc; không đếm raw như measurement. |
| F | Nhiều FAILED vĩnh viễn không chặn bản ghi hợp lệ mới | `postgres_permanent_projection_failures_do_not_starve_new_measurement` | NOT RUN | 12 UNIQUE conflict thật + bản ghi mới, cursor/quota production; không mock trả Ok. |
| C/F/UI | CSV/Excel giữ decision/NULL/decimal/loads, loại TIP padding | `as200Report.test.ts` (10 test), `MonitorPanels.test.ts` (9 test) | PASS | Test hàm/export offline; không mở UI thật hoặc dùng số liệu từ DB test. |

Đã đọc ô workbook `docs/X212PI-Register-Mapping-PLC-Software.xlsx` bằng ZIP/XML, SHA-256 `ba9577a90b724cde8a348d31b468572bce50b1f2de871e900e3ab365a102cf07`. Đối chiếu RecordSize=240, Count D25008 DINT, Done D25012, RecordSeq offset22, BUTT_Load offset28/TIP_Load offset124 REAL đơn vị g, D25200 offset180 là WORD đầu TIP P3 A1 Bend. **Không có trường Weight riêng** trong workbook/source hiện tại; regression load kiểm tra đúng hai giá trị này, không tự thêm Weight. Workbook không đủ chứng minh Ladder publish slot0, timing Done, LOW trên thiết bị hoặc luật hoàn tất measurement được nghiệm thu: các nội dung đó **NOT VERIFIED**.

### D. Production Issues

**Chưa phát hiện bug production được test thực thi chứng minh trong lượt này.** Không thay Worker, decoder, transaction, schema, migration hoặc UI để sửa các lỗi fixture cũ. Lỗi ban đầu được phân loại:

| Vị trí test | Nguyên nhân / bằng chứng | Phạm vi tác động và xử lý |
|---|---|---|
| `connection_settings/tests.rs`, fixed_identity_and_ack_permission_cannot_be_changed_by_connect | Assertion cũ đòi validate_config từ chối unit_id=2; source cho phép Unit ID cấu hình hợp lệ, còn same_environment ngăn đổi khi đang chạy. | Lỗi expectation test; bổ sung assertion freeze thay vì cấm Unit ID 2 hoặc nới guard production. |
| `as200Report.test.ts`, fixture record và bốn assertion export | TIP P2 ngoài business point nên bị filter theo code hiện tại; test trông đợi row không còn thuộc workflow. | Lỗi fixture test; chuyển điểm chưa đo sang BUTT P2, thêm assertion TIP padding bị loại và giữ nguyên assertion export/NULL/precision. |

Không coi thiếu DB test là bằng chứng production lỗi hoặc chứng minh production an toàn. Các nguy cơ đã biết ở mục 5 (Done-high bị bỏ lỡ, attempted chưa rõ, reset/history conflict) vẫn chưa được loại trừ bằng runtime. Đề xuất tiếp theo chỉ là chạy test tích hợp trên DB test được xác minh, rồi kiểm chứng Ladder/trace; nếu test tái hiện bug mới, sửa production trong lượt được cho phép riêng.

### E. Giới hạn còn lại

- **Đã kiểm thử offline:** unit decode/projection/map/heartbeat/status/binding/legacy-reject; socket Modbus loopback thật; Frontend helper/report/export. Rust test harness chạy main/CLI target có 0 test, không gọi entrypoint ứng dụng hay khởi động acquisition thật ngoài fixture loopback.
- **Cần PostgreSQL test:** tám test tích hợp mới/viết lại và ba test opt-in cũ chưa chạy. Test helper không tự tạo/reset/migrate DB. Các test read-only cũ yêu cầu target riêng của chúng, không được chạy hàng loạt --ignored với config ứng dụng.
- **Cần PLC Simulator + PostgreSQL test:** kiểm tra trọn lifecycle polling/reconnect/stop, tốc độ và backlog dài; peer loopback hiện có chỉ là mô hình giao thức có kiểm soát.
- **Cần Delta AS200 thật/Ladder/trace:** retention qua mất điện, thứ tự cập nhật Ready/raw/Total/Done, width/latch Done, route/interface, Unit ID thực và LOW thực tế. Không coi test đọc 10 snapshot hoặc build thành công là chứng nhận không mất dữ liệu ở nhà máy.
- Chưa inject mất response PostgreSQL COMMIT thật, OS process kill/power loss hoặc mọi cửa sổ cancellation. Các integration test viết mới chỉ được xác nhận biên dịch, không được báo PASS khi chưa chạy.

Đối chiếu SHA-256 **70 file production/UI/migration** trước/sau lượt: không thay đổi, không thêm file production. Thay đổi chỉ ở test/helper và Markdown. Không đọc DB config để kết nối, không chạy migration/ứng dụng/Simulator, không chạm Journal runtime hay cấu hình PLC. Không che FAIL bằng ignore; 11 ignored phản ánh dependency DB chưa được xác minh. Vì còn các ca tích hợp NOT RUN/NOT VERIFIED, kết quả tổng thể vẫn **PARTIAL**, dù cả năm lệnh đều exit 0.

## 10. Kiểm tra auto migration Neon khi chạy Tauri DEV

Ngày: **17/11/2026**. Lượt này cho phép kiểm tra DB production chỉ đọc, không áp migration. Các giới hạn “không kết nối DB” ở mục trước mô tả lượt test trước đó.

### Kết quả kiểm tra thực tế

Đã chạy CLI `as200-db` hiện hữu **không có --migrate**, với startup option `default_transaction_read_only=on` cho connection string Cloud. Không gọi entrypoint Tauri hoặc Worker. Audit hoàn tất exit 0, SELECT xác nhận:

| Thuộc tính | Kết quả |
|---|---|
| Config DEV được chọn | `C:\Users\Daz\Desktop\TT\golf-report\src-tauri\resources\database_config.toml` |
| Database thực tế (`current_database()`) | `golf_report` |
| Endpoint trong connection string | `ep-long-shape-azaeb4rd.c-3.ap-southeast-1.aws.neon.tech:5432` |
| Schema thực tế | `public`; search_path `"$user", public` |
| Bảng trong các schema không phải hệ thống | Chỉ `public._sqlx_migrations` |
| Migration history | **0 dòng** |
| Cấu trúc tracking | version bigint, description text, installed_on timestamptz, success boolean, checksum bytea, execution_time bigint; đều NOT NULL |
| Quyền role | Có CREATE trong schema public; không có nghĩa đã thử DDL thành công |
| Advisory lock của DB | Không có tại thời điểm audit cuối |
| Binary audit đang biên dịch | Nhúng đủ 4 migration theo đúng thứ tự |
| Chế độ audit | `transaction_read_only=on` do CLI cố ý đặt; không suy ra cấu hình read-only của phiên ứng dụng từ kết quả này |

Không có password/token/username/connection URL đầy đủ trong báo cáo hoặc output audit. Tên branch thân thiện/branch ID trên Neon Console **chưa xác minh**: hostname chỉ xác định endpoint được cấu hình. Cần đối chiếu endpoint đó với branch trên Console; không tự gọi API quản lý Neon hoặc yêu cầu token. Không có PG*/DATABASE_URL trong môi trường shell được kiểm tra để đổi target cấu hình.

**Kết luận nguyên nhân ở mức bằng chứng hiện có:** chưa có migration nào được ghi nhận áp dụng thành công trên endpoint/database này, nên chưa có tám bảng ứng dụng. Schema thực tế không hoàn toàn trống: tracking table đã tồn tại. Audit chỉ đọc không tạo bảng này. Không có bằng chứng AppData ghi đè hoặc kết nối nhầm database name. **Chưa xác định được lỗi cụ thể của lần startup trước** vì chưa có log CONNECTION_FAILED/MIGRATION_FAILED và không được chạy migrator production trong lượt này. Tracking rỗng có thể tồn tại sau một lần runner dừng/lỗi trước khi commit migration đầu; không đủ để khẳng định riêng nguyên nhân đó hoặc một câu lỗi SQL cụ thể.

### Config và đường startup

- `database/config.rs::load_config`: debug_assertions dùng CARGO_MANIFEST_DIR/resources/database_config.toml, không phụ thuộc cwd, không fallback AppData/config root/resource template. Release đọc resources/database_config.toml cạnh EXE qua application_directory. Không có code đọc AppData hoặc copy AppData đè resources trong đường này.
- Config hiện có trường connection_string. Target lấy từ URL, không từ các trường legacy host/database đang để mặc định rỗng. TLS VerifyFull được áp bởi code. Tauri bundle resources map đúng đường dẫn này; mapping bundle không làm DEV ưu tiên AppData.
- `lib.rs::run/setup`: spawn initialize DB, await xong rồi mới launch Worker. Lỗi kiểm tra thư mục/config PLC được giữ riêng; không bỏ qua initialize DB. Không sửa lifecycle này.
- `DatabaseState::connect_and_migrate`: tạo PgPool từ đúng options trên, gọi migrations::migrate(&pool), chỉ trả/chia sẻ pool sau khi migration thành công. Không tạo database mới.
- `migrations.rs`: audit → validate_history → MIGRATOR.run trên cùng pool. SQLx nhúng SQL ở compile time; không cần Neon tự tìm file SQL. Runner dùng tracking table/checksum và advisory lock; chỉ chạy migration chưa áp dụng.

Thứ tự đã kiểm tra đầy đủ nội dung bốn file:

1. `20261109120000_initialize_as200.sql`: 6 bảng chính và hàm kiểm tra raw 240 WORD.
2. `20261111120000_plc_source_registry.sql`: registry lịch sử.
3. `20261117120000_publication_receipts.sql`: metadata/UNIQUE publication, CHECK tương thích và plc_acquisition.
4. `20261117130000_done_write_evidence.sql`: done_started nullable và CHECK bảo toàn ACK chưa rõ.

Không phát hiện lỗi cú pháp trực tiếp khi rà tĩnh; chưa thực thi DDL nên không chứng nhận migration runtime thành công. Với trạng thái audit hiện tại (chỉ tracking rỗng), validate_history cho phép chạy cả bốn. Sau khi hoàn tất dự kiến có **8 bảng ứng dụng + _sqlx_migrations**, tracking 4 dòng success.

Phân biệt lịch sử: nếu tracking có dòng migration đã áp dụng mà bảng tương ứng mất, validate_history trả MIGRATION_FAILED thay vì tự chạy lại hoặc reset tracking. SQLx cũng từ chối checksum/version không tương thích. Trạng thái hiện tại **không phải** trường hợp “history đã áp dụng còn nguyên nhưng bảng nghiệp vụ đã bị xóa”. Nếu schema bị sửa sau khi pool đã khởi tạo, SELECT 1 chỉ chứng minh liveness, không kiểm tra lại đầy đủ schema mỗi poll; restart mới chạy gate migration. Không thêm cơ chế sửa schema tự động.

### Sửa tối thiểu trong lượt này

| File | Thay đổi |
|---|---|
| `src-tauri/build.rs` | Thêm cargo:rerun-if-changed=migrations để file migration mới thêm kích hoạt rebuild. Source SQLx macro 0.9 hiện dùng include_str cho file đã có; tracking cả directory chỉ có ở nhánh macro unstable. Đây là thiếu sót build DEV được xác định từ source; **chưa chứng minh là nguyên nhân của DB đang thiếu bảng**. |
| `database/config.rs` | Log đường dẫn config thực chọn, không log nội dung/URL. Không đổi thứ tự load. |
| `database/connection.rs` | Log host/port/database trước connect và state/message khi startup thất bại. Không log credentials, không đổi TLS/pool/Worker. |
| `database/migrations.rs` | Log danh mục embedded/applied/tables và giai đoạn CHECKING/APPLYING_PENDING/COMPLETE. Giữ nguyên validate_history/runner/guard, không reset tracking. |
| `src-tauri/src/bin/as200-db.rs` | Sửa output Database rỗng khi URL chứa DB nhưng trường config.database rỗng. Audit đọc current_database/schema/search_path/CREATE permission, catalog bảng/tracking và advisory locks; in danh sách migration nhúng. Mặc định Cloud vẫn chỉ đọc, chỉ --migrate mới có nhánh migrator. |
| `docs/PLC_BACKEND_REFACTOR.md` | Báo cáo này. |

Không sửa config DB/PLC, SQL migrations, UI, tauri.conf.json, lib.rs, Worker hoặc ACK. Không drop/truncate/reset, không áp migration hoặc tạo database mới.

### UI và cách chạy tiếp

CONNECTION_FAILED dùng cho lỗi load config/URL/TCP/TLS/auth; MIGRATION_FAILED dùng cho lỗi audit/history/runner sau connect. Khi lỗi, pool không được publish; status polling không tự đổi lỗi đó sang CONNECTED chỉ nhờ TCP còn hoạt động. Frontend nhận đúng enum qua DTO và MainLayout hiển thị message lỗi cùng nút Thử kết nối lại, poll 2 giây. Hai trạng thái cùng dùng Alert error nhưng message khác; không sửa UI. Retry khi chưa có migrated pool sẽ chạy lại connect_and_migrate.

Từ root project, sau khi thoát phiên DEV cũ để nạp binary/config mới:

```powershell
npm run tauri dev
```

Không cần CREATE DATABASE, CLI --migrate, hay xóa _sqlx_migrations. Startup hiện hữu sẽ tự áp cả bốn migration còn pending vào golf_report đã cấu hình. Xem terminal theo thứ tự DATABASE_CONFIG → DATABASE_CONNECT → DATABASE_MIGRATION CHECKING → APPLYING_PENDING → COMPLETE. Nếu không COMPLETE, giữ nguyên history/data và lấy DATABASE_STARTUP state/reason hoặc message Alert để xác định lỗi thực tế; không tự reset để chạy tiếp. Đối chiếu đúng endpoint/branch/database/public trên Neon Console rồi refresh danh sách bảng.

Lệnh trên khởi động toàn ứng dụng và sau DB startup sẽ đi tiếp lifecycle Worker theo config PLC hiện hữu; **agent không chạy lệnh này trong lượt kiểm tra**. Không dùng nó như một công cụ migration-only nếu không muốn Worker hoạt động. Các thay đổi trên không bật/tắt PLC hoặc cấp lại quyền ACK.

Kiểm chứng: cargo check cuối **exit 0**, npm run build **exit 0** (còn cảnh báo chunk >500 kB); audit SELECT cuối **exit 0**. Một lượt audit trung gian qua cargo run hết hạn 75 giây khi có Cargo khác giữ khóa build; không được tính là bằng chứng lỗi Neon/migration. Không sửa test hay chạy test DB production. Kết quả tạo bảng bằng startup vẫn **chưa thực thi trong lượt này**, không báo migration production PASS.

## 11. Chạy PostgreSQL integration test trên Neon hiện hữu

Ngày **17/11/2026**. Theo yêu cầu cho phép riêng của người dùng, đã thực thi đủ **11/11 test DB: PASS; 0 FAIL, 0 NOT RUN** trên `golf_report`, schema `public`. Kết quả này cập nhật phần NOT RUN tại mục 9; không biến các giới hạn Ladder/thiết bị thật thành PASS. Không chạy Tauri, không gọi lifecycle Worker production, không kết nối PLC thật. Engine trong integration fixture chỉ dùng peer Modbus TCP `127.0.0.1` với port tạm.

### Target và điều kiện chạy

Đọc connection string từ config DEV hiện hữu, không sửa config hoặc in URL/credentials. Trước DML, helper kiểm tra chính xác endpoint `ep-long-shape-azaeb4rd.c-3.ap-southeast-1.aws.neon.tech:5432`, database `golf_report`, schema `public`, TLS VerifyFull và bốn migration success phù hợp danh mục nhúng. Audit trước test thấy đủ tám bảng ứng dụng; không tạo DB/branch, không chạy migration. Không xác minh tên branch thân thiện qua Neon management API.

Helper yêu cầu opt-in rõ `GOLF_NEON_INTEGRATION=I_ACCEPT_golf_report` và run ID riêng. Lượt này dùng **`grit-1794928744834`**, Session **`grit-1794928744834-session`**, SKU **`IT-grit-1794928744834`**. Không dùng SourceKey `golf-report-plc-01`. Để chạy lại phải dùng run ID mới; dữ liệu nguồn có block/ACK uncertain của lượt này được cố ý giữ nguyên.

Lệnh thực thi 11 test, từ `src-tauri`, với hai biến môi trường trên:

```powershell
cargo test --lib -- --ignored --test-threads=1 --nocapture
```

Test vẫn ignored theo mặc định để một lệnh cargo test thông thường không tự ghi Neon. Chạy tuần tự giúp kiểm kê và đối chiếu snapshot; không sửa production để làm test PASS.

### Kết quả từng test

Tên dưới đây là tên hàm; health nằm tại database/connection, hai test report tại repositories/measurement_repository, test projection transaction tại services, bảy test còn lại tại plc/worker.

| Test | Kết quả | Dữ liệu/transaction và bằng chứng đã chạy |
|---|---|---|
| `postgres_health_probe_recovers_status_on_the_same_pool_read_only` | PASS | Pool read-only, SELECT health trên cùng pool; không giả lập outage Neon thật. |
| `postgres_count_loss_first_done_restart_and_unknown_done_never_resends` | PASS | Receipt/state DB thật; loopback làm mất response Count, tiếp tục Done lần đầu và restart; không resend Done uncertain. |
| `postgres_gap_changed_payload_reset_and_binding_do_not_ack_wrong_publication` | PASS | Bốn nguồn riêng cho gap, payload đổi cùng identity, counter giảm và binding sai; giữ checkpoint/raw, không ACK sai. |
| `postgres_legacy_sentinel_and_next_total_require_real_anchor_and_complete_prefix` | PASS | Legacy sentinel source_id=0, receipt/prefix thật; accepted+1 chỉ qua khi anchor/history đủ. JSON fixture riêng, không sửa Journal runtime. |
| `postgres_one_two_five_ten_measurements_retry_and_restart` | PASS | Mốc 1/2/5/10 Measurement; retry/restart qua Engine và PostgreSQL, không đếm raw thay Measurement. |
| `postgres_permanent_projection_failures_do_not_starve_new_measurement` | PASS | 12 conflict Measurement có chủ đích và receipt hợp lệ mới; quota/cursor cho bản mới được xử lý. |
| `postgres_raw_zero_one_word_counter_zero_to_one_and_distinct_identical_publications` | PASS | Lưu đủ 240 WORD, toàn 0/một WORD, counter 0→1, các publication cùng payload khác counter; transaction/UNIQUE/Count→Done và proof cao→thấp. |
| `postgres_restart_with_count_uncertain_or_done_unknown_never_replays_or_invents_proof` | PASS | Ba nguồn có Count uncertain, Done true và Done NULL; restart chỉ quan sát, không tự tạo bằng chứng hoặc replay. |
| `point_summary_merges_skus_from_decision_samples` | PASS | Hai fixture SQL trong TEMP tables trên hai session riêng; decision samples và nhóm SKU, không ghi đè public tables. |
| `postgres_report_filters_pagination_and_nested_details_read_only` | PASS | Tạo Measurement tree riêng bằng service transaction, rồi pool read-only kiểm tra report/filter/pagination/nested detail và thống kê theo SKU riêng. |
| `postgres_projection_transaction_retry_idempotency_and_history` | PASS | Transaction rollback, projection retry/concurrent idempotency, link cùng Inbox và truy vấn history trên nguồn riêng. |

Lượt 11 test hoàn tất exit 0 trong **200,15 giây**. Sau toàn bộ DML, chạy lại riêng health test với pool read-only để kiểm kê cuối: PASS, snapshot ngoài run không đổi. Đây là chạy lặp một test, không cộng thành test thứ 12.

| Kiểm tra sau cùng | Kết quả |
|---|---|
| `cargo check --all-targets` | PASS, exit 0 |
| `cargo test` | PASS, exit 0: 61 PASS, 0 FAIL, 11 ignored theo mặc định; cả 11 đã chạy PASS bằng lệnh explicit phía trên. CLI/main/doc-test không có test. |
| `npm test -- --run` | PASS, exit 0: 23 PASS, 0 FAIL, ba file. |
| `npm run build` | PASS, exit 0; còn cảnh báo chunk >500 kB. |

Tổng **95 test khác nhau PASS, 0 FAIL**: 72 Rust (61 offline + 11 DB), 23 Frontend. Không có test DB NOT RUN trong danh sách 11 này.

### File thay đổi trong lượt này

- `database/test_support.rs`: guard Neon/DB/schema/migration, opt-in, namespace riêng, read-only pool, snapshot dữ liệu ngoài run và inventory. Không provision/migrate/cleanup.
- `plc/worker/tests.rs`, `services/tests.rs`: fixture dùng SourceKey/Session/SKU riêng, SQL bind đúng identity; endpoint loopback bắt buộc, timeout fixture phù hợp độ trễ Neon.
- Chỉ module test inline trong `database/connection.rs` và `repositories/measurement_repository.rs`: dùng verified pool; report có fixture riêng để assertion không PASS rỗng; TEMP measurements có source_id tương thích query hiện tại; hai session TEMP thay TRUNCATE; assertion điểm TIP=1/BUTT=4 theo workflow hiện tại.
- Báo cáo này và `docs/NEON_INTEGRATION_TEST_CLEANUP.sql` (hướng dẫn cho người dùng, chưa thực thi).

Đối chiếu SHA-256 trước/sau **72 file/phần production, UI, migration và cấu hình: không đổi**. Hai file chứa test inline chỉ đổi phần test. Không sửa transaction/ACK production, không thay migration hoặc cấu hình database ứng dụng. Không phát hiện bug production qua 11 test đã thực thi; các chỉnh sửa là helper/fixture phù hợp target và schema hiện hữu.

### Manifest dữ liệu test giữ lại

Mọi SourceKey bên dưới có tiền tố **`grit-1794928744834-`**; ghép tiền tố với cột hậu tố để được key đầy đủ. Session dùng chuỗi riêng nêu trên; ba nguồn legacy/report/services không có plc_acquisition. Legacy có SQL session NULL và Session nằm trong key/JSON fixture. Không nguồn nào dùng Session production.

| source_id | Hậu tố SourceKey | Inbox | Measurement |
|---:|---|---:|---:|
| 2 | worker-0 | 2 | 0 |
| 6 | worker-1 | 1 | 0 |
| 7 | worker-2 | 1 | 0 |
| 8 | worker-3 | 1 | 0 |
| 9 | worker-4 | 1 | 0 |
| 10 | legacy-5 | 1 | 0 |
| 11 | worker-6 | 10 | 10 |
| 14 | worker-7 | 13 | 13 |
| 16 | worker-8 | 4 | 0 |
| 19 | worker-9 | 1 | 0 |
| 21 | worker-10 | 1 | 0 |
| 23 | worker-11 | 1 | 0 |
| 25 | report-12 | 0 | 1 |
| 26 | services-13 | 1 | 1 |

| Bảng | Số row thuộc run sau cùng |
|---|---:|
| plc_sources | 14 |
| plc_inbox | 38 |
| plc_acquisition | 11 |
| measurements | 25 |
| measurement_stations | 50 |
| measurement_points | 125 |
| measurement_attempts | 500 |
| plc_source_registry | 0 |

Các trạng thái block/pending/Done uncertain trên nguồn test là fixture dự kiến, không phải checkpoint production cần sửa. TEMP tables tự hết theo session, không còn trong public schema. Sequence ID có tăng do INSERT/rollback/ON CONFLICT; không reset sequence.

### Ảnh hưởng và giới hạn

Snapshot trước test và audit read-only sau test đối chiếu count + hash nội dung theo nguồn ngoài namespace run, trên tám bảng ứng dụng và tracking. **Một Source có sẵn không đổi; không có Inbox/Measurement/acquisition production trước hoặc sau; `_sqlx_migrations` vẫn đúng bốn dòng không đổi.** Không sửa checkpoint ACK production, không reset/delete dữ liệu trong lượt chạy. Chỉ ghi các row test có nguồn riêng. Kết quả snapshot là đối chiếu trước/sau, không phải trace mọi thao tác của client khác.

Vì test được phép ghi vào cùng public schema, các query UI không lọc SKU/source có thể hiển thị raw test và tính thêm **25 Measurement test** cho tới khi dọn. Lọc SKU riêng để kiểm tra; không coi namespace riêng là cách tự ẩn test trên UI. Không đổi UI để che dữ liệu đó.

Chưa nghiệm thu PLC thật/Ladder, retention/mất điện, timing Done thực tế hoặc kill OS tại mọi cửa sổ COMMIT/write. Restart test dựng lại Engine với trạng thái durable và peer loopback, không cắt điện laptop. Caller bỏ kết quả commit rồi retry không thay cho inject mất response COMMIT trên wire PostgreSQL. PASS tích hợp này không loại bỏ các giới hạn ACK uncertain đã ghi tại mục 5.

### Dọn dữ liệu sau khi người dùng xem kết quả

**Agent chưa chạy cleanup/reset.** File [NEON_INTEGRATION_TEST_CLEANUP.sql](NEON_INTEGRATION_TEST_CLEANUP.sql) chỉ nhắm đúng 14 cặp source_id/SourceKey trong manifest, có guard database, kiểm tra manifest và tham chiếu ACK bên ngoài. Xem lại endpoint/database trên Neon trước khi dùng. Đóng các client dùng nguồn test này; chạy script ở chế độ mặc định **ROLLBACK** để xem số row, đối chiếu manifest. Khi muốn áp dụng thật, người dùng chủ động đổi duy nhất dòng ROLLBACK cuối thành COMMIT và chạy lại toàn transaction. Nếu guard/FK/count khác dự kiến, dừng đối chiếu, không mở rộng điều kiện DELETE.

Script xóa dữ liệu của các nguồn test theo thứ tự FK, không đụng `_sqlx_migrations`, nguồn production, config/Journal hoặc sequence. Không DROP/TRUNCATE bảng ứng dụng, không DELETE toàn bảng, không reset database. Sau cleanup, refresh UI và kiểm tra các SourceKey test không còn; không dùng lệnh reset schema hoặc xóa history migration.

## 12. Số đo bằng 0 và bản ghi toàn 0

Ngày **17/11/2026**. Đã đọc lại luồng reader → store → projection → repository → Giám sát/Lịch sử và workbook `docs/X212PI-Register-Mapping-PLC-Software.xlsx` bằng ZIP/XML, cả ba sheet. SHA-256 vẫn là `ba9577a90b724cde8a348d31b468572bce50b1f2de871e900e3ab365a102cf07`. Không lấy việc bật Ready làm chứng cứ phép đo hoàn tất.

### Nguyên nhân bản ghi toàn 0 không thành Measurement

`reader::poll_published_record` chỉ đọc record khi DataReady=1, kiểm tra control/raw ổn định và giữ đủ 240 WORD kể cả toàn 0. Reader có diagnostic ZeroPayload và không tự tạo record decoded cho payload này. `store::receive` lưu raw, RecordSeq gốc và identity publication vào Inbox; không dùng kết quả chất lượng làm điều kiện lưu raw.

Projection `InboxService` thử decode độc lập từ raw đã commit. Với 240 WORD toàn 0, lỗi đầu tiên là **Version=0**, trong khi `Cau_truc_ban_ghi!L5` yêu cầu 1. Kể cả chỉ sửa Version, các trường sau vẫn không chứng minh được phép đo:

| Trường | Địa chỉ slot 0 | Toàn 0 nghĩa gì / yêu cầu |
|---|---|---|
| Version | D25020 | 0 chưa phải định dạng hỗ trợ; cần 1. |
| Status bit0 | D25021 | 0 chưa đánh dấu bản ghi hợp lệ/chưa xác nhận. Measurement yêu cầu bit0=1; không dùng bit này để chấm OK/NG. |
| ProductCode | D25024–33 | Trống. Workflow hiện yêu cầu mã ASCII có nội dung; SKU gán khi nhận là metadata riêng, không ghi đè payload. |
| OverallResult | D25040 | 0 = chưa có kết quả, 1 = OK, 2 = NG (`L9`). |
| RecordSeq | D25042–43 | 0 là số nguyên đọc được; phải khớp identity/counter của publication và Inbox. Không tự đổi thành 1. |
| BUTT_Result / TIP_Result | D25044 / D25140 | 0 = chưa đo; Measurement hoàn tất yêu cầu kết quả PLC 1 hoặc 2, khớp Overall. |
| PointCount / NGPoint | BUTT D25045/46; TIP D25141/42 | Count=0 chưa có điểm đã đo. NGPoint=0 hợp lệ khi trạm OK hoặc chưa đo, không phải lỗi số đo. |
| Point Result / AttemptCount / DecideAttempt | BUTT P1 D25052/53/54; TIP P1 D25148/49/50 | Result=0, Count=0, Decide=0 mô tả điểm chưa đo; không được tự tạo attempt có kết quả bằng 0. Các điểm BUTT tiếp theo stride 22 WORD. |

`DataReady` D25011 là control báo có publication chưa xác nhận (`Vung_dieu_khien!I13`), không thay cho Version, kết quả trạm/điểm hay số lượt thực hiện. `TotalCount` D25004–05 nằm ngoài 240 WORD, nên từ payload toàn 0 không suy được TotalCount thực tế của lần thử. Workbook yêu cầu RecordSeq bằng TotalCount tại lúc tạo record (`Cau_truc_ban_ghi!L11`). TotalCount=0 không tự thành publication 1; quy tắc ACK counter kế tiếp >0 vẫn giữ nguyên. Projection không cấm riêng sequence=0 khi metadata khớp, nhưng điều đó không bỏ điều kiện hoàn tất hay cấp ACK cho counter 0.

Không có bằng chứng độc lập về control/Ladder của lần thử người dùng mô tả; không kết nối thiết bị để đo lại. Theo source, toàn 0 thiếu metadata và completion, **không phải bị loại vì Bend/PushDist/Load bằng 0**.

### Những số 0 được giữ nguyên

REAL32 hữu hạn **0, số âm, số dương** ở Position, PushDist, Bend, BUTT_Load/TIP_Load đều được decoder và projection chấp nhận, không chia 100 và không suy quality từ trị số. Số đo 0 ở lượt đã thực hiện được lưu NUMERIC 0, giữ `Some(0)`, không đổi thành NULL. Load ở D25048–49 và D25144–45 có đơn vị g. Không có trường Weight riêng trong mapping. NaN/Infinity không thành numeric measurement; raw vẫn được giữ.

Phân biệt: Result=0 là UNMEASURED; AttemptCount=0 không có lượt thực hiện; DecideAttempt=0 thành NULL; điểm chưa đo có position NULL và không có attempt row. NGPoint=0 hợp lệ với trạm OK; reserve/padding bằng 0 hợp lệ; StickNo=0 không bị cấm riêng. Giá trị 0 của số đo không tự chứng minh đã đo.

### PLC cần công bố gì để Measurement có các giá trị đo 0?

PLC cần công bố record ổn định đúng layout/word order: Version=1, Status bit0=1, ProductCode hợp lệ, RecordSeq khớp publication TotalCount; nguồn/Session và SKU đã gán lúc nhận phải có. Control có FormatVersion=1, RecordSize=240, BufferCount=2, Ready/Done hợp lệ; không tự tăng counter từ PC.

Theo workflow source hiện tại, TIP đo P1; BUTT đo P1–P4 nếu OK. Mỗi điểm đã đo phải có kết quả PLC, AttemptCount 1–4 và DecideAttempt đúng lượt quyết định. Điểm NG phải đủ 4 lượt, dừng các điểm sau; trạm NG có PointCount=NGPoint. Overall khớp kết quả hai trạm. Những slot không dùng để 0. Workbook có layout bốn slot TIP, còn giới hạn TIP P1 là workflow trong source; cần kỹ thuật xác nhận nếu chương trình máy khác. Không yêu cầu đặt số đo khác 0 để vượt kiểm tra.

Ví dụ metadata cho **một phép đo PLC xác định là OK**: Overall=1, hai Result trạm=1; BUTT PointCount=4, TIP PointCount=1, NGPoint=0; năm điểm nghiệp vụ có Result=1, AttemptCount=1, DecideAttempt=1. Position/Load/PushDist/Bend có thể đều bằng 0 nếu đó là giá trị máy thực sự đo/công bố. Đây là mô tả hợp đồng, không phải lệnh tự gán OK hoặc hướng dẫn ghi thanh ghi từ ứng dụng. PLC phải quyết định quality và hoàn tất trước khi publish.

### Thay đổi tối thiểu

- `plc/as200/decode.rs`: khi payload toàn 0 bị từ chối Version, diagnostic nêu rõ Version/Status/ProductCode/kết quả/số lượt chưa có; không thay đổi điều kiện chấp nhận hoặc phép giải mã số.
- `services/tests.rs`: bổ sung test toàn 0 ở counter 0/1 báo lỗi rõ và không sửa payload; test record hoàn tất có tất cả REAL đã đo bằng 0/âm/dương, cả LOW/HIGH, giữ quality/attempt/NULL semantics.
- `hooks/usePlcMonitor.ts`, `AddMeasurement.tsx`, `plcConnectionService.test.ts`: khắc phục điều kiện Alert trước đó không bao gồm RAW_UNDECODED dù có câu thông báo. Giám sát hiện thông báo đã lưu nhưng thiếu thông tin hoàn tất và không có kết quả mới trong Lịch sử; giữ cảnh báo ACK ưu tiên, thêm giải thích nếu phù hợp. Không có bảng RAW, 240 WORD hoặc Inbox ID trong thông báo này.

`acquisition::process_receipt` hiện hữu ghi lỗi projection vào `plc_inbox.decode_error`, giữ raw nguyên trạng và tránh retry decode mỗi poll. Lỗi toàn 0 mới sẽ được lưu với diagnostic rõ hơn; các row cũ đã có decode_error vẫn giữ nguyên, không tự sửa dữ liệu lịch sử hay tạo lại publication cùng identity. Inbox không có Measurement không xuất hiện trong query lịch sử/thống kê Measurement. UI vẫn hiển thị 0 bằng `formatDecimal` và giữ 0 trong biểu đồ; không cần sửa query báo cáo, schema hay cơ chế lưu.

Không sửa Worker acquisition, store/ACK, Register Mapping, migration hoặc database config. Agent không khởi chạy app, kết nối PLC thật hoặc Neon, không chạy test DB ignored trong lượt này. Không dùng kết quả Neon ở mục 11 để suy nghiệm thu thay đổi hiện tại.

| Lệnh kiểm chứng mục 12 | Kết quả |
|---|---|
| `cargo check --all-targets` | PASS, exit 0, không có lỗi/cảnh báo Rust. Ban đầu chờ Cargo khác giữ build lock, sau đó hoàn tất. |
| `cargo test` | PASS, exit 0: 63 PASS, 0 FAIL, 11 DB ignored. Hai test Rust mới đều PASS. |
| `npm test -- --run` | PASS, exit 0: 24 PASS, 0 FAIL. Bổ sung assertion thông báo đã lưu nhưng chưa hoàn tất, không chứa dữ liệu RAW/mã kỹ thuật. Các test sẵn có kiểm tra 0 không thành NULL, số âm/dương và export/biểu đồ tiếp tục PASS. |
| `npm run build` | PASS, exit 0; vẫn có cảnh báo chunk >500 kB. |

Chưa xác minh runtime UI/SQL cho thay đổi diagnostic này và không cập nhật decode_error đã có trong Neon. Không đưa bảng RAW trở lại UI; phần Measurement đã giải mã, layout/filter/export được giữ nguyên trong lượt này.
