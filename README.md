# Golf Report

Ứng dụng Windows Tauri 2 + Rust + React để kết nối thiết bị đo AS200 qua Modbus TCP và lưu kết quả vào PostgreSQL.

Quy trình kết nối: **Dừng thu dữ liệu → Nhập IP/Port/Unit ID → Lưu và kết nối → Tự động thu**. Backend kiểm tra trạng thái nguồn, journal, inbox và snapshot trước khi bắt đầu thu.
