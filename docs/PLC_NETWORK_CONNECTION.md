# Kết nối PLC trên laptop Windows khác nhau

Ngày 18/11/2026. **Trạng thái mới nhất ở mục “Chốt vận hành QC — kết nối thủ công, tự tìm Ethernet” cuối tài liệu. Các mục trước đó là snapshot những lượt triển khai trước.** Giao diện không còn khối lựa chọn mạng. Ứng dụng không tự kết nối PLC khi mở; backend tự tìm Ethernet khi có yêu cầu kết nối, không lưu IP laptop cố định. Không đổi giao thức thu nhận, checkpoint, Session, giải mã hoặc cơ chế ACK.

## Cách cấu hình

1. Trong Kết nối PLC, mở phần kỹ thuật. Ngắt kết nối và đợi Worker dừng trước khi đổi lựa chọn mạng.
2. Mặc định **Automatic**: không lưu/bind IP nguồn. Windows chọn tuyến. Đã gỡ `plc_local_ip` cố định khỏi `as200_config.toml` trong workspace và khỏi ví dụ cấu hình mặc định.
3. Nếu cần chỉ rõ card, chọn **Manual — tên Ethernet (IPv4)** rồi lưu. Danh sách chỉ cho chọn Ethernet đang Up và có IPv4 hợp lệ; nút cập nhật đọc lại danh sách sau khi cắm cáp. IPv4 không được lưu cố định: mỗi socket mới lấy địa chỉ hiện tại do Windows chọn trên card đó.
4. Bấm Kiểm tra kết nối khi có PLC tại nhà máy. Không cần thử khi đang ở nhà và Ethernet Disconnected. Thông báo Ethernet chưa kết nối là tình trạng môi trường, không phải chứng minh ứng dụng hỏng.

Installer hiện không đóng gói `as200_config.toml` đang dùng. Khi chưa có cấu hình/lịch sử, ứng dụng tạo template tắt thu/ACK với Automatic. Không sao chép `plc_adapter` hoặc IP nguồn từ laptop khác. Manual có kiểm tra adapter ID và MachineGuid của laptop đã lưu; nếu không khớp, cần chọn/lưu lại trên máy mới. MachineGuid chỉ dùng cục bộ, không đưa vào DTO Frontend hay log. Cấu hình cũ có `plc_local_ip` vẫn parse được để có thể mở cài đặt, nhưng không được sử dụng cho PLC từ xa: cần chọn lại Automatic/Manual. Ngoại lệ bind loopback chỉ phục vụ endpoint loopback.

Việc chuyển **lịch sử/Session/DB và quyền sở hữu nguồn PLC** giữa laptop vẫn theo các guard hiện hữu; đổi card không giải pending ACK, không tạo Session mới và không cho phép hai laptop cùng vận hành một nguồn.

## Route và Wi-Fi

Trước mỗi socket TCP mới, connector đọc `GetAdaptersAddresses` và `GetBestRoute2` của Windows, không gửi packet tới PLC để kiểm tra route. Automatic từ chối tuyến qua Wi-Fi/card không phải Ethernet; Manual còn yêu cầu interface của tuyến trùng card đã chọn. Nếu hai card cùng subnet và Windows chọn sai card, ứng dụng báo cần kiểm tra subnet/route/metric, không tự sửa Windows. Wi-Fi tới Neon không bị thay đổi.

Automatic không biết cáp PLC thực tế cắm vào Ethernet nào nếu có nhiều Ethernet. Chọn Manual để xác minh card mong muốn. Kiểm tra route là snapshot trước connect, không khóa route Windows và không chứng minh PLC tồn tại hoặc Ladder đúng; route vẫn có thể đổi sau kiểm tra. Bind IP nguồn không được coi là cơ chế ép route.

## File thay đổi

- Thêm `src-tauri/src/plc/network.rs`: inventory Windows, lựa chọn Automatic/Manual, xác minh máy, route và test offline.
- `Cargo.toml`/`Cargo.lock`: dùng Windows IP Helper và Registry read-only qua `windows-sys` đã có trong dependency graph.
- `plc/mod.rs`, `lib.rs`: module và command lấy danh sách adapter.
- `plc/simulator_config.rs`, `plc/connection_settings.rs`: parse/lưu lựa chọn adapter, xóa IP cũ khi người dùng lưu, probe dùng connector chung.
- `plc/as200/mod.rs`, `reader.rs`, `ack.rs`: truyền lựa chọn mạng tới cùng TCP connector. Chỉ đổi phần chuẩn bị kết nối; giữ nguyên FC03, FC16/FC06, timeout, heartbeat và guard ACK. `as200/tests.rs`, test transport inline và `bin/as200-reader.rs` khởi tạo thêm trường cấu hình mạng.
- `plc/connection_settings/tests.rs`: template Automatic và probe/reader dùng cùng cấu hình/kết nối; adapter không hợp lệ không mở TCP.
- `plc/worker/tests.rs` và helper thư mục trong connection settings test: thêm bộ đếm vào tên thư mục tạm để tránh hai test song song trùng timestamp trên Windows. Không sửa Worker production hoặc assertion.
- `src/services/plcConnectionService.ts` và `.test.ts`: DTO/service adapter, test arguments Automatic/Manual và thông báo mạng.
- `src/pages/ConnectionSettings/ConnectionSettings.tsx`: thay ô IP nguồn bằng lựa chọn Automatic/Manual, cập nhật adapter và báo lựa chọn cũ không hợp lệ. Giữ layout/CSS/các trang dữ liệu.
- `as200_config.example.toml`, cấu hình local `as200_config.toml`: bỏ IP laptop cố định, giữ endpoint PLC và các thiết lập khác.

Không sửa installer vì cả NSIS/MSI không copy lựa chọn mạng laptop vào cấu hình mặc định. Không thay Worker acquisition, Reader snapshot/map, Decoder, Heartbeat, SQL migration/schema, PostgreSQL, Measurement hoặc báo cáo. Không động tới local-runtime. File SQL cleanup đã bị xóa trong working tree trước lượt này được giữ nguyên trạng thái, không thuộc thay đổi của lượt này.

## Kiểm chứng

| Lệnh | Kết quả cuối |
|---|---|
| `cargo check --all-targets` | PASS, exit 0, không có lỗi/cảnh báo Rust |
| `cargo test --quiet` (sau các lượt `cargo test`) | PASS, exit 0: **71 PASS, 0 FAIL, 18 ignored** |
| `npm test` | PASS, exit 0: **25 PASS, 0 FAIL**, 3 file |
| `npm run build` | PASS, exit 0; còn cảnh báo chunk >500 kB |

Các ca mới: `automatic_manual_and_reconnect_resolve_current_ip` kiểm Automatic không bind, Manual lấy IP hiện tại, IP cũ không được dùng và cấu hình khác laptop bị chặn. `wifi_disconnected_ethernet_and_same_subnet_do_not_bypass_route` kiểm Wi-Fi, Ethernet ngắt, hai adapter cùng subnet và Manual sai interface. `automatic_and_resolved_manual_binding_use_loopback_sockets_only` kiểm socket Automatic, bind địa chỉ đã chọn và IP không phù hợp trả lỗi không panic. `probe_and_worker_reader_share_automatic_transport_and_manual_config` kiểm probe và reader của Worker dùng cùng lựa chọn; Manual không hợp lệ không mở TCP. `windows_inventory_is_read_only_and_has_unique_adapter_ids` chạy API inventory/MachineGuid và route lookup loopback trên Windows hiện tại. Các test FC03 reconnect/FC16/FC06 loopback hiện hữu tiếp tục PASS. Frontend test mới xác minh arguments lưu Automatic/Manual không chứa IP laptop và giữ thông báo Ethernet chưa kết nối.

Lượt Rust trung gian có **70 PASS / 1 FAIL / 18 ignored** do `TestDirectory::new` trong Worker tests bị `AlreadyExists` khi hai test dùng timestamp trùng; đã sửa tính duy nhất của tên thư mục fixture như manifest, không xóa assertion/ignore test, rồi chạy lại thành kết quả cuối trên.

Các test mạng chỉ dùng inventory/route lookup cục bộ và Modbus/TCP loopback; adapter/route lỗi được kiểm bằng dữ liệu giả, không đổi mạng Windows. 18 test DB giữ ignored (11 integration Neon và 7 migration test). Không chạy Tauri DEV, không kết nối `192.168.1.49:502`, không gửi ACK thật, không kết nối Neon. Chưa xác minh Manual với Ethernet/PLC vật lý hoặc chuyển Wi-Fi/Ethernet thực giữa lúc kết nối; không coi test giả lập là nghiệm thu thiết bị.

## Cập nhật — khắc phục tuyến Wi-Fi/Ethernet

Đây là hành vi hiện tại, thay thế mô tả Automatic chỉ để Windows chọn ở snapshot phía trên.

### Chọn card và địa chỉ nguồn

- **Tự động (Ethernet)**: chỉ xét Ethernet vật lý theo IP Helper (`IfType` và `GetIfEntry2.HardwareInterface`), đang Up, có IPv4 Preferred và prefix phù hợp với IP PLC. Không dựa vào tên card; không chọn Wi-Fi, VPN hoặc Ethernet ảo. Một card phù hợp được chọn tự động; nhiều card phù hợp cần người dùng chọn. Không coi subnet là bằng chứng cáp PLC nối vào card đó.
- Dùng `OnLinkPrefixLength` thật của từng IPv4, không giả định /24. Không chọn địa chỉ PLC trùng IP local, địa chỉ network/broadcast hoặc prefix không hợp lệ. Không có card phù hợp thì hướng dẫn cắm cáp/cấu hình IPv4 Ethernet cùng subnet PLC; ứng dụng không tự đổi IPv4.
- Connector vẫn dùng `GetBestRoute2` không ràng buộc nguồn/interface để kiểm tra lựa chọn Windows trước TCP. Nếu đúng Ethernet đã chọn, bind IPv4 hiện tại phù hợp rồi connect; không lưu IP nguồn cố định. Bind không được dùng để che một route Windows đang sai.
- `Engine::open` chỉ thêm bước ghim GUID/machine của card vào config **trong bộ nhớ**. Reader và ACK cùng dùng lựa chọn này; reconnect có thể lấy IP mới trên cùng card nhưng không tự đổi sang card khác trong Engine đang chạy. Nếu chưa chọn được card lúc mở Engine, cần cắm cáp/chọn card rồi kết nối lại. Dừng hoàn toàn để chọn card khác; không sửa Session/binding/checkpoint hoặc bằng chứng ACK.

### Sửa tuyến có xác nhận

1. Cập nhật card mạng, chọn/lưu cấu hình. Kiểm tra mạng và Test Connection chỉ đọc; không gọi thao tác sửa tuyến, không gửi ACK.
2. Khi route sai nhưng có Ethernet phù hợp và IP PLC là RFC1918, UI đề xuất **Khắc phục đường mạng**. Nếu đã có bất cứ host route `/32` tới IP đó, không ghi đè hoặc tạo thêm. Với nhiều card, phải chọn và lưu một card trước khi có đề xuất.
3. Ngắt kết nối và đợi Worker dừng hoàn toàn. Hộp xác nhận nêu đúng IP PLC, card và IPv4 dự kiến, phạm vi `/32` cho mọi kết nối tới IP đó; không sửa Wi-Fi, gateway, DNS, IPv4 hay metric toàn hệ thống.
4. Backend kiểm tra lại đề xuất dưới `settings_gate`, rồi gọi chính binary hiện tại bằng `ShellExecuteExW/runas`, với request có cấu trúc mã hóa hex (không ghép lệnh shell). Chỉ child xử lý `--plc-host-route` được nâng quyền và thoát trước Tauri setup; UI, Worker và kết nối DB không khởi chạy trong child. UAC bị hủy/không đủ quyền trả thông báo, không giả thành công.
5. Child chỉ đọc `as200_config.toml` đúng thư mục ứng dụng, validate config/map hiện hữu, kiểm lại endpoint/adapter/machine/IP sau UAC, giữ khóa acquisition hiện hữu trước thao tác. Parent giữ settings gate đến khi child thoát, kể cả khi future chờ bị hủy; helper dùng mutex riêng để tuần tự hóa thao tác route giữa các instance. Không chỉnh nội dung capture.json/checkpoint.
6. `CreateIpForwardEntry2` tạo duy nhất route đích IP PLC `/32`, next-hop `0.0.0.0` (on-link), interface LUID/index đã xác minh, metric **của route này** là 5. Không dùng Set/replace, không sửa route mặc định. Đọc lại bảng route và `GetBestRoute2` để xác minh; không mở TCP/Modbus để thử tuyến. Không tự bắt đầu Worker sau sửa.

API tạo route yêu cầu quyền quản trị; nâng quyền nằm ở child cho thao tác cụ thể. Tham khảo [CreateIpForwardEntry2](https://learn.microsoft.com/en-us/windows/win32/api/netioapi/nf-netioapi-createipforwardentry2) và [ShellExecuteEx process handle](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/ns-shellapi-shellexecuteinfow).

Nếu chính sách máy không cho nâng quyền, chuyển IP PLC/card/index hiển thị trong đề xuất cho kỹ thuật. Kỹ thuật cần kiểm tra route `/32` hiện có trước, rồi có thể tạo on-link route trong ActiveStore bằng công cụ quản trị Windows; không thay default gateway/metric Wi-Fi. Route tạo thủ công không được ứng dụng nhận là của Golf Report và phải được kỹ thuật hoàn tác riêng. Ứng dụng vẫn đọc lại route và cho kết nối nếu đúng Ethernet, không bắt buộc phải dùng nút khắc phục của ứng dụng.

### Bằng chứng sở hữu và hoàn tác

- Metadata nằm trong Windows Registry **HKLM 64-bit `SOFTWARE\GolfReport\PlcHostRoutes-v1`**, quyền ghi theo ACL quản trị kế thừa. Đây là metadata Windows cục bộ, không phải bảng PostgreSQL `plc_source_registry`. Không thêm bảng DB. Không ghi credential DB.
- Flush intent trước Create, sau đó lưu fingerprint route đọc lại và cờ committed. Nếu child crash sau intent mà chưa đủ bằng chứng, không nhận một route đang có làm của mình để xóa. Lỗi xác minh sau Create có thể để lại route/intent cần đối chiếu; không rollback mù.
- UI liệt kê bằng chứng của **mọi IP đã quản lý**, kể cả endpoint/adapter cũ. Chọn **Hoàn tác**, xác nhận và cấp quyền; Worker phải dừng. Chỉ xóa khi đúng machine, đúng plan và đúng một host route khớp fingerprint đã commit (LUID, index, next-hop, metric, protocol, lifetimes, flags, origin; Age không so vì tự tăng). Route bị sửa, có thêm route hoặc intent chưa rõ thì từ chối xóa và yêu cầu kỹ thuật. Nếu route đã hết tồn tại, chỉ xóa metadata của nó.
- Khi đổi card với cùng IP PLC, hoàn tác route cũ trước rồi đề xuất lại cho card mới; ứng dụng không tự chuyển route sang card mới. Khi đổi endpoint, route cũ vẫn xuất hiện để người dùng chủ động hoàn tác sau restart theo lifecycle cấu hình hiện hữu.
- Route là **ActiveStore, không persistent qua Windows reboot**. Sau reboot, metadata vẫn còn. Nếu plan/IP/interface vẫn khớp và route không còn, UI có thể đề xuất tạo lại với xác nhận mới; không âm thầm tái tạo khi startup/reconnect. Nếu IP/index/card đã đổi, hoàn tác metadata cũ rồi đề xuất lại. Không cần nhập lệnh route mỗi lần mở app, nhưng sau reboot có thể cần xác nhận lại.

### File sửa trong lượt này

| File | Nội dung |
|---|---|
| `src-tauri/src/plc/network.rs` | Prefix IPv4, nhận diện Ethernet vật lý, chọn theo subnet, ghim adapter và test selection/reconnect |
| `src-tauri/src/plc/network_route.rs` (mới) | Đề xuất read-only, kiểm tra consent/state, quyền sở hữu, create/undo và mock System tests |
| `src-tauri/src/plc/network_route/native.rs` (mới) | IP Helper route table/Create/Delete; HKLM evidence; UAC child/COM/process handle/mutex |
| `src-tauri/src/plc/connection_settings.rs` | Hai command inspect/change; dừng Worker, giữ settings gate trong cả thao tác nâng quyền |
| `src-tauri/src/plc/worker/acquisition.rs` | Chỉ thêm config mutable và gọi pin_worker khi mở Engine; không đổi transaction hoặc ACK predicates |
| `src-tauri/src/plc/mod.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/main.rs` | Module, registration, helper entry trước Tauri setup |
| `src-tauri/Cargo.toml` | Bật Windows API features cho Shell/COM/Registry/mutex, không thêm framework |
| `src/services/plcConnectionService.ts`, `.test.ts` | DTO/service inspect/change và kiểm tra ranh giới đọc/sửa/hoàn tác |
| `src/pages/ConnectionSettings/ConnectionSettings.tsx`, `.css` | Nhãn chọn Ethernet, đề xuất khi có lỗi, hộp xác nhận, hoàn tác route cũ, responsive |
| `docs/PLC_NETWORK_CONNECTION.md` | Báo cáo hiện tại |

Không sửa FC03/FC16/FC06, Reader snapshot/Decoder/Heartbeat, SQL/Measurement, SourceKey/Session, binding và bằng chứng ACK. Không sửa config PLC/DB hoặc Journal runtime trong lượt triển khai. Khi người dùng thực sự xác nhận sửa tuyến, parent có thể tạo thư mục/file lock acquisition còn thiếu bằng quyền thường, không tạo capture.json; helper chỉ mở và giữ lock đó. Những diff từ các lượt trước và file SQL cleanup đã bị xóa trước lượt này không thuộc thay đổi route hiện tại.

### Kiểm chứng hiện tại và giới hạn

- `cargo check --all-targets`: PASS; Rust mặc định: **80 PASS, 0 FAIL, 18 ignored** (11 Neon + 7 migration). Test target chính/CLI không chạy entrypoint ứng dụng.
- `npm test`: **26 PASS**, 3 file. `npm run build`: PASS, còn cảnh báo chunk >500 kB.
- Mock Windows boundary kiểm đúng/sai route khi Wi-Fi cùng bật, nhiều Ethernet cùng subnet, card Down/VPN, prefix sai, IP thay đổi, laptop mới, quyền bị từ chối, intent không lưu được, host route có sẵn, fingerprint đổi/route trùng, khôi phục sau reboot và undo sau đổi endpoint. Test inspector xác nhận không có mutation. Các test Modbus loopback FC03/read-only probe, reconnect, Count/Done transport hiện hữu tiếp tục chạy. Không thay SQL bằng mock để báo test DB PASS.
- **Không gọi Create/Delete route thật, không ghi HKLM ownership thật, không mở UAC, không chạy Tauri DEV, không kết nối `192.168.1.49:502` hoặc Neon.** Inventory/route lookup loopback chỉ đọc. Native route/UAC/registry/lock phối hợp mới được compile và đối chiếu source, chưa nghiệm thu trên Windows test machine với route thật.
- IP Helper không có transaction/CAS bao trùm mọi chương trình quản lý mạng. App kiểm trước/sau và dùng mutex cho child của mình, nhưng admin/VPN khác vẫn có thể sửa route trong khe giữa kiểm tra và OS call; không tuyên bố loại trừ tuyệt đối race đó. Một route bị admin thay bằng route hoàn toàn giống fingerprint cũng không có owner token của Windows để phân biệt. Cần tránh sửa mạng đồng thời khi áp/undo và đối chiếu thủ công khi có dấu hiệu mâu thuẫn.
- Subnet, adapter vật lý và route đúng không chứng minh đúng PLC. Cáp, VLAN/firewall, quyền UAC/ACL triển khai, driver USB Ethernet, định tuyến thực khi Wi-Fi/VPN cùng bật và handshake/timing Ladder vẫn cần nghiệm thu riêng. Không đánh dấu đã khắc phục trên phần cứng thật từ test offline.

## Chốt vận hành QC — kết nối thủ công, tự tìm Ethernet

### Giao diện và thao tác hiện tại

Đã xóa toàn bộ khối **Kết nối mạng PLC** khỏi QC và Thiết lập kỹ thuật: label, Select Automatic/Manual, thông tin adapter, nút cập nhật, cảnh báo vàng, đề xuất/hoàn tác route và modal tương ứng. Đã dọn state, effects, handlers, imports, service/type Frontend riêng của khối và CSS không còn consumer. Vùng nhớ giao tiếp nằm ngay sau IP/Port/Unit ID. Sidebar, tiêu đề, hai chế độ QC/kỹ thuật, các trường kỹ thuật khác, các nút kiểm tra/kết nối/ngắt, chỉ số và nhật ký giữ nguyên.

Startup gọi `WorkerState::initialize` để nạp settings/log, không gọi `launch_under_gate`, không mở Engine/socket, kể cả TOML còn `enabled=true`. Không sửa cờ đó, marker dừng, Session hoặc checkpoint để đạt hành vi này. Database startup vẫn theo cơ chế hiện hữu. Không kiểm tra Ethernet chỉ vì mở trang hay bật Thiết lập kỹ thuật.

- **Kiểm tra kết nối:** dưới settings gate, tự chọn/ghim Ethernet cho lần probe; TCP rồi snapshot FC03/control AS200 phải thành công mới báo kết nối hợp lệ. Không tạo Worker, ghi Count/Done, PostgreSQL, Journal hoặc gọi route helper/UAC. Nếu Worker đang chạy, probe dùng bản sao cấu hình adapter đã ghim của chính Engine dưới diagnostic read gate, không tự chọn card khác. Không đưa diagnostic payload chưa decode vào kết quả kiểm tra mạng.
- **Kết nối:** kiểm tra bằng cùng policy/connector, rồi mới bật acquisition theo đường hiện hữu. Engine ghim adapter khi mở; reconnect trong Engine giữ adapter đó, lấy IPv4 hiện tại, không sửa route. ACK vẫn cần toàn bộ receipt/checkpoint/snapshot/heartbeat proof cũ.
- **Ngắt kết nối:** stop/join/marker hiện hữu giữ nguyên, không tự reconnect. Chỉ lần nhấn Kết nối kế tiếp mới gỡ marker theo đường cũ. Startup mới cũng không tự tiếp tục.

### Chọn Ethernet và cấu hình cũ

Backend vẫn giữ inventory Ethernet vật lý, prefix thực, `GetBestRoute2`, connector chung và route create/undo có xác nhận. Một Ethernet phù hợp được tự chọn; nhiều Ethernet phù hợp trả lỗi yêu cầu kỹ thuật chỉ định trong cấu hình triển khai, không thêm lựa chọn cho QC. Wi-Fi/VPN không được chọn. Route sai chỉ báo lỗi; QC không được tự mở UAC hoặc sửa route từ hai nút kết nối.

`plc_local_ip` vẫn được parse để đọc TOML cũ nhưng không còn dùng để bind trong reader/config acquisition. Ở đầu một probe/Engine mới, nó được bỏ khỏi **bản sao trong bộ nhớ**. `plc_adapter` thuộc machine khác hoặc ID không còn là Ethernet trên máy hiện tại được bỏ khỏi bản sao lựa chọn, rồi áp quy tắc tự chọn duy nhất. Không sửa/xóa TOML. Chỉ định adapter còn hợp lệ trên chính machine hiện tại vẫn là cấu hình triển khai của kỹ thuật (hữu ích khi nhiều Ethernet); nếu card đó Down hoặc sai subnet thì trả lỗi, không failover âm thầm. Một Engine đã chạy không áp fallback này khi mất card: pin tiếp tục nghiêm ngặt đến khi dừng. Probe đang có Engine dùng pin đó. Kiểm tra idle trước đổi SKU cũng dùng cùng chuẩn bị mạng trên bản sao, giữ nguyên mọi điều kiện kiểm tra DB/handshake.

Lưu chu kỳ đọc từ UI chỉ gửi `pollIntervalMs`; không làm mất cấu hình adapter cũ. Backend vẫn giữ command maintenance có `updateAdapter=true` tường minh để kỹ thuật thay lựa chọn và vẫn bắt buộc Worker dừng. Hai command inspect/change route cùng helper nâng quyền và test vẫn được giữ; không có caller/effect sửa tuyến trong UI vận hành. Các quy tắc ownership, xác nhận, không overwrite và hoàn tác của mục route trước vẫn áp dụng cho thao tác kỹ thuật; không biến thành tự động sửa mạng.

### Thông báo và lỗi cũ

Chỉ một vùng thông báo kết nối trong bảng Trạng thái kết nối, không còn cảnh báo mạng trùng bên form. Phân biệt chưa kết nối, đang kiểm tra, đang kết nối, đã kết nối, kết nối thất bại, mất kết nối và đang dừng. Lỗi TCP/Modbus giữ đúng lớp lỗi, không đổi thành Ethernet Disconnected.

| Tình trạng | Thông báo |
|---|---|
| Không có Ethernet Up phù hợp được chỉ định | Chưa phát hiện kết nối Ethernet đến mạng PLC. |
| Ethernet có Up nhưng IPv4/subnet không phù hợp | Địa chỉ IPv4 của Ethernet chưa phù hợp với mạng PLC. |
| Windows chọn sai tuyến hoặc chưa có tuyến | Đường mạng đến PLC chưa đúng. Vui lòng liên hệ kỹ thuật để kiểm tra cấu hình mạng. |
| Nhiều Ethernet phù hợp | Yêu cầu kỹ thuật chỉ định card trong cấu hình triển khai; không kết nối ngẫu nhiên |

Badge kết nối dùng bằng chứng probe/FC03 thực, không dùng subnet làm success và không suy ACK thành công. Lỗi thao tác cũ bị thay bằng lần thử mới hoặc bỏ khỏi feedback khi Worker có lần đọc mới hơn đã xác minh kết nối. Probe thành công chỉ dọn ERROR mạng cũ trong status **volatile** khi không có Worker/task chạy, pause, ACK FAILED/RECONCILIATION hoặc blocked data; không xóa event log, không sửa trạng thái ACK hoặc bằng chứng DB. Cảnh báo dữ liệu/ACK thật được giữ tách biệt với kết nối.

### File thay đổi của lượt chốt QC

- `src/pages/ConnectionSettings/ConnectionSettings.tsx`, `.css`, `.test.tsx`: gỡ khối mạng, gom feedback về status, giữ form/KPI/log, thêm kiểm tra render QC không gọi IPC/kết nối và không có lựa chọn mạng.
- `src/services/plcConnectionService.ts`, `.test.ts`: bỏ wrapper/type mạng không còn Frontend caller; lưu poll riêng; mapping trạng thái thao tác, probe và phục hồi, không sửa ACK.
- `src-tauri/src/lib.rs`, `src-tauri/src/plc/worker.rs`, `worker/tests.rs`: startup metadata-only, clone cấu hình pin cho probe khi Engine tồn tại, dọn lỗi mạng cũ có điều kiện; test startup với enabled=true không có TCP/Journal và không nới proof ACK.
- `src-tauri/src/plc/network.rs`, `simulator_config.rs`: thông báo mạng theo lớp, tương thích IP/adapter cũ trên bản sao, test laptop mới/IPv4/subnet, reader không dùng legacy source IP.
- `src-tauri/src/plc/connection_settings.rs`, `connection_settings/tests.rs`: probe chuẩn bị mạng mới hoặc dùng pin đang chạy, lưu poll không sửa adapter, test FC03-only và cấu hình cũ không bind IP laptop trước.
- `src-tauri/src/plc/worker/acquisition.rs`: chỉ bổ sung chuẩn bị bản sao mạng cho kiểm tra idle trước đổi SKU; mọi predicate, query, transaction và trình tự ACK giữ nguyên.
- `src-tauri/src/plc/network_route.rs`: inspector kỹ thuật dùng cùng policy tương thích cấu hình cũ; không xóa route backend/native/helper hay tests.
- `docs/PLC_NETWORK_CONNECTION.md`: kết quả và hướng dẫn hiện tại.

### Kiểm chứng lượt chốt QC

`cargo check --all-targets`: PASS. Rust mặc định: **84 PASS, 0 FAIL, 18 ignored** (không chạy 11 Neon hoặc 7 migration test). `npm test`: **26 PASS**, 4 file. `npm run build`: PASS, còn cảnh báo bundle >500 kB.

Đã chạy startup enabled=true nhưng không có socket/Engine/task; reader/probe loopback chỉ FC03; constructor pin chung; cấu hình IP/adapter cũ; candidate/subnet/Wi-Fi/VPN; pin không failover; TCP reconnect; stop/join/marker; giữ ACK evidence; FC03/FC16/FC06 loopback; UI idle không gọi IPC, không còn khối mạng; feedback lỗi cũ được thay bằng proof mới. Full Worker reconnect với SQL/PLC thật, render tương tác trên máy QC, đường mạng vật lý, route/UAC, firewall/VLAN và Ladder vẫn cần nghiệm thu riêng.

Lượt chẩn đoán đầu có hai lỗi fixture mới: test startup replace nhầm cả chuỗi `ack_enabled`, và SSR thiếu Router context của PageHeader. Đã sửa fixture bằng thay đúng dòng `enabled` và bọc MemoryRouter, không nới validation/ACK hoặc bỏ assertion; kết quả cuối như trên.

Không chạy Tauri DEV, không kết nối PLC `192.168.1.49:502`, không gửi ACK thật, không mở UAC/sửa route/HKLM, không truy cập Neon hoặc sửa local-runtime production. Không sửa config PLC/DB, schema, migration, decoder, heartbeat, Modbus transport hoặc cơ chế lưu/phục hồi ACK.
