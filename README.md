# Golf Report

Ứng dụng Windows Tauri 2 + Rust + React để kết nối thiết bị đo AS200 qua Modbus TCP và lưu kết quả vào PostgreSQL.

Quy trình QC: nhập **IP/Port/Unit ID → Kiểm tra kết nối → Kết nối**. Mỗi bộ ba IP + Port + Unit ID là một nguồn logic riêng. Backend tự tìm hoặc tạo SourceKey, Session ban đầu và Journal v4 riêng sau khi probe và baseline đạt điều kiện an toàn. Không cần Serial hoặc đăng ký hồ sơ thủ công. **Ngắt kết nối** dừng và lưu dữ liệu an toàn.

### Cấu hình PLC trên giao diện

Trang **4 · Kết nối PLC** có hai chế độ cùng trang: QC có Kiểm tra kết nối và Kết nối/Ngắt kết nối; **Thiết lập kỹ thuật** có thêm các thông số nâng cao và Lưu cài đặt. Không có popup chuyển nguồn, serial hoặc checkbox đăng ký. Kỹ thuật dùng **Kiểm tra kết nối → Lưu cài đặt → Kết nối**. Cấu hình chờ lưu tại `as200_config.connection-draft` cạnh file cấu hình, có revision gắn với cấu hình gốc, không chứa SourceKey, Session, Journal hoặc checkpoint. Worker không đọc file chờ; lưu không thay đổi nguồn đang thu. Thay đổi cấu hình gốc làm bản chờ cũ hết hiệu lực.

Backend khôi phục đúng SourceKey/Session/Journal nếu endpoint đã có hồ sơ. Endpoint mới dùng SourceKey ổn định `endpoint-v1-...` theo địa chỉ chuẩn hóa, Port, Unit ID; Session ban đầu cố định theo SourceKey, không sinh Session khác để vượt lỗi. Journal và target-config lưu tại `local-runtime/managed-sources/<source-key>/`. Nguồn mới luôn `read_only=true`, `ack_enabled=false`, không ghi heartbeat; không thêm phê duyệt danh tính vật lý giả vào registry. ACK chỉ được bật sau phê duyệt riêng trong registry và kiểm tra phạm vi giao thức hiện có.

Profile hiện hỗ trợ **AS200 Published v1, 2 buffer × 240 WORD**, giữ BUTT/TIP P1–P4 và PC chỉ đọc Bản ghi 1. Control Start, Modbus offset, WordOrder, chu kỳ đọc, timeout đọc/kết nối, heartbeat, retry và tự kết nối lại được truyền đến runtime. Backend từ chối BufferCount/RecordSize khác hoặc layout chưa biết trước khi đọc; chưa hỗ trợ 256/300 WORD, bảng SKU hoặc tự khởi động Windows.

Dịch vùng nhớ tương thích (ví dụ D26000), HIGH hoặc offset khác phạm vi ACK được duyệt chỉ cho phép kiểm tra/chỉ đọc khi chưa có lịch sử nguồn. Không sửa layout trên hồ sơ đã có SourceKey/Journal để vượt bằng chứng cũ; hệ thống chặn và yêu cầu đối chiếu. Không tự cấp ACK cho vùng nhớ mới. Bỏ chế độ chỉ đọc chỉ cấp lại ACK sau khi luồng kết nối kiểm tra phạm vi AS200 đã duyệt và đầy đủ lịch sử/snapshot. Journal có ACK intent chưa rõ kết quả chặn chuyển sang chỉ đọc.

Chế độ chỉ đọc vẫn lưu kết quả theo Journal v4 → Inbox → PostgreSQL, không phát ACK; PLC có thể giữ bản ghi hiện tại cho tới khi có handshake hợp lệ. Tắt tự kết nối lại giữ Worker ở trạng thái mất kết nối, chờ yêu cầu kết nối lại thủ công. Nhật ký kết nối chỉ gồm sự kiện backend thực tế trong phiên ứng dụng, tối đa 60 sự kiện, không lưu vào PostgreSQL.

Kiểm tra tại nhà máy: nhập endpoint → Kiểm tra kết nối → Kết nối; kỹ thuật có thể Lưu cài đặt trước. Lần đầu chỉ khởi tạo nguồn khi TotalCount/PC_ReadCount/PC_ReadDone/DataReady đều 0, raw Bản ghi 1 rỗng và snapshot ổn định. Nếu counter/buffer chưa sạch, hiển thị lý do và giữ chế độ kiểm tra; không reset PLC hoặc nhập counter làm checkpoint. Đã có lịch sử PostgreSQL/registry nhưng mất hồ sơ hoặc Journal thì dừng đối chiếu, không tạo nguồn thay thế. Chế độ ACK tắt vẫn lưu bản ghi, nhưng PLC/Simulator có thể giữ đầu hàng đợi tới khi có handshake được cấp quyền.

Chuyển nguồn không dùng IP mới làm alias của nguồn cũ: mỗi endpoint có lịch sử riêng. Nguồn ACK-enabled phải đóng đủ checkpoint thực trước khi rời nguồn. Nguồn chỉ đọc không cần giả ACK để chuyển: mọi bản ghi quan sát phải có receipt PROCESSED, measurement khớp, prefix liên tiếp đầy đủ trong PostgreSQL, không còn retry/intent/cảnh báo. Giữ nguyên accepted/receipt/Journal; thiếu bất kỳ bằng chứng nào thì chặn chuyển. RESET vẫn cần đối chiếu riêng; không tự mở Session mới.