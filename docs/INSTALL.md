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

1. **Cài Toss** bằng `Toss-setup.exe`, chọn cài **cho tất cả người dùng** (cần quyền Administrator). Bước này tự thêm luật Windows Firewall cho Toss (chiều vào, **UDP**, chỉ cho `Toss.exe`). Cài "chỉ mình tôi" hoặc luật bị mất thì bấm nút **Cho phép qua firewall** trong Cài đặt → Kết nối (xem mục 3).
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

### Cổng UDP 41820

Máy tổng dùng cổng UDP cố định **41820**. Mở cổng này trên router studio (hoặc bật UPnP) để editor ở xa nối **Trực tiếp** thay vì qua relay chậm. Hướng dẫn từng bước ở mục 3 bên dưới.

---

## 3. Tốc độ & kết nối

### Vì sao Relay chậm

Toss luôn thử hai đường cùng lúc:

- **Trực tiếp (Direct)**: file đi thẳng từ máy tổng tới editor qua UDP. Nhanh (đo thực tế 25-33 MB/s trên đường truyền tốt, chỉ bị giới hạn bởi mạng hai đầu).
- **Relay**: file đi vòng qua một máy chủ trung gian. Relay công cộng mặc định ở nước ngoài và bị giới hạn tốc độ cho mỗi kết nối, nên chỉ còn khoảng 1 MB/s (ví dụ đo 22/10: một job 57 MB giữa hai máy **cùng mạng** chạy 998 KB/s vì đi qua relay). Relay vẫn đảm bảo chuyển được file khi Direct không thông, nhưng chậm.

Mọi thứ dưới đây nhằm một việc: để thẻ truyền hiện **Trực tiếp** thay vì **Relay**. Nếu editor ngồi cùng mạng LAN với máy tổng, Toss tự tìm nhau qua mDNS và nên lên Direct trong vài giây mà không phải cấu hình gì.

### Firewall (Windows)

- Cài Toss bằng **"cho tất cả người dùng"** (có UAC) thì trình cài tự thêm **một** luật tên `Toss`: chiều vào, giao thức **UDP**, chương trình `Toss.exe`. Không mở TCP.
- Cài "chỉ mình tôi" không có quyền Administrator, hoặc ai đó đã xoá luật: mở Toss → **Cài đặt → Kết nối**, dòng **Firewall**. Nếu hiện "Thiếu", bấm **Cho phép qua firewall** (một lần hỏi quyền Administrator).
- Hoặc chạy `installer/host-setup/windows-host.ps1` bằng PowerShell Administrator, script cũng tạo đúng luật này (và thay luật cũ chỉ-theo-chương-trình).
- Tự kiểm tra: PowerShell `Get-NetFirewallRule -DisplayName Toss | Get-NetFirewallPortFilter` phải thấy `Protocol : UDP`.
- Mac: cùng nút đó (hỏi mật khẩu quản trị), hoặc System Settings → Network → Firewall → Options → thêm Toss và chọn *Allow incoming connections*. Nếu tường lửa Mac đang tắt thì không cần làm gì.

### Router studio: bật UPnP hoặc forward UDP 41820

Máy tổng nghe ở cổng UDP **41820** cố định (nếu cổng bị chương trình khác chiếm, Toss dùng cổng ngẫu nhiên và ghi lý do trong bảng Kết nối). Có hai cách để editor ở ngoài Internet vào được cổng đó:

1. **UPnP / NAT-PMP / PCP (tự động)**: vào trang quản trị router, bật UPnP (NAT-PMP, PCP nếu có). Toss (mục *Port mapping* đang bật mặc định) tự xin router mở cổng. Thành công thì bảng Kết nối hiện *Port mapping: đã map* kèm địa chỉ công cộng, khả năng nối trực tiếp thành **Cao**.
2. **Forward tay**: nếu router không có UPnP hoặc đã tắt vì an ninh. Trong *Port Forwarding / Virtual Server*: giao thức **UDP**, cổng ngoài **41820**, cổng trong **41820**, IP đích = IP LAN của máy tổng. Trước đó đặt cho máy tổng **IP cố định** hoặc *DHCP reservation* theo MAC, nếu không IP đổi là luật hỏng. Không cần DMZ và không cần forward TCP.

Lưu ý:

- **Hai lớp NAT** (modem nhà mạng + router riêng phía sau): forward ở cả hai lớp, hoặc chuyển modem sang chế độ bridge.
- **CGNAT**: nếu địa chỉ WAN trên router không phải IP công cộng (dạng `100.64.x.x`, `10.x.x.x`, `192.168.x.x`) thì nhà mạng đang dùng NAT nhiều lớp, forward không có tác dụng. Xin nhà mạng IP công cộng (thường miễn phí hoặc vài chục nghìn mỗi tháng) hoặc dùng IPv6 / relay riêng bên dưới.
- Sau khi mở cổng, **tạo link job mới**: địa chỉ công cộng được ghi vào link lúc tạo, link cũ vẫn mang địa chỉ cũ (Toss vẫn nối được qua relay, chỉ chậm).

### Máy tổng nối dây LAN

Cắm máy tổng bằng **cáp mạng (Ethernet)**, đừng dùng Wi-Fi. Wi-Fi làm mất gói và độ trễ dao động, ảnh hưởng nặng tới UDP/QUIC; port mapping qua Wi-Fi cũng hay chập chờn. Nếu máy có cả LAN và Wi-Fi, tắt Wi-Fi để Windows không chọn nhầm. Cổng switch/router nên 1 Gbps.

### IPv6

Nếu cả mạng studio và mạng editor đều có **IPv6 công cộng** (bảng Kết nối hiện địa chỉ IPv6 và khả năng **Cao**), hai máy có thể nối trực tiếp mà không cần forward IPv4. Nhiều router chặn IPv6 đi vào theo mặc định: nếu vẫn ra Relay, vào mục *IPv6 firewall* của router và cho phép UDP 41820 tới máy tổng. Luật Windows Firewall ở trên đã bao gồm IPv6.

### Đọc bảng Kết nối (Cài đặt → Kết nối)

Bảng cho biết tình trạng mạng **của máy đang mở Toss**. Quan trọng nhất là xem ở **máy tổng**, rồi dùng nút thử từ máy editor.

| Mục | Ý nghĩa |
| --- | --- |
| **Khả năng nối trực tiếp: Cao** | Có cổng đã được router map (UPnP) hoặc có IPv6 công cộng. Editor ở xa gần như chắc chắn lên Trực tiếp. |
| **Trung bình** | UDP ra Internet được và NAT không đổi cổng theo từng đích, nên đục NAT (hole punching) thường thành công, nhưng không chắc với mọi mạng của editor. Mở cổng UDP 41820 để lên Cao. |
| **Thấp** | NAT đối xứng hoặc không có UDP: gần như chắc chắn đi qua Relay. Xem lý do bên dưới, thường phải forward cổng, dùng IPv6 hoặc relay riêng. |
| **Đang đo** | Toss chưa đo xong, đợi vài giây. |
| Relay + chấm xanh | Đang nối với relay (cần để Toss tìm nhau và làm đường dự phòng). Chấm đỏ: không ra được Internet hoặc relay bị chặn. |
| Cổng UDP | Cổng máy này đang nghe. Máy tổng phải là **41820**. Cổng ngẫu nhiên nghĩa là chưa bật chế độ máy tổng hoặc 41820 đang bị chiếm. |
| Port mapping | `đã map` (kèm địa chỉ công cộng) = router đã mở cổng. `không khả dụng` = router không hỗ trợ/đã tắt UPnP. `đang dò` = đợi. `tắt` = đã tắt trong Nâng cao. |
| NAT | UDP có/không, IP công cộng v4/v6, NAT đối xứng (có/không), hairpin, độ trễ tới relay. |
| Firewall | `OK` / `Thiếu` (có nút **Cho phép qua firewall**) / không rõ. |
| Lý do | Các câu ngắn nói thẳng việc cần làm, ví dụ "Router không hỗ trợ UPnP — mở cổng UDP 41820 thủ công để editor ở xa nối trực tiếp", "NAT đối xứng: editor ở xa sẽ đi qua relay", "Chưa nối được relay". |
| **Thử nối tới máy tổng** | Từ máy editor: nối thử tới máy tổng (dùng link admin hoặc link dán vào), đợi tối đa 8 giây hoặc tới khi lên Trực tiếp, rồi cho biết đường đi (Trực tiếp/Relay), độ trễ (ms), thời gian nối và lý do nếu không phải Trực tiếp. Không tải gì cả. |
| **Xuất log** | Tạo file zip log và cài đặt (không có khoá bí mật) ở màn hình Desktop để gửi cho người hỗ trợ. |

Trên thẻ truyền, cạnh nhãn **Trực tiếp / Relay** là độ trễ (ví dụ `12 ms`); khi đang là Relay có link **Vì sao?** mở thẳng bảng Kết nối.

### Relay riêng (khi Direct không được)

Một số mạng (NAT đối xứng, CGNAT, mạng khách sạn/công ty chặn UDP) không bao giờ nối trực tiếp được. Khi đó thay vì relay công cộng ở xa, studio có thể thuê một VPS nhỏ trong nước làm **relay riêng**: nhanh hơn nhiều, chi phí khoảng vài trăm nghìn đồng mỗi tháng, và Toss vẫn tự quay về relay công cộng nếu relay riêng sập. Cách làm 6 bước, chi phí và theo dõi: [`hub/relay/README.md`](../hub/relay/README.md). Cuối cùng dán địa chỉ vào **Cài đặt → Kết nối → Nâng cao → Relay riêng** trên **cả máy tổng và các editor**.

### Checklist nhanh khi thẻ truyền hiện "Relay"

1. **Đợi 10-20 giây**: Toss thử lại địa chỉ sau 10 giây và nhiều khi tự nâng lên Trực tiếp. Không cần dừng việc đang tải.
2. **Máy tổng đang bật và Toss chạy ở chế độ máy tổng?** Cài đặt → Kết nối → *Cổng UDP* phải là **41820**.
3. **Firewall = OK?** Nếu "Thiếu", bấm **Cho phép qua firewall**.
4. **Port mapping = đã map?** Nếu không: bật UPnP trên router, hoặc forward tay UDP 41820 tới IP cố định của máy tổng. Rồi **tạo link job mới**.
5. **Máy tổng cắm dây LAN**, không dùng Wi-Fi.
6. **Địa chỉ WAN của router có phải IP công cộng?** Nếu là `100.64.x.x` / `10.x` / `192.168.x` thì nhà mạng đang CGNAT: xin IP công cộng hoặc dùng relay riêng.
7. **Tắt VPN / proxy** ở cả hai đầu. Editor đang ở mạng công ty, khách sạn, quán cà phê hoặc dùng 4G thường chặn UDP hoặc NAT đối xứng: chấp nhận Relay hoặc dùng relay riêng.
8. **Editor ngồi cùng LAN với máy tổng** mà vẫn Relay: kiểm tra cùng một mạng (không phải Wi-Fi khách / AP isolation / VLAN khác); tắt "client isolation" trên Wi-Fi.
9. Trên máy editor bấm **Thử nối tới máy tổng** và đọc dòng *Lý do*.
10. Vẫn không được: bấm **Xuất log** và gửi file zip cho người hỗ trợ.

---

## 4. Link hoạt động thế nào

- Link web dạng `https://<trang>/#<ticket>`. Phần sau dấu `#` **không bao giờ gửi lên server**; trang chỉ chuyển nó thành `toss://t/<ticket>` để mở app Toss trên máy bạn.
- Nếu 1,5 giây sau Toss chưa mở, trang hiện nút **Mở lại trong Toss** và nút tải bản cài. Cài xong, **mở Toss một lần**, rồi bấm lại link.
- Ticket là một chuỗi tự chứa quyền (folder nào, nhận hay gửi, hết hạn khi nào), ký bằng khoá của máy tổng. Không có server trung gian. File đi thẳng từ máy tổng tới editor.

---

## 5. Xử lý sự cố

| Triệu chứng | Nguyên nhân thường gặp / cách xử lý |
| --- | --- |
| "Host offline" / không kết nối được | Máy tổng đang tắt, đang restart, hoặc kẹt ở màn hình đăng nhập. Kiểm tra auto-login và Scheduled Task (`Get-ScheduledTask Toss`). Toss tự thử lại, cứ để cửa sổ mở. |
| Báo "Busy" / xếp hàng | Máy tổng giới hạn số job chạy cùng lúc (mặc định 2). Toss tự thử lại mỗi 30 giây trong tối đa 1 giờ. |
| Tốc độ chậm | Xem nhãn kết nối trong tiến trình: **Direct** là nhanh, **Relay** là đi vòng nên chậm. Làm theo "Checklist nhanh khi thẻ truyền hiện Relay" ở mục 3 (firewall, UPnP / forward UDP 41820, cắm LAN, tắt VPN, relay riêng). Đôi khi sau vài giây kết nối tự nâng từ Relay lên Direct. |
| Bấm link mà Toss không mở | Chưa cài hoặc chưa mở Toss lần nào. Cài, mở một lần, bấm lại link. Trình duyệt hỏi "Open Toss?" thì chọn Open. Dùng nút **Copy ticket** rồi dán vào Toss nếu vẫn không được. |
| "Link hết hạn" / "Đã thu hồi" | Nhờ studio tạo link mới. |
| Mac nói Toss "đã hỏng" / "không thể mở" | Chuột phải → Open (xem mục Mac). Nếu vẫn lỗi: `xattr -dr com.apple.quarantine /Applications/Toss.app`. |
| Tải xong file báo lỗi hash | Toss tự tải lại file đó một lần. Nếu vẫn lỗi, kiểm tra ổ đĩa đầy hoặc phần mềm diệt virus đang khoá file. |
| Windows Update khởi động lại máy tổng | Bình thường: máy tự đăng nhập, Task chạy Toss, online lại sau ~1 phút. Editor đang tải sẽ tự nối tiếp. |
