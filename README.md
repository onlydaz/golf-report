# Thu Thập Dữ Liệu Độ Cong Gậy Golf

## 1. Giới thiệu

**New Ocean Group** — Phần mềm Thu Thập Dữ Liệu Độ Cong Gậy Golf.
Kết nối PLC Delta AS200, thu thập dữ liệu đo TIP/BUTT và lưu PostgreSQL Local.
Hỗ trợ theo dõi kết quả, xem lịch sử và xuất báo cáo Excel/CSV.

## 2. Tải và cài đặt

**Tải source:** dùng Git hoặc chọn Code → Download ZIP tại [GitHub](https://github.com/onlydaz/golf-report).

```powershell
git clone https://github.com/onlydaz/golf-report.git
cd golf-report
```

**Tải setup:** nếu đã có [Release](https://github.com/onlydaz/golf-report/releases),
tải `.exe`/`.msi` cho Windows hoặc `.deb` cho Ubuntu tại mục Assets.
Windows: chạy bộ cài theo hướng dẫn. Ubuntu: mở `.deb` bằng trình cài gói của hệ thống.
Bản setup không cần Node.js/Rust; vẫn cần PostgreSQL và các thành phần giao diện hệ thống.
Repo cũng có [workflow Ubuntu DEB](.github/workflows/build-linux.yml); kết quả nằm trong Artifacts của lần chạy.

**Nếu chạy source, cài trước:**

- [Node.js](https://nodejs.org/en/download) nhánh 22, phiên bản **22.12 trở lên**, kèm npm.
- [Rust stable qua rustup](https://rustup.rs/) và Git nếu tải bằng lệnh clone.
- PostgreSQL Local và pgAdmin 4 theo mục 3.
- Windows: Microsoft C++ Build Tools, chọn **Desktop development with C++** và Windows SDK;
  cài Microsoft Edge WebView2 Runtime nếu chưa có. Xem [Tauri Windows](https://v2.tauri.app/start/prerequisites/#windows).
- Ubuntu: cài thêm các thư viện dưới đây, theo workflow hiện tại của repo:

```bash
sudo apt-get update
sudo apt-get install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev patchelf
```

Workflow dùng Ubuntu 22.04. Hướng dẫn Linux được đối chiếu source, chưa kiểm chứng
cài đặt bản DEB trong lượt cập nhật README này; lưu ý vị trí cấu hình DB bên dưới.

## 3. Cài đặt PostgreSQL Local

1. Tải và cài từ [PostgreSQL Downloads](https://www.postgresql.org/download/).
   Bộ cài Windows EDB có Server và pgAdmin; có thể tải [pgAdmin 4](https://www.pgadmin.org/download/) riêng.
2. Ghi nhớ mật khẩu tài khoản `postgres` và cổng đã chọn; cấu hình mẫu dùng `5432`.
3. Mở pgAdmin 4, kết nối server Local bằng tài khoản `postgres`.
4. Chuột phải **Databases → Create → Database…**, đặt tên **`golf_report`**, chọn Owner rồi **Save**.

**Chuẩn bị cấu hình:** nếu file đích chưa có, sao chép:

- `database_config.example.toml` → `src-tauri/resources/database_config.toml` khi chạy source.
- `as200_config.example.toml` → `as200_config.toml` ở thư mục gốc khi chạy source.

Không ghi đè cấu hình hiện hữu; mở file và chỉnh cho đúng máy đang sử dụng.
Điền cấu hình PostgreSQL như sau, thay mật khẩu minh họa bằng mật khẩu của máy:

```toml
host = "localhost"
port = 5432
database = "golf_report"
username = "postgres"
password = "DIEN_MAT_KHAU_CUA_BAN"
```

Mật khẩu không được để trống hoặc đưa lên GitHub.
Trong `as200_config.toml`, chỉnh `address` theo IP/cổng PLC (mẫu `192.168.1.5:502`)
và `unit_id` theo thiết bị (mẫu `1`); giữ các thông số kỹ thuật do người triển khai cung cấp.

**Vị trí file cấu hình theo source hiện tại:**

| Cách chạy | `database_config.toml` | `as200_config.toml` |
|---|---|---|
| Source Windows/Ubuntu | `<project>/src-tauri/resources/database_config.toml` | `<project>/as200_config.toml` |
| Windows bản cài | `<thư mục golf-report.exe>/resources/database_config.toml` | `<thư mục golf-report.exe>/as200_config.toml` |
| Linux bản cài | `<thư mục executable>/resources/database_config.toml` | `${XDG_CONFIG_HOME:-$HOME/.config}/com.golf.report/as200_config.toml` |

Với DEB, nếu executable ở `/usr/bin/golf-report`, DB config được tìm tại
`/usr/bin/resources/database_config.toml`. Cần kiểm tra đường dẫn thực tế trong log
`DATABASE_CONFIG path=...`; không mặc định resource của gói DEB đã nằm đúng chỗ.

Ứng dụng tự tạo/cập nhật bảng khi kết nối PostgreSQL thành công; tài khoản DB cần quyền tạo bảng.
Nếu có banner lỗi kết nối, kiểm tra dịch vụ PostgreSQL và cấu hình rồi bấm **Thử kết nối lại**.

## 4. Chạy phần mềm

Mở Terminal/PowerShell tại thư mục gốc dự án, sau khi đã chuẩn bị cấu hình:

```powershell
npm ci
npm run tauri dev
```

Đóng gói bộ cài trên Windows:

```powershell
npm run tauri build
```

Kết quả: `src-tauri/target/release/bundle/nsis/` và `src-tauri/target/release/bundle/msi/`.
Đóng gói trên Ubuntu:

```bash
npm run tauri build -- --bundles deb
```

Kết quả: `src-tauri/target/release/bundle/deb/`.
`npm run build` chỉ build frontend; dùng lệnh Tauri phía trên để đóng gói ứng dụng.

Sau khi cài setup trên Windows, mở bằng shortcut trong Start Menu hoặc trên Desktop.
Trên Ubuntu, mở từ menu ứng dụng hoặc chạy `golf-report` trong Terminal nếu lệnh đã được cài vào PATH.
