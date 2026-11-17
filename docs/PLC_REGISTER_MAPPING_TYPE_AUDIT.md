# Đối chiếu kiểu dữ liệu PLC với Register Mapping

Ngày **18/11/2026, giờ Việt Nam**. Rà trực tiếp workbook/source và chạy test offline; không sửa Rust, UI, test, mapping/config hoặc dữ liệu. Không chạy Tauri, migration, PLC thật hay Neon.

## 1. Workbook và kết luận

Không tìm thấy `X212PI-Register-Mapping-PLC-Software(20261117-225906).xlsx`, kể cả tìm hidden/ignored ngoài dependency/build output. Bản duy nhất trong workspace là [X212PI-Register-Mapping-PLC-Software.xlsx](X212PI-Register-Mapping-PLC-Software.xlsx). Đã đọc ba sheet Tong_quan, Vung_dieu_khien, Cau_truc_ban_ghi bằng ZIP/XML và cached value; không chạy Excel để tính lại công thức.

SHA-256: `ba9577a90b724cde8a348d31b468572bce50b1f2de871e900e3ab365a102cf07`.

**Không phát hiện lệch địa chỉ, offset, độ rộng 32-bit hoặc Reserve làm lấn trường kế tiếp trong bản mapping này.** Kiểm kê 12 trường control bao phủ 20 WORD và 124 trường record mô tả 234 WORD; không overlap. Chỗ thiếu duy nhất là record +14..19. Code giữ đủ 240 WORD, không ép layout thành 234.

Có khác biệt biểu diễn cần ghi rõ: Excel khai báo Reserve là INT/INT array, còn Rust giữ bit pattern bằng u16/u16 array hoặc Vec<u16>. Đây không phải sai độ dài; nếu một WORD reserve là FFFF thì Excel INT có thể hiểu -1, Rust raw là 65535. Các trường này chưa tham gia công thức nghiệp vụ nên không làm lệch số đo. Chưa thể xác nhận nội dung bản workbook có hậu tố thời gian bị thiếu, thứ tự WORD thực tế hoặc Ladder publish bằng audit source này.

## 2. Source đã đối chiếu

- [map.rs](../src-tauri/src/plc/as200/map.rs): base, size, các offset control/header/trạm, stride điểm.
- [decode.rs](../src-tauri/src/plc/as200/decode.rs): uint16/int16/bits32/dint32/real32/product_code/control/point/station/record.
- [reader.rs](../src-tauri/src/plc/as200/reader.rs): FC03, nối block WORD, stable snapshot và vùng published.
- [model.rs](../src-tauri/src/plc/as200/model.rs): signed/unsigned/f32, raw reserve và validation metadata/workflow.
- [inbox_service.rs](../src-tauri/src/services/inbox_service.rs), [measurement_service.rs](../src-tauri/src/services/measurement_service.rs): NUMERIC, nullable point, attempt/result/link.
- [as200Report.ts](../src/utils/as200Report.ts), [MonitorPanels.tsx](../src/pages/AddMeasurement/MonitorPanels.tsx): 0/NULL/SKIPPED và decision Bend. Lịch sử dùng cùng formatDecimal, không tự đổi 0 thành NULL.

Đối chiếu địa chỉ tại base Excel control 25000, record 25020/25260, Modbus offset 0. Reader có thể nhận base/offset từ config; địa chỉ PDU thực tế = D + modbus_address_offset. Không coi phép cộng PDU của source là bằng chứng endpoint PLC đã được nghiệm thu.

## 3. Control: toàn bộ 20 WORD

| Trường | Offset | Địa chỉ D | Excel / WORD chiếm dụng | Rust |
|---|---:|---|---|---|
| FormatVersion | 0 | D25000 | INT / 1 | i16, yêu cầu 1 |
| RecordSize | 1 | D25001 | INT / 1 | i16, yêu cầu 240 |
| BufferCount | 2 | D25002 | INT / 1 | i16, yêu cầu 2 |
| WriteIdx | 3 | D25003 | INT / 1 | i16, 0 hoặc 1 |
| TotalCount | 4–5 | D25004–05 | DINT / 2 | i32, ghép đúng 2 WORD |
| LastIdx | 6 | D25006 | INT / 1 | i16, 0 hoặc 1 |
| Heartbeat | 7 | D25007 | INT / 1 | i16; decoder từ chối âm, monitor xử lý wrap |
| PC_ReadCount | 8–9 | D25008–09 | DINT / 2 | i32, không đọc lấn PC_Heartbeat |
| PC_Heartbeat | 10 | D25010 | INT / 1 | i16, đọc nhưng acquisition không ghi |
| DataReady | 11 | D25011 | BOOL lưu INT 0/1 / 1 | i16, kiểm 0/1; không đọc FC01 coil |
| PC_ReadDone | 12 | D25012 | BOOL lưu INT 0/1 / 1 | i16, kiểm 0/1 |
| Reserve | 13–19 | D25013–19 | INT ×7 / 7 | [u16;7], giữ raw, không diễn giải dấu |

## 4. Record và Reserve

Header: Version +0, Status +1, StickNo +2..3, ASCII ProductCode +4..13; **unknown +14..19**; OverallResult +20, Reserve_Header +21, RecordSeq +22..23. Trạm BUTT +24..31; điểm BUTT +32..119. Trạm TIP +120..127; điểm TIP +128..215. Reserve_End +216..239.

Mỗi header trạm dài 8 WORD: Result +0, PointCount +1, NGPoint +2, Reserve1 +3, Load REAL +4..5, Reserve2 +6..7. Mỗi điểm dài **22 WORD**:

| Trường trong điểm | Offset tương đối | WORD |
|---|---|---:|
| Result / AttemptCount / DecideAttempt | +0 / +1 / +2 | 1 mỗi trường |
| Reserve | +3 | 1 |
| Position REAL | +4..5 | 2 |
| A1 PushDist / Bend REAL | +6..7 / +8..9 | 2 mỗi trường |
| A2 PushDist / Bend REAL | +10..11 / +12..13 | 2 mỗi trường |
| A3 PushDist / Bend REAL | +14..15 / +16..17 | 2 mỗi trường |
| A4 PushDist / Bend REAL | +18..19 / +20..21 | 2 mỗi trường |

Bases BUTT P1–4: **32, 54, 76, 98**. Bases TIP P1–4: **128, 150, 172, 194**. A4 Bend của TIP P4 dùng +214..215, kết thúc ngay trước Reserve_End +216; không tràn record.

### TIP P1: hai trường cần chú ý

`Cau_truc_ban_ghi` hàng 81–82 khai báo:

| Trường | Excel | Buffer 0 | Buffer 1 | Code |
|---|---|---|---|---|
| TIP_P1_Reserve | INT, 1 WORD, +131 | D25151 | D25391 | point(base=128): uint16(base+3), chỉ raw[131] |
| TIP_P1_Position | REAL, 2 WORD, +132..133 | D25152–53 | D25392–93 | real32(base+4), chỉ raw[132] và raw[133] |
| TIP_P1_A1_PushDist | REAL, 2 WORD, +134..135 | D25154–55 | D25394–95 | real32(base+6), bắt đầu sau Position |

**Reserve không bị đọc như REAL hai WORD; Position không bắt đầu tại 131.** Khác biệt signedness Reserve đã nêu ở mục 1 không ảnh hưởng ranh giới này.

### Khoảng trống +14..19 và tổng 234

Đếm đúng Số TG các hàng Excel được 234, các offset không chồng lấn nhưng thiếu 14,15,16,17,18,19. Đây là sáu WORD D25034–39 ở buffer 0 và D25274–79 ở buffer 1. `map::UNKNOWN_HEADER=14`, `OVERALL_RESULT=20`; decoder lấy `words[14..20]` thành `[u16;6]`. Không tự gán tên/ý nghĩa chưa có trong Excel và không kéo OverallResult về +14.

Ô E133=234, F133 báo khác RecordSize. Đây là thiếu mô tả của tài liệu, không phải bằng chứng RecordSize phải đổi thành 234. 234 + 6 unknown = 240; raw/unknown đều giữ lại, Reserve_End vẫn đủ 24 WORD. Cần chủ sở hữu mapping xác nhận tên sáu WORD này và chỉnh tài liệu nếu cần; audit này không sửa workbook/source.

## 5. LOW/HIGH, IEEE 754 và signedness

`bits32` truy cập đúng offset k và k+1, kiểm biên trước khi ghép. LOW: bits=(WORD[k+1]<<16)|WORD[k]; HIGH: bits=(WORD[k]<<16)|WORD[k+1]. `dint32` cast u32 sang i32 giữ two's complement; `real32` dùng f32::from_bits, **không cast WORD sang float theo giá trị và không chia 100**.

| Giá trị | Bit pattern | LOW: WORD đầu, WORD sau | HIGH: WORD đầu, WORD sau |
|---|---|---|---|
| REAL 0.0 | 00000000 | 0000,0000 | 0000,0000 |
| REAL 1.25 | 3FA00000 | 0000,3FA0 | 3FA0,0000 |
| REAL -2.5 | C0200000 | 0000,C020 | C020,0000 |
| DINT -2 | FFFFFFFE | FFFE,FFFF | FFFF,FFFE |

Các mã hex trên là ví dụ unit test/bit representation, không phải lệnh ghi PLC. Modbus trả u16; byte order ASCII là quy tắc khác: ProductCode dùng **byte thấp trước trong từng WORD** bằng to_le_bytes, 10 WORD/20 byte, NUL padding. Không đảo byte ASCII khi đổi LOW/HIGH numeric.

INT nghiệp vụ được int16/i16, Status được uint16/u16. Reserve INT được giữ như bit raw u16; chưa có phép tính signed trên Reserve. NaN/Infinity từ REAL bị từ chối tạo dữ liệu decoded, nhưng snapshot raw vẫn được giữ ở pipeline reception. Không dùng NaN làm NULL hoặc bỏ raw. Hai-word REAL/DINT không lấn field kế tiếp; reader nối các block u16 xong mới decode, không decode từng FC03 block như một record riêng.

## 6. Buffer thực tế và giới hạn xác minh

Excel mô tả hai buffer vật lý: **index 0 D25020–D25259**, **index 1 D25260–D25499**. Cùng layout 240 WORD, mọi địa chỉ buffer 1 bằng buffer 0 +240; phụ lục đã kiểm cả hai cột D và Modbus.

Production `read_published_words` đọc **control_start+20**, với map chuẩn là buffer index 0. `poll_published_record` trả buffer_index=0, không dùng LastIdx/WriteIdx để chọn buffer index 1. `record_base(index)` hai slot chỉ có cfg(test); decoder có thể giải mã mảng của slot 1 với buffer_index=1 nhưng đó không phải reader production đang đọc slot 1.

Ready=0: đọc lại control ổn định, không đọc RAM record cũ. Ready=1: hai lần đọc 240 WORD, control trước/giữa/sau. FC03 giới hạn 100 WORD, mỗi lần 240 gồm 100+100+40; snapshot ready tổng 9 FC03, idle 2 FC03. Đây là logic quan sát ổn định, không tự chứng minh Ladder quản lý buffer đúng.

Comment source nói PLC tự publish bản ghi cần ACK vào slot 0; **Excel buffer vòng không tự chứng minh hợp đồng đó**. Cần Ladder/trace hoặc xác nhận kỹ thuật cho slot công bố, LOW/HIGH trên thiết bị và Modbus offset thực. Không đề xuất tự đổi reader sang LastIdx trong lượt kiểm tra kiểu dữ liệu.

## 7. Số 0, âm, dương, NULL và SKIPPED

REAL hữu hạn 0.0/âm/dương được chấp nhận cho Position/PushDist/Bend/Load, chuyển NUMERIC giữ giá trị thực. Workflow không tự đánh quality từ Bend. Zero đã đo vẫn Some(0), formatDecimal hiển thị 0 và chart không lọc bằng truthiness. Không coi IEEE negative-zero là một trạng thái đo khác: raw giữ bit, projection numeric không bảo đảm giữ dấu -0.

Result=0 là UNMEASURED, AttemptCount=0 là chưa có lượt, DecideAttempt=0 thành None/NULL. Projection chỉ lưu attempt đã thực hiện; điểm chưa đo có position NULL, không tạo các attempt 0 giả. Điểm phía sau trạm NG được UI trình bày SKIPPED từ metadata, không có giá trị float SKIPPED trong PLC hoặc SQL enum SKIPPED.

Decoder vẫn đọc bốn slot mỗi trạm; workflow production hiện chỉ cho TIP P1, BUTT P1–4 hoặc dừng NG đúng quy trình. Đó là luật projection ngoài layout bốn điểm TIP trong Excel. Nếu PLC sử dụng TIP P2–P4, raw vẫn giữ nhưng projection hiện từ chối; không phải lỗi kiểu REAL/offset.

Toàn 240 WORD bằng 0 có Version/Status/kết quả/số lượt đều chưa hợp lệ để xác định Measurement hoàn tất. Không tự gán OK/NG hoặc số lần đo; điều này khác số đo 0 trong record có metadata hoàn tất. Thay một WORD D25200 bằng 12 cũng không tạo Bend=12: đây là WORD đầu TIP P3 A1 Bend (+180), cần ghép IEEE 754 với WORD +181 theo word order.

## 8. Test/check đã thực thi

| Lệnh | Kết quả |
|---|---|
| cargo check --all-targets (src-tauri) | PASS, exit 0, không diagnostic lỗi/cảnh báo Rust |
| cargo test --lib plc::as200:: | 30 PASS, 0 FAIL, không ignored trong nhóm |
| cargo test --lib services::tests:: | 10 PASS, 0 FAIL, 1 test PostgreSQL ignored theo mặc định |

Tổng **40 unit/transport-loopback test PASS**, không chạy test DB. Các ca chính: scalar_decoding_both_word_orders_and_signed_boundaries; offsets_cover_both_slots_and_all_points; full_record_preserves_all_fields_without_legacy_conversion; reader published region/stable snapshot; completed_measurement_preserves_zero_negative_and_positive_reals_in_both_word_orders; unmeasured_slots_are_null_and_measured_zero_and_negative_values_remain_real_values; all_zero_payload_reports_missing_completion_metadata_without_mutation.

Không bổ sung hoặc sửa test trong lượt audit. Test full-record kiểm Position tất cả điểm và unknown header/tail, nhưng **chưa có test sentinel riêng đặt Reserve[131] khác 0 để cô lập nó khỏi Position[132..133]**. Đối chiếu direct source và bảng offset xác nhận ranh giới; không báo đã chạy một test sentinel chưa tồn tại. Đề xuất nếu cần tăng regression: thêm ca Reserve=FFFF, Position=1.25 ở cả LOW/HIGH và kiểm trường kế tiếp; không cần đổi production decoder.

## Phụ lục: toàn bộ 124 trường record theo Excel

Mọi hàng bên dưới đã khớp offset/độ rộng và cả địa chỉ D/PDU của hai buffer, không overlap. Cột Rust mô tả loại đọc/storage; ghi chú Reserve signedness không phải kết luận type INT và u16 đồng nghĩa. Khoảng +14..19 không có hàng trong Excel và được giữ riêng như mục 4.

| Ô Excel | Trường Excel | Kiểu | Số WORD | Offset | D buffer 0 | D buffer 1 | Code Rust | Đúng/Sai | Nguyên nhân |
|---:|---|---|---:|---|---|---|---|---|---|
| 5 | Version | INT | 1 | 0 | D25020 | D25260 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 6 | Status | WORD | 1 | 1 | D25021 | D25261 | u16 / WORD bits | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 7 | StickNo | DINT | 2 | 2-3 | D25022-D25023 | D25262-D25263 | i32 / dint32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 8 | ProductCode | WORD[10] | 10 | 4-13 | D25024-D25033 | D25264-D25273 | u16[10] -> ASCII, low byte first | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 9 | OverallResult | INT | 1 | 20 | D25040 | D25280 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 10 | Reserve_Header | INT | 1 | 21 | D25041 | D25281 | u16 raw reserve (Excel INT signed) | Đúng offset/width; khác dấu | INT signed trong Excel, Rust giữ nguyên bit u16; không tính nghiệp vụ |
| 11 | RecordSeq | DINT | 2 | 22-23 | D25042-D25043 | D25282-D25283 | i32 / dint32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 13 | BUTT_Result | INT | 1 | 24 | D25044 | D25284 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 14 | BUTT_PointCount | INT | 1 | 25 | D25045 | D25285 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 15 | BUTT_NGPoint | INT | 1 | 26 | D25046 | D25286 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 16 | BUTT_Reserve1 | INT | 1 | 27 | D25047 | D25287 | u16 raw reserve (Excel INT signed) | Đúng offset/width; khác dấu | INT signed trong Excel, Rust giữ nguyên bit u16; không tính nghiệp vụ |
| 17 | BUTT_Load | REAL | 2 | 28-29 | D25048-D25049 | D25288-D25289 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 18 | BUTT_Reserve2 | INT[2] | 2 | 30-31 | D25050-D25051 | D25290-D25291 | u16 raw reserve (Excel INT signed) | Đúng offset/width; khác dấu | INT signed trong Excel, Rust giữ nguyên bit u16; không tính nghiệp vụ |
| 19 | BUTT_P1_Result | INT | 1 | 32 | D25052 | D25292 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 20 | BUTT_P1_AttemptCount | INT | 1 | 33 | D25053 | D25293 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 21 | BUTT_P1_DecideAttempt | INT | 1 | 34 | D25054 | D25294 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 22 | BUTT_P1_Reserve | INT | 1 | 35 | D25055 | D25295 | u16 raw reserve (Excel INT signed) | Đúng offset/width; khác dấu | INT signed trong Excel, Rust giữ nguyên bit u16; không tính nghiệp vụ |
| 23 | BUTT_P1_Position | REAL | 2 | 36-37 | D25056-D25057 | D25296-D25297 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 24 | BUTT_P1_A1_PushDist | REAL | 2 | 38-39 | D25058-D25059 | D25298-D25299 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 25 | BUTT_P1_A1_Bend | REAL | 2 | 40-41 | D25060-D25061 | D25300-D25301 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 26 | BUTT_P1_A2_PushDist | REAL | 2 | 42-43 | D25062-D25063 | D25302-D25303 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 27 | BUTT_P1_A2_Bend | REAL | 2 | 44-45 | D25064-D25065 | D25304-D25305 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 28 | BUTT_P1_A3_PushDist | REAL | 2 | 46-47 | D25066-D25067 | D25306-D25307 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 29 | BUTT_P1_A3_Bend | REAL | 2 | 48-49 | D25068-D25069 | D25308-D25309 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 30 | BUTT_P1_A4_PushDist | REAL | 2 | 50-51 | D25070-D25071 | D25310-D25311 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 31 | BUTT_P1_A4_Bend | REAL | 2 | 52-53 | D25072-D25073 | D25312-D25313 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 32 | BUTT_P2_Result | INT | 1 | 54 | D25074 | D25314 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 33 | BUTT_P2_AttemptCount | INT | 1 | 55 | D25075 | D25315 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 34 | BUTT_P2_DecideAttempt | INT | 1 | 56 | D25076 | D25316 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 35 | BUTT_P2_Reserve | INT | 1 | 57 | D25077 | D25317 | u16 raw reserve (Excel INT signed) | Đúng offset/width; khác dấu | INT signed trong Excel, Rust giữ nguyên bit u16; không tính nghiệp vụ |
| 36 | BUTT_P2_Position | REAL | 2 | 58-59 | D25078-D25079 | D25318-D25319 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 37 | BUTT_P2_A1_PushDist | REAL | 2 | 60-61 | D25080-D25081 | D25320-D25321 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 38 | BUTT_P2_A1_Bend | REAL | 2 | 62-63 | D25082-D25083 | D25322-D25323 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 39 | BUTT_P2_A2_PushDist | REAL | 2 | 64-65 | D25084-D25085 | D25324-D25325 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 40 | BUTT_P2_A2_Bend | REAL | 2 | 66-67 | D25086-D25087 | D25326-D25327 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 41 | BUTT_P2_A3_PushDist | REAL | 2 | 68-69 | D25088-D25089 | D25328-D25329 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 42 | BUTT_P2_A3_Bend | REAL | 2 | 70-71 | D25090-D25091 | D25330-D25331 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 43 | BUTT_P2_A4_PushDist | REAL | 2 | 72-73 | D25092-D25093 | D25332-D25333 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 44 | BUTT_P2_A4_Bend | REAL | 2 | 74-75 | D25094-D25095 | D25334-D25335 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 45 | BUTT_P3_Result | INT | 1 | 76 | D25096 | D25336 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 46 | BUTT_P3_AttemptCount | INT | 1 | 77 | D25097 | D25337 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 47 | BUTT_P3_DecideAttempt | INT | 1 | 78 | D25098 | D25338 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 48 | BUTT_P3_Reserve | INT | 1 | 79 | D25099 | D25339 | u16 raw reserve (Excel INT signed) | Đúng offset/width; khác dấu | INT signed trong Excel, Rust giữ nguyên bit u16; không tính nghiệp vụ |
| 49 | BUTT_P3_Position | REAL | 2 | 80-81 | D25100-D25101 | D25340-D25341 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 50 | BUTT_P3_A1_PushDist | REAL | 2 | 82-83 | D25102-D25103 | D25342-D25343 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 51 | BUTT_P3_A1_Bend | REAL | 2 | 84-85 | D25104-D25105 | D25344-D25345 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 52 | BUTT_P3_A2_PushDist | REAL | 2 | 86-87 | D25106-D25107 | D25346-D25347 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 53 | BUTT_P3_A2_Bend | REAL | 2 | 88-89 | D25108-D25109 | D25348-D25349 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 54 | BUTT_P3_A3_PushDist | REAL | 2 | 90-91 | D25110-D25111 | D25350-D25351 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 55 | BUTT_P3_A3_Bend | REAL | 2 | 92-93 | D25112-D25113 | D25352-D25353 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 56 | BUTT_P3_A4_PushDist | REAL | 2 | 94-95 | D25114-D25115 | D25354-D25355 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 57 | BUTT_P3_A4_Bend | REAL | 2 | 96-97 | D25116-D25117 | D25356-D25357 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 58 | BUTT_P4_Result | INT | 1 | 98 | D25118 | D25358 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 59 | BUTT_P4_AttemptCount | INT | 1 | 99 | D25119 | D25359 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 60 | BUTT_P4_DecideAttempt | INT | 1 | 100 | D25120 | D25360 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 61 | BUTT_P4_Reserve | INT | 1 | 101 | D25121 | D25361 | u16 raw reserve (Excel INT signed) | Đúng offset/width; khác dấu | INT signed trong Excel, Rust giữ nguyên bit u16; không tính nghiệp vụ |
| 62 | BUTT_P4_Position | REAL | 2 | 102-103 | D25122-D25123 | D25362-D25363 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 63 | BUTT_P4_A1_PushDist | REAL | 2 | 104-105 | D25124-D25125 | D25364-D25365 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 64 | BUTT_P4_A1_Bend | REAL | 2 | 106-107 | D25126-D25127 | D25366-D25367 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 65 | BUTT_P4_A2_PushDist | REAL | 2 | 108-109 | D25128-D25129 | D25368-D25369 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 66 | BUTT_P4_A2_Bend | REAL | 2 | 110-111 | D25130-D25131 | D25370-D25371 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 67 | BUTT_P4_A3_PushDist | REAL | 2 | 112-113 | D25132-D25133 | D25372-D25373 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 68 | BUTT_P4_A3_Bend | REAL | 2 | 114-115 | D25134-D25135 | D25374-D25375 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 69 | BUTT_P4_A4_PushDist | REAL | 2 | 116-117 | D25136-D25137 | D25376-D25377 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 70 | BUTT_P4_A4_Bend | REAL | 2 | 118-119 | D25138-D25139 | D25378-D25379 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 72 | TIP_Result | INT | 1 | 120 | D25140 | D25380 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 73 | TIP_PointCount | INT | 1 | 121 | D25141 | D25381 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 74 | TIP_NGPoint | INT | 1 | 122 | D25142 | D25382 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 75 | TIP_Reserve1 | INT | 1 | 123 | D25143 | D25383 | u16 raw reserve (Excel INT signed) | Đúng offset/width; khác dấu | INT signed trong Excel, Rust giữ nguyên bit u16; không tính nghiệp vụ |
| 76 | TIP_Load | REAL | 2 | 124-125 | D25144-D25145 | D25384-D25385 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 77 | TIP_Reserve2 | INT[2] | 2 | 126-127 | D25146-D25147 | D25386-D25387 | u16 raw reserve (Excel INT signed) | Đúng offset/width; khác dấu | INT signed trong Excel, Rust giữ nguyên bit u16; không tính nghiệp vụ |
| 78 | TIP_P1_Result | INT | 1 | 128 | D25148 | D25388 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 79 | TIP_P1_AttemptCount | INT | 1 | 129 | D25149 | D25389 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 80 | TIP_P1_DecideAttempt | INT | 1 | 130 | D25150 | D25390 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 81 | TIP_P1_Reserve | INT | 1 | 131 | D25151 | D25391 | u16 raw reserve (Excel INT signed) | Đúng offset/width; khác dấu | INT signed trong Excel, Rust giữ nguyên bit u16; không tính nghiệp vụ |
| 82 | TIP_P1_Position | REAL | 2 | 132-133 | D25152-D25153 | D25392-D25393 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 83 | TIP_P1_A1_PushDist | REAL | 2 | 134-135 | D25154-D25155 | D25394-D25395 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 84 | TIP_P1_A1_Bend | REAL | 2 | 136-137 | D25156-D25157 | D25396-D25397 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 85 | TIP_P1_A2_PushDist | REAL | 2 | 138-139 | D25158-D25159 | D25398-D25399 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 86 | TIP_P1_A2_Bend | REAL | 2 | 140-141 | D25160-D25161 | D25400-D25401 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 87 | TIP_P1_A3_PushDist | REAL | 2 | 142-143 | D25162-D25163 | D25402-D25403 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 88 | TIP_P1_A3_Bend | REAL | 2 | 144-145 | D25164-D25165 | D25404-D25405 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 89 | TIP_P1_A4_PushDist | REAL | 2 | 146-147 | D25166-D25167 | D25406-D25407 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 90 | TIP_P1_A4_Bend | REAL | 2 | 148-149 | D25168-D25169 | D25408-D25409 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 91 | TIP_P2_Result | INT | 1 | 150 | D25170 | D25410 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 92 | TIP_P2_AttemptCount | INT | 1 | 151 | D25171 | D25411 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 93 | TIP_P2_DecideAttempt | INT | 1 | 152 | D25172 | D25412 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 94 | TIP_P2_Reserve | INT | 1 | 153 | D25173 | D25413 | u16 raw reserve (Excel INT signed) | Đúng offset/width; khác dấu | INT signed trong Excel, Rust giữ nguyên bit u16; không tính nghiệp vụ |
| 95 | TIP_P2_Position | REAL | 2 | 154-155 | D25174-D25175 | D25414-D25415 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 96 | TIP_P2_A1_PushDist | REAL | 2 | 156-157 | D25176-D25177 | D25416-D25417 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 97 | TIP_P2_A1_Bend | REAL | 2 | 158-159 | D25178-D25179 | D25418-D25419 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 98 | TIP_P2_A2_PushDist | REAL | 2 | 160-161 | D25180-D25181 | D25420-D25421 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 99 | TIP_P2_A2_Bend | REAL | 2 | 162-163 | D25182-D25183 | D25422-D25423 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 100 | TIP_P2_A3_PushDist | REAL | 2 | 164-165 | D25184-D25185 | D25424-D25425 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 101 | TIP_P2_A3_Bend | REAL | 2 | 166-167 | D25186-D25187 | D25426-D25427 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 102 | TIP_P2_A4_PushDist | REAL | 2 | 168-169 | D25188-D25189 | D25428-D25429 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 103 | TIP_P2_A4_Bend | REAL | 2 | 170-171 | D25190-D25191 | D25430-D25431 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 104 | TIP_P3_Result | INT | 1 | 172 | D25192 | D25432 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 105 | TIP_P3_AttemptCount | INT | 1 | 173 | D25193 | D25433 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 106 | TIP_P3_DecideAttempt | INT | 1 | 174 | D25194 | D25434 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 107 | TIP_P3_Reserve | INT | 1 | 175 | D25195 | D25435 | u16 raw reserve (Excel INT signed) | Đúng offset/width; khác dấu | INT signed trong Excel, Rust giữ nguyên bit u16; không tính nghiệp vụ |
| 108 | TIP_P3_Position | REAL | 2 | 176-177 | D25196-D25197 | D25436-D25437 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 109 | TIP_P3_A1_PushDist | REAL | 2 | 178-179 | D25198-D25199 | D25438-D25439 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 110 | TIP_P3_A1_Bend | REAL | 2 | 180-181 | D25200-D25201 | D25440-D25441 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 111 | TIP_P3_A2_PushDist | REAL | 2 | 182-183 | D25202-D25203 | D25442-D25443 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 112 | TIP_P3_A2_Bend | REAL | 2 | 184-185 | D25204-D25205 | D25444-D25445 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 113 | TIP_P3_A3_PushDist | REAL | 2 | 186-187 | D25206-D25207 | D25446-D25447 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 114 | TIP_P3_A3_Bend | REAL | 2 | 188-189 | D25208-D25209 | D25448-D25449 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 115 | TIP_P3_A4_PushDist | REAL | 2 | 190-191 | D25210-D25211 | D25450-D25451 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 116 | TIP_P3_A4_Bend | REAL | 2 | 192-193 | D25212-D25213 | D25452-D25453 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 117 | TIP_P4_Result | INT | 1 | 194 | D25214 | D25454 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 118 | TIP_P4_AttemptCount | INT | 1 | 195 | D25215 | D25455 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 119 | TIP_P4_DecideAttempt | INT | 1 | 196 | D25216 | D25456 | i16 / int16 -> enum/count | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 120 | TIP_P4_Reserve | INT | 1 | 197 | D25217 | D25457 | u16 raw reserve (Excel INT signed) | Đúng offset/width; khác dấu | INT signed trong Excel, Rust giữ nguyên bit u16; không tính nghiệp vụ |
| 121 | TIP_P4_Position | REAL | 2 | 198-199 | D25218-D25219 | D25458-D25459 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 122 | TIP_P4_A1_PushDist | REAL | 2 | 200-201 | D25220-D25221 | D25460-D25461 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 123 | TIP_P4_A1_Bend | REAL | 2 | 202-203 | D25222-D25223 | D25462-D25463 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 124 | TIP_P4_A2_PushDist | REAL | 2 | 204-205 | D25224-D25225 | D25464-D25465 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 125 | TIP_P4_A2_Bend | REAL | 2 | 206-207 | D25226-D25227 | D25466-D25467 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 126 | TIP_P4_A3_PushDist | REAL | 2 | 208-209 | D25228-D25229 | D25468-D25469 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 127 | TIP_P4_A3_Bend | REAL | 2 | 210-211 | D25230-D25231 | D25470-D25471 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 128 | TIP_P4_A4_PushDist | REAL | 2 | 212-213 | D25232-D25233 | D25472-D25473 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 129 | TIP_P4_A4_Bend | REAL | 2 | 214-215 | D25234-D25235 | D25474-D25475 | f32 / real32 (2 WORD) | Đúng | Địa chỉ/offset/width và hàm đọc khớp |
| 131 | Reserve_End | INT[24] | 24 | 216-239 | D25236-D25259 | D25476-D25499 | u16 raw reserve (Excel INT signed) | Đúng offset/width; khác dấu | INT signed trong Excel, Rust giữ nguyên bit u16; không tính nghiệp vụ |
