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
   - tạo luật Windows Firewall cho Toss nếu chưa có;
   - in ra checklist các bước tay bên dưới.

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

## Cổng UDP (tuỳ chọn)

Mặc định không cần mở cổng nào, Toss tự đục NAT hoặc đi qua relay. Chi tiết trong `docs/INSTALL.md`.
