# Golf Report

Ứng dụng Windows Tauri 2 + Rust + React để kết nối thiết bị đo AS200 qua Modbus TCP và lưu kết quả vào PostgreSQL.

Quy trình kết nối: **Dừng thu dữ liệu → Nhập IP/Port/Unit ID → Lưu và kết nối → Tự động thu**. Backend kiểm tra trạng thái nguồn, journal, inbox và snapshot trước khi bắt đầu thu.

Xem [chuyển nguồn và giới hạn phục hồi](docs/PLC-SWITCHING.md). Ứng dụng không cần Python; Simulator là công cụ kiểm thử độc lập bên ngoài. Khi quay lại endpoint cũ, ứng dụng chỉ tiếp tục phiên đã lưu nếu xác minh được tính liên tục. Counter giảm, dữ liệu chờ hoặc ACK chưa rõ kết quả vẫn bị chặn; không xóa journal hoặc đổi session để vượt lỗi phục hồi.
