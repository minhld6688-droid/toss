# Cài đặt máy tổng (Windows)

Máy tổng là máy ở studio giữ file gốc và luôn bật để editor ở xa nhận file. Mục tiêu: **mất điện, Windows tự khởi động lại hay cập nhật xong thì Toss vẫn tự chạy lại, không cần ai ngồi bấm.**

## Làm một lần

1. Cài Toss (chọn **cài cho tất cả người dùng** để có luật firewall), mở Toss, bật **Chế độ máy tổng**, khai các folder gốc (RAW, Edit, ...) và folder nhận file về.
2. Mở **PowerShell bằng quyền Administrator** (chuột phải → Run as administrator), rồi chạy:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\windows-host.ps1
   ```

   Script sẽ:
   - tắt Sleep / Hibernate khi cắm điện, đóng nắp laptop không làm gì;
   - tạo Scheduled Task **Toss**: chạy `Toss.exe --hidden` mỗi khi tài khoản đăng nhập, quyền cao nhất, tự chạy lại tối đa 5 lần nếu app thoát bất thường;
   - xoá mục autostart `Run` cũ của Toss (tránh chạy 2 bản);
   - tạo **một** luật Windows Firewall tên `Toss`: chiều vào, **UDP**, cho `Toss.exe` (luật cũ chỉ theo chương trình hoặc luật Windows tự tạo khi hiện hộp thoại "Allow access" sẽ bị thay);
   - kiểm tra cổng **UDP 41820** có đang do Toss giữ không (Toss chỉ lắng nghe cổng này khi **Chế độ máy tổng đang bật**);
   - nếu có `toss-cli.exe` cạnh `Toss.exe` hoặc trong PATH: chạy `toss-cli diag` và in dòng **khả năng nối trực tiếp** (High/Medium/Low) cùng trạng thái **port mapping (UPnP)**. Không có thì bỏ qua, thêm `-SkipDiag` để tắt hẳn;
   - in ra checklist các bước tay bên dưới (gồm bật UPnP hoặc forward UDP 41820 trên router).

   Cần chỉ đường dẫn khác: `-ExePath "D:\Apps\Toss\Toss.exe"`. Gỡ task: thêm `-Uninstall`.

3. Làm tiếp các bước tay (script không tự làm được).

## Tự đăng nhập Windows (auto-login)

Scheduled Task chạy **sau khi tài khoản đăng nhập**, nên máy phải tự đăng nhập sau khi khởi động lại:

1. Bấm `Win + R`, gõ `netplwiz`, Enter.
2. Chọn tài khoản của máy tổng, **bỏ tick** "Users must enter a user name and password to use this computer", OK, nhập mật khẩu 2 lần.
3. Nếu không thấy ô tick: Settings → Accounts → Sign-in options → tắt "For improved security, only allow Windows Hello sign-in...", rồi mở lại `netplwiz`.

Lưu ý bảo mật: ai đứng trước máy này cũng vào được Windows. Để máy ở nơi an toàn, hoặc dùng tài khoản riêng chỉ có quyền trên các folder cần thiết.

## Windows Update và khởi động lại

Windows tự restart sau khi cập nhật thì: máy bật lại → auto-login → Scheduled Task **Toss** chạy → Toss nằm ở khay hệ thống, host online lại sau vài chục giây. Trong lúc restart, editor đang tải sẽ tự thử lại (cùng một link, file tải dở được nối tiếp, không tải lại từ đầu).

Nên làm thêm: Settings → Windows Update → **Advanced options → Active hours** đặt theo giờ studio để restart rơi vào ban đêm.

## Mất điện

Vào BIOS/UEFI, đặt **Restore on AC Power Loss = Power On** (tên khác nhau tuỳ hãng: "After Power Failure", "AC Back"). Có điện lại máy tự bật rồi chạy như trên. Nếu có UPS thì càng tốt.

## Kiểm tra

- Khởi động lại máy một lần, đừng chạm gì, sau ~1 phút biểu tượng Toss phải xuất hiện ở khay.
- PowerShell: `Get-ScheduledTask Toss | Get-ScheduledTaskInfo` cho biết lần chạy gần nhất và kết quả.
- Từ máy editor: mở một link job thử.

## Cổng UDP 41820 và router

Máy tổng dùng cổng UDP cố định **41820**. Để editor ở xa nối **trực tiếp** (nhanh) thay vì qua relay (chậm), bật **UPnP** trên router studio (Toss tự mở cổng) hoặc forward tay **UDP 41820** tới IP cố định của máy tổng. Không làm cũng không hỏng gì, chỉ có thể rơi vào relay. Các bước chi tiết và cách đọc bảng Kết nối: `docs/INSTALL.md`, mục "Tốc độ & kết nối".

Ghi chú: `toss-cli diag` trong script chạy bằng danh tính tạm (không đụng khoá của Toss đang chạy) nên nó đo mạng của máy này (NAT, UPnP), không đo riêng cổng 41820. Số liệu chính xác cho máy tổng thật nằm ở Toss → Cài đặt → Kết nối.
