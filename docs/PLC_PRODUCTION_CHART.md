# Biểu đồ PLC production — cấu hình cố định trong source

Ngày 18/11/2026. Thay thế hướng dẫn form cấu hình PostgreSQL của lượt trước.

## Thông số và cách thay đổi

Kỹ thuật viên chỉnh `src/config/measurementSettings.ts`, build và triển khai lại ứng dụng.

| Trạm | Lực đè (g) | Khoảng cách đẩy (mm) | Giới hạn Bend (mm) | Vị trí điểm (mm) |
|---|---|---|---|---|
| BUTT | 500 | 10 | 0–2 | 150, 300, 450, 600 |
| TIP | 200 | 8 | 0–2 | 80 |

Các field có thể chỉnh: `loadG`, `pushDistMm`, `bendLowerMm`, `bendUpperMm`, `positionsMm` theo trạm. Đây là mặc định từ source DEMO cũ, **cần QC xác nhận**, không phải dữ liệu PLC. TIP 80 mm đã đối chiếu `simulatorDemoSettings.TIP.positions` trong Git HEAD `6f08ab9`, không suy đoán từ PDF hoặc số đo.

Thông số áp dụng cho mọi SKU; không có override từ PostgreSQL hoặc UI. Có thể dùng null nếu QC không quy định một thông số/giới hạn. Giá trị 0 vẫn hiển thị; giữ đủ chữ số thập phân để giới hạn nhỏ không bị làm tròn thành 0.

## Hành vi hiện tại

- Bốn ô đọc trực tiếp file TypeScript. Dữ liệu Measurement/Bend vẫn đọc PostgreSQL như trước; giá trị đo không được dùng làm setpoint.
- Bỏ nút Cấu hình thông số đo, form, trạng thái tải/lưu/error của cấu hình, service IPC, model/repository và hai command cấu hình cùng registration.
- Giữ 30 gậy theo SKU, BUTT P1–P4 và TIP P1; không điều kiện DEMO-AS200, Simulator, IP hoặc SourceKey.
- Giới hạn đỏ nét đứt, vòng đỏ chỉ khi Bend vượt giới hạn. Bằng giới hạn không đánh dấu vượt. Trục Y bao gồm giới hạn và số đo. NULL tạo khoảng ngắt, không biến thành 0.
- Tooltip phân biệt vị trí cài đặt, Bend thực tế và giới hạn hiện tại. Đối chiếu trực quan không phải snapshot thông số tại thời điểm đo; không thay Overall/BUTT/TIP, kết quả điểm hoặc KPI của PLC.
- Không sửa Modbus, Worker, ACK, mapping, decoder, RAW, Inbox, config PLC/DB hoặc local-runtime trong lượt này. Các diff decoder/projection có sẵn thuộc lượt trước.

## Xử lý checksum migration đã áp dụng

Đã chạy `as200-db --migration-checksums` chỉ đọc: database `golf_report`, schema `public`, `transaction_read_only=on`. Chỉ SELECT identity kết nối và version/success/checksum SQLx; không chạy migrator, không sửa tracking, không đọc dữ liệu đo hoặc row cấu hình, không in credential/URL.

Cả 8 version đã áp dụng đều success=true; 7 version khác khớp source. Riêng `20261118160000` mismatch vì CHECK load/push/position trong workspace đã đổi từ `>=0` sang cho phép số hữu hạn có dấu sau bản đã áp dụng. So sánh các bản SQL xác định bản gốc UTF-8, LF, không BOM với SHA-384:

```text
1508c54f177eab759fabd612d71ba5d7695e4755322c6b8955585cfefe4cef42fb77dabc530437c4a7ef5fad4677725a
```

Đã khôi phục `src-tauri/migrations/20261118160000_measurement_settings.sql` đúng checksum trên. Giữ version và toàn bộ SQL gốc đã xác minh; không xóa migration hoặc chỉnh `_sqlx_migrations`. Thêm test pin checksum nhúng và `.gitattributes` giữ LF khi Git checkout file này.

**Không tạo DROP migration.** Bảng `measurement_settings` và mọi row hiện có được giữ nguyên. Chưa kiểm kê row/consumer ngoài repo, nên không kết luận bảng trống hoặc dữ liệu được phép xóa. Guard startup vẫn yêu cầu bảng tồn tại khi history đã có version này; đó là tương thích lịch sử, không phải API cấu hình. Cài mới vẫn chạy migration lịch sử để giữ SQLx nhất quán, nhưng ứng dụng không seed hoặc sử dụng bảng để lưu thông số biểu đồ.

Nếu muốn retire bảng về sau: kiểm kê dữ liệu/consumer, sao lưu và xác nhận được phép loại bỏ; tạo migration forward version lớn hơn, dừng nếu có row/phụ thuộc, không CASCADE; cập nhật startup chỉ chấp nhận thiếu bảng sau retirement hợp lệ. Không xóa bảng hoặc sửa history thủ công để vượt checksum.

## File của lượt này

- Thêm `src/config/measurementSettings.ts`, `src/utils/bendChart.ts`, `bendChart.test.ts`, `src/pages/AddMeasurement/BendComparison.test.tsx`.
- Sửa `MonitorPanels.tsx`, `AddMeasurement.tsx`, `AddMeasurement.css` để dùng thông số cố định, giữ đường giới hạn/tooltip/vòng đỏ.
- Xóa `MeasurementSettingsEditor.tsx` cùng test, `measurementSettingsService.ts` cùng test, `models/measurement_settings.rs`, `repositories/measurement_settings_repository.rs`.
- Bỏ command/registration/module tại `commands/measurement_command.rs`, `lib.rs`, `models/mod.rs`, `repositories/mod.rs`; các file này trở về nội dung trước khi thêm quản lý cấu hình.
- `database/migration_tests.rs`: bỏ test CRUD API đã xóa, giữ test schema/bảo toàn row lịch sử (DB opt-in riêng), thêm pin checksum đã xác minh.
- `bin/as200-db.rs`: thêm checksum audit read-only, cấm dùng cùng `--migrate`; không khởi động Tauri/Worker.
- Khôi phục migration 160000 đúng bản đã áp dụng; thêm `.gitattributes`; cập nhật tài liệu này.

## Kiểm chứng

| Lệnh | Kết quả cuối |
|---|---|
| `cargo check` | PASS, exit 0 |
| `cargo test --quiet` | 82 PASS, 0 FAIL, 23 ignored |
| `npm test` | 38 PASS, 0 FAIL, 7 file |
| `npm run build` | PASS; còn cảnh báo bundle >500 kB |

Test mới xác nhận source defaults của BUTT/TIP, vị trí TIP 80 mm, SKU production/DEMO/không chọn SKU đều có thông số, không form hoặc IPC cấu hình, số 0 và giới hạn thập phân nhỏ, vòng đỏ ngoài biên, NULL/bằng biên, miền Y chứa giới hạn. Test Rust pin đúng checksum migration nhúng PASS. Các test loopback và projection hiện hữu tiếp tục PASS.

23 bài SQL integration vẫn ignored theo mặc định, chưa có database test cục bộ được cấu hình. Test bảo toàn bảng/row cấu hình cũ đã biên dịch nhưng NOT RUN; không dùng Neon production để chạy test ghi. Checksum audit Neon là truy vấn read-only riêng, không phải integration test. Không chạy Tauri DEV, không kết nối/ghi PLC thật, không chạy migration hoặc ghi dữ liệu Neon trong lượt này. Chưa nghiệm thu tương tác browser hoặc phần cứng thật.
