# Cài đặt Toss

Toss có hai vai trò:

- **Editor** (máy Mac/Windows ở xa): cài Toss, bấm link để nhận folder job, kéo thả file đã làm xong để gửi ngược về.
- **Máy tổng** (một máy Windows ở studio, luôn bật): giữ file gốc, tạo link cho từng job.

Tải bản mới nhất ở trang Releases của dự án:
`https://github.com/minhld6688-droid/toss/releases/latest`
(trực tiếp: `.../releases/latest/download/Toss.dmg` cho Mac, `.../Toss-setup.exe` cho Windows).

---

## 1. Cho editor

### Mac

1. Mở `Toss.dmg`, kéo **Toss** vào thư mục **Applications**.
2. Bản hiện tại chưa ký Apple nên lần đầu macOS sẽ chặn. Vào **Applications**, **chuột phải vào Toss → Open → Open**. Chỉ cần làm một lần.
   Nếu vẫn bị chặn: System Settings → Privacy & Security → kéo xuống bấm **Open Anyway** cạnh dòng nói về Toss.
3. Mở Toss một lần để nó đăng ký link `toss://`, đặt tên máy và chọn folder nhận file (mặc định `~/Toss`).

### Windows

1. Chạy `Toss-setup.exe`.
2. Nếu hiện cửa sổ xanh **Windows protected your PC** (SmartScreen): bấm **More info → Run anyway**. (Do bản chưa ký số.)
3. Cài cho "chỉ mình tôi" là đủ cho editor. Mở Toss một lần sau khi cài.

### Dùng

- Bấm link `https://…/#toss1…` mà studio gửi (hoặc quét QR). Trình duyệt sẽ hỏi mở Toss, chọn **Open / Mở**.
- Toss hiện job và nút **Nhận**. Bấm là tải về, file nằm trong `<folder nhận>/<tên job>/`.
- Gửi ngược: nếu link cho phép gửi, kéo thả file/folder vào cửa sổ Toss.
- Tải dở bị ngắt (mất mạng, tắt máy) thì bấm lại đúng link đó, Toss nối tiếp từ chỗ dở, không tải lại file đã xong.

---

## 2. Cho máy tổng (Windows, ở studio)

1. **Cài Toss** bằng `Toss-setup.exe`, chọn cài **cho tất cả người dùng** (cần quyền Administrator). Bước này tự thêm luật Windows Firewall cho Toss.
2. Mở Toss → bật **Chế độ máy tổng**.
3. **Khai folder gốc**: thêm từng folder gốc kèm một tên ngắn (ví dụ `RAW` → `D:\Studio\RAW`, `EDIT` → `D:\Studio\Edit`) và **folder nhận file về** (nơi file editor gửi ngược lại sẽ rơi vào). Link chỉ truy cập được bên trong các folder gốc này.
4. **Chạy `windows-host.ps1`** (nằm trong `installer/host-setup/`): mở PowerShell bằng *Run as administrator*, rồi

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\windows-host.ps1
   ```

   Script tắt sleep, tạo Scheduled Task **Toss** (chạy `Toss.exe --hidden` lúc đăng nhập), và in checklist. Hướng dẫn chi tiết: [`installer/host-setup/README.md`](../installer/host-setup/README.md).
5. **Bật tự đăng nhập Windows** (`Win + R` → `netplwiz` → bỏ tick "Users must enter a user name and password…"). Không có bước này, sau khi Windows tự restart máy sẽ dừng ở màn hình đăng nhập và Toss không chạy.
6. Trong BIOS đặt **Restore on AC Power Loss = Power On** để máy tự bật lại sau mất điện.
7. Khởi động lại thử một lần, không chạm gì, đợi khoảng 1 phút: biểu tượng Toss phải hiện ở khay hệ thống.

### Tạo link cho một job

Trên máy tổng: chọn folder gốc + đường dẫn con, đặt tên job, chọn quyền **nhận (get)** và/hoặc **gửi (put)** và thời hạn, bấm tạo. Toss cho ra link web và mã QR. Gửi link cho editor qua Zalo/Telegram/email. Ai có link là dùng được, nên đặt thời hạn ngắn và thu hồi (revoke) khi xong job.

### Chia sẻ nhanh bằng chuột phải: "Tạo link Toss" (v0.3)

Máy nào cài Toss cũng chia sẻ được một folder mà không cần khai folder gốc: **chuột phải vào folder → Tạo link Toss**. Toss mở màn hình Chia sẻ, tạo link nhận folder đó, **tự copy link vào clipboard** và báo "Đã copy link Toss cho <folder>". Dán link vào Zalo/Telegram là xong. Nếu chế độ máy tổng đang tắt, Toss tự bật nó (file gửi ngược về nằm trong `<folder nhận>/Tra-ve`) và tự tắt lại khi bạn gỡ chia sẻ cuối cùng. Folder đã chia sẻ vẫn chạy sau khi khởi động lại máy; gỡ chia sẻ sẽ thu hồi mọi link của folder đó.

- **Windows**: mục **Tạo link Toss** có khi chuột phải vào một folder, và khi chuột phải vào chỗ trống bên trong một folder. Trên Windows 11 menu rút gọn không hiện mục này: bấm **Show more options** (hoặc giữ `Shift` khi chuột phải). Mục được ghi cho tài khoản Windows hiện tại (không cần quyền Administrator) và bị xoá khi gỡ Toss.
- **Mac**: mục nằm trong **chuột phải → Quick Actions → Tạo link Toss** (và trong menu **Finder → Services**). Lần đầu dùng, macOS có thể hỏi cho phép chạy, chọn **Allow/Cho phép**. Nếu chưa thấy mục: System Settings → Keyboard → Keyboard Shortcuts → Services (hoặc System Settings → Privacy & Security → Extensions → Finder) và bật **Tạo link Toss**. Bản Quick Action nằm ở `~/Library/Services/Tạo link Toss.workflow`.
- Tắt/bật mục này trong phần **Cài đặt** của Toss. Toss ghi lại mục này mỗi lần mở, nên chuyển Toss sang chỗ khác hoặc cài lại cũng không cần làm gì.

Có thể tạo link từ xa: tạo một **admin ticket** trên máy tổng và dán vào Toss trên máy tin cậy của bạn để tạo link job mà không cần ngồi ở studio.

### Cổng UDP (tuỳ chọn, nâng cao)

Mặc định **không cần mở cổng nào**: Toss (qua iroh) tự đục NAT để nối trực tiếp, và nếu không được thì đi vòng qua relay (chậm hơn nhưng vẫn chạy). Mạng studio nhiều lớp NAT hoặc firewall chặt thì thường rơi vào relay.

Mặc định Toss dùng cổng UDP ngẫu nhiên. Muốn forward cổng: vào **Cài đặt → Chế độ máy tổng → Cổng UDP cố định**, đặt một cổng (ví dụ 41641), mở lại Toss, trên router forward UDP cổng đó tới IP nội bộ của máy tổng (nên đặt IP tĩnh/DHCP reservation). Router bật UPnP/NAT-PMP thì thường không phải làm gì. Nếu không chắc, bỏ qua bước này.

---

## 3. Link hoạt động thế nào

- Link web dạng `https://<trang>/#<ticket>`. Phần sau dấu `#` **không bao giờ gửi lên server**; trang chỉ chuyển nó thành `toss://t/<ticket>` để mở app Toss trên máy bạn.
- Nếu 1,5 giây sau Toss chưa mở, trang hiện nút **Mở lại trong Toss** và nút tải bản cài. Cài xong, **mở Toss một lần**, rồi bấm lại link.
- Ticket là một chuỗi tự chứa quyền (folder nào, nhận hay gửi, hết hạn khi nào), ký bằng khoá của máy tổng. Không có server trung gian. File đi thẳng từ máy tổng tới editor.

---

## 4. Xử lý sự cố

| Triệu chứng | Nguyên nhân thường gặp / cách xử lý |
| --- | --- |
| "Host offline" / không kết nối được | Máy tổng đang tắt, đang restart, hoặc kẹt ở màn hình đăng nhập. Kiểm tra auto-login và Scheduled Task (`Get-ScheduledTask Toss`). Toss tự thử lại, cứ để cửa sổ mở. |
| Báo "Busy" / xếp hàng | Máy tổng giới hạn số job chạy cùng lúc (mặc định 2). Toss tự thử lại mỗi 30 giây trong tối đa 1 giờ. |
| Tốc độ chậm | Xem nhãn kết nối trong tiến trình: **Direct** là nhanh, **Relay** là đi vòng nên chậm. Thử cắm LAN cho máy tổng, tắt VPN trên hai đầu, hoặc xem mục cổng UDP ở trên. Đôi khi sau vài giây kết nối tự nâng từ Relay lên Direct. |
| Bấm link mà Toss không mở | Chưa cài hoặc chưa mở Toss lần nào. Cài, mở một lần, bấm lại link. Trình duyệt hỏi "Open Toss?" thì chọn Open. Dùng nút **Copy ticket** rồi dán vào Toss nếu vẫn không được. |
| "Link hết hạn" / "Đã thu hồi" | Nhờ studio tạo link mới. |
| Mac nói Toss "đã hỏng" / "không thể mở" | Chuột phải → Open (xem mục Mac). Nếu vẫn lỗi: `xattr -dr com.apple.quarantine /Applications/Toss.app`. |
| Tải xong file báo lỗi hash | Toss tự tải lại file đó một lần. Nếu vẫn lỗi, kiểm tra ổ đĩa đầy hoặc phần mềm diệt virus đang khoá file. |
| Windows Update khởi động lại máy tổng | Bình thường: máy tự đăng nhập, Task chạy Toss, online lại sau ~1 phút. Editor đang tải sẽ tự nối tiếp. |
