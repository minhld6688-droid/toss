# Cài đặt Toss

Từ **v0.6** máy nào cài Toss cũng như nhau: máy nào cũng gửi, nhận, tạo link được. Không còn "máy tổng" hay "editor".

Tải bản mới nhất: `https://github.com/minhld6688-droid/toss/releases/latest`
(trực tiếp: `.../releases/latest/download/Toss.dmg` cho Mac, `.../Toss-setup.exe` cho Windows). Từ 0.5.2 trở đi Toss tự cập nhật; muốn kiểm tra ngay thì bấm **Cập nhật** ở góc Trang chính.

---

## 1. Cài

### Mac

1. Mở `Toss.dmg`, kéo **Toss** vào **Applications**.
2. Bản chưa ký Apple nên lần đầu macOS chặn: vào **Applications**, **chuột phải vào Toss → Open → Open** (một lần). Vẫn bị chặn: System Settings → Privacy & Security → **Open Anyway**.
3. Mở Toss, đặt tên máy (người khác thấy tên này) và chọn folder nhận file (mặc định `~/Toss`).

### Windows

1. Chạy `Toss-setup.exe`. Nếu hiện **Windows protected your PC**: **More info → Run anyway**.
2. Mở Toss một lần sau khi cài. Nếu Toss hỏi **Cho phép qua firewall**, bấm đồng ý để máy khác nối thẳng (nhanh hơn).

---

## 2. Dùng

- **Ghép máy (một lần)**: Trang chính → **Thêm máy** → gửi lời mời cho máy kia. Từ đó hai máy thấy nhau trong **Danh bạ**.
- **Gửi**: kéo folder thả vào **tên một máy** trong danh bạ. Hoặc:
  - Mac: thả lên **icon Toss trên thanh menu** — cửa sổ bật ra ngay dưới icon, kéo tiếp vào tên người nhận. Thả thẳng lên icon (hoặc lên icon Toss ở Dock) thì hiện bảng **Gửi tới ai?**.
  - **Chuột phải vào folder → Gửi bằng Toss** (Windows 11: *Show more options*; Mac: *Quick Actions*) → **Gửi tới ai?**
  - Thả folder vào chỗ trống trong cửa sổ Toss → **Gửi tới ai?**
  - Windows: khay hệ thống của Windows không nhận thả file lên icon (giới hạn của Windows) — dùng chuột phải hoặc kéo vào cửa sổ.
- **Gửi cho người chưa có trong danh bạ**: trong **Gửi tới ai?** chọn **Tạo link**. Link tự copy sẵn; dán vào Zalo/Messenger. Bật **Cho phép gửi file ngược lại** nếu muốn họ gửi lại về máy bạn.
- **Nhận**: bấm link người khác gửi (hoặc dán vào ô trên Trang chính). File nằm trong `<folder nhận>/<tên>/`. Từ danh bạ thì file tự về, không cần bấm gì.
- Đứt mạng / tắt máy giữa chừng: mở lại Toss (hoặc bấm lại link), Toss chạy tiếp từ chỗ dở. Gửi cho máy đang offline: file chờ sẵn, máy đó mở Toss là tự đi.

### Máy luôn bật, không có người ngồi (vd. máy ở studio)

Không cần chế độ gì riêng, chỉ cần máy luôn chạy Toss:

1. Cài `Toss-setup.exe` **cho tất cả người dùng** (tự thêm luật Windows Firewall cho `Toss.exe`, chiều vào, UDP).
2. Chạy `installer/host-setup/windows-host.ps1` bằng PowerShell *Run as administrator*:
   `powershell -ExecutionPolicy Bypass -File .\windows-host.ps1` — tắt sleep, tạo Scheduled Task chạy `Toss.exe --hidden` lúc đăng nhập. Chi tiết: [`installer/host-setup/README.md`](../installer/host-setup/README.md).
3. Bật tự đăng nhập Windows (`netplwiz`), BIOS **Restore on AC Power Loss = Power On**.
4. Muốn điều khiển máy này từ xa (tạo link trong folder của nó, bảo nó tự cập nhật): trên máy đó **Cài đặt → Nâng cao → Link điều khiển**; dán link vào máy của bạn ở **Cài đặt → Nâng cao → Điều khiển máy khác**. Muốn tạo link cho folder con bên trong một kho lớn: thêm kho đó ở **Nâng cao → Folder cố định**.

### Cổng UDP 41820

Máy nào đã tạo link hoặc có folder cố định sẽ dùng cổng UDP **41820**. Mở cổng này trên router (hoặc bật UPnP) để máy ở xa nối **Trực tiếp** thay vì qua relay chậm. Chi tiết ở mục 3.

---

## 3. Tốc độ & kết nối

### Vì sao Relay chậm

Toss luôn thử hai đường cùng lúc:

- **Trực tiếp (Direct)**: file đi thẳng từ máy ở studio tới máy ở xa qua UDP. Nhanh (đo thực tế 25-33 MB/s trên đường truyền tốt, chỉ bị giới hạn bởi mạng hai đầu).
- **Relay**: file đi vòng qua một máy chủ trung gian. Relay công cộng mặc định ở nước ngoài và bị giới hạn tốc độ cho mỗi kết nối, nên chỉ còn khoảng 1 MB/s (ví dụ đo 22/10: một job 57 MB giữa hai máy **cùng mạng** chạy 998 KB/s vì đi qua relay). Relay vẫn đảm bảo chuyển được file khi Direct không thông, nhưng chậm.

Mọi thứ dưới đây nhằm một việc: để thẻ truyền hiện **Trực tiếp** thay vì **Relay**. Nếu máy ở xa ngồi cùng mạng LAN với máy ở studio, Toss tự tìm nhau qua mDNS và nên lên Direct trong vài giây mà không phải cấu hình gì.

### Firewall (Windows)

- Cài Toss bằng **"cho tất cả người dùng"** (có UAC) thì trình cài tự thêm **một** luật tên `Toss`: chiều vào, giao thức **UDP**, chương trình `Toss.exe`. Không mở TCP.
- Cài "chỉ mình tôi" không có quyền Administrator, hoặc ai đó đã xoá luật: mở Toss → **Cài đặt → Nâng cao → Kết nối**, dòng **Firewall**. Nếu hiện "Thiếu", bấm **Cho phép qua firewall** (một lần hỏi quyền Administrator).
- Hoặc chạy `installer/host-setup/windows-host.ps1` bằng PowerShell Administrator, script cũng tạo đúng luật này (và thay luật cũ chỉ-theo-chương-trình).
- Tự kiểm tra: PowerShell `Get-NetFirewallRule -DisplayName Toss | Get-NetFirewallPortFilter` phải thấy `Protocol : UDP`.
- Mac: cùng nút đó (hỏi mật khẩu quản trị), hoặc System Settings → Network → Firewall → Options → thêm Toss và chọn *Allow incoming connections*. Nếu tường lửa Mac đang tắt thì không cần làm gì.

### Router studio: bật UPnP hoặc forward UDP 41820

Máy ở studio nghe ở cổng UDP **41820** cố định (nếu cổng bị chương trình khác chiếm, Toss dùng cổng ngẫu nhiên và ghi lý do trong bảng Kết nối). Có hai cách để máy ở xa ở ngoài Internet vào được cổng đó:

1. **UPnP / NAT-PMP / PCP (tự động)**: vào trang quản trị router, bật UPnP (NAT-PMP, PCP nếu có). Toss (mục *Port mapping* đang bật mặc định) tự xin router mở cổng. Thành công thì bảng Kết nối hiện *Port mapping: đã map* kèm địa chỉ công cộng, khả năng nối trực tiếp thành **Cao**.
2. **Forward tay**: nếu router không có UPnP hoặc đã tắt vì an ninh. Trong *Port Forwarding / Virtual Server*: giao thức **UDP**, cổng ngoài **41820**, cổng trong **41820**, IP đích = IP LAN của máy ở studio. Trước đó đặt cho máy ở studio **IP cố định** hoặc *DHCP reservation* theo MAC, nếu không IP đổi là luật hỏng. Không cần DMZ và không cần forward TCP.

Lưu ý:

- **Hai lớp NAT** (modem nhà mạng + router riêng phía sau): forward ở cả hai lớp, hoặc chuyển modem sang chế độ bridge.
- **CGNAT**: nếu địa chỉ WAN trên router không phải IP công cộng (dạng `100.64.x.x`, `10.x.x.x`, `192.168.x.x`) thì nhà mạng đang dùng NAT nhiều lớp, forward không có tác dụng. Xin nhà mạng IP công cộng (thường miễn phí hoặc vài chục nghìn mỗi tháng) hoặc dùng IPv6 / relay riêng bên dưới.
- Sau khi mở cổng, **tạo link job mới**: địa chỉ công cộng được ghi vào link lúc tạo, link cũ vẫn mang địa chỉ cũ (Toss vẫn nối được qua relay, chỉ chậm).

### Máy ở studio nối dây LAN

Cắm máy ở studio bằng **cáp mạng (Ethernet)**, đừng dùng Wi-Fi. Wi-Fi làm mất gói và độ trễ dao động, ảnh hưởng nặng tới UDP/QUIC; port mapping qua Wi-Fi cũng hay chập chờn. Nếu máy có cả LAN và Wi-Fi, tắt Wi-Fi để Windows không chọn nhầm. Cổng switch/router nên 1 Gbps.

### IPv6

Nếu cả mạng studio và mạng máy ở xa đều có **IPv6 công cộng** (bảng Kết nối hiện địa chỉ IPv6 và khả năng **Cao**), hai máy có thể nối trực tiếp mà không cần forward IPv4. Nhiều router chặn IPv6 đi vào theo mặc định: nếu vẫn ra Relay, vào mục *IPv6 firewall* của router và cho phép UDP 41820 tới máy ở studio. Luật Windows Firewall ở trên đã bao gồm IPv6.

### Đọc bảng Kết nối (Cài đặt → Nâng cao → Kết nối)

Bảng cho biết tình trạng mạng **của máy đang mở Toss**. Quan trọng nhất là xem ở **máy ở studio**, rồi dùng nút thử từ máy ở xa.

| Mục | Ý nghĩa |
| --- | --- |
| **Khả năng nối trực tiếp: Cao** | Có cổng đã được router map (UPnP) hoặc có IPv6 công cộng. Máy ở xa ở xa gần như chắc chắn lên Trực tiếp. |
| **Trung bình** | UDP ra Internet được và NAT không đổi cổng theo từng đích, nên đục NAT (hole punching) thường thành công, nhưng không chắc với mọi mạng của máy ở xa. Mở cổng UDP 41820 để lên Cao. |
| **Thấp** | NAT đối xứng hoặc không có UDP: gần như chắc chắn đi qua Relay. Xem lý do bên dưới, thường phải forward cổng, dùng IPv6 hoặc relay riêng. |
| **Đang đo** | Toss chưa đo xong, đợi vài giây. |
| Relay + chấm xanh | Đang nối với relay (cần để Toss tìm nhau và làm đường dự phòng). Chấm đỏ: không ra được Internet hoặc relay bị chặn. |
| Cổng UDP | Cổng máy này đang nghe. Máy ở studio phải là **41820**. Cổng ngẫu nhiên nghĩa là chưa bật chế độ máy ở studio hoặc 41820 đang bị chiếm. |
| Port mapping | `đã map` (kèm địa chỉ công cộng) = router đã mở cổng. `không khả dụng` = router không hỗ trợ/đã tắt UPnP. `đang dò` = đợi. `tắt` = đã tắt trong Nâng cao. |
| NAT | UDP có/không, IP công cộng v4/v6, NAT đối xứng (có/không), hairpin, độ trễ tới relay. |
| Firewall | `OK` / `Thiếu` (có nút **Cho phép qua firewall**) / không rõ. |
| Lý do | Các câu ngắn nói thẳng việc cần làm, ví dụ "Router không hỗ trợ UPnP — mở cổng UDP 41820 thủ công để máy ở xa nối trực tiếp", "NAT đối xứng: máy ở xa sẽ đi qua relay", "Chưa nối được relay". |
| **Thử nối tới máy đó** | Nối thử tới máy kia (dùng link điều khiển hoặc một link Toss dán vào), đợi tối đa 8 giây hoặc tới khi lên Trực tiếp, rồi cho biết đường đi (Trực tiếp/Relay), độ trễ (ms), thời gian nối và lý do nếu không phải Trực tiếp. Không tải gì cả. |
| **Xuất log** | Tạo file zip log và cài đặt (không có khoá bí mật) ở màn hình Desktop để gửi cho người hỗ trợ. |

Trên thẻ truyền, cạnh nhãn **Trực tiếp / Relay** là độ trễ (ví dụ `12 ms`); khi đang là Relay có link **Vì sao?** mở thẳng bảng Kết nối.

### Relay riêng (khi Direct không được)

Một số mạng (NAT đối xứng, CGNAT, mạng khách sạn/công ty chặn UDP) không bao giờ nối trực tiếp được. Khi đó thay vì relay công cộng ở xa, studio có thể thuê một VPS nhỏ trong nước làm **relay riêng**: nhanh hơn nhiều, chi phí khoảng vài trăm nghìn đồng mỗi tháng, và Toss vẫn tự quay về relay công cộng nếu relay riêng sập. Cách làm 6 bước, chi phí và theo dõi: [`hub/relay/README.md`](../hub/relay/README.md). Cuối cùng dán địa chỉ vào **Cài đặt → Nâng cao → Kết nối → Nâng cao → Relay riêng** trên **cả máy ở studio và các máy ở xa**.

### Checklist nhanh khi thẻ truyền hiện "Relay"

1. **Đợi 10-20 giây**: Toss thử lại địa chỉ sau 10 giây và nhiều khi tự nâng lên Trực tiếp. Không cần dừng việc đang tải.
2. **Máy ở studio đang bật và Toss chạy ở chế độ máy ở studio?** Cài đặt → Nâng cao → Kết nối → *Cổng UDP* phải là **41820**.
3. **Firewall = OK?** Nếu "Thiếu", bấm **Cho phép qua firewall**.
4. **Port mapping = đã map?** Nếu không: bật UPnP trên router, hoặc forward tay UDP 41820 tới IP cố định của máy ở studio. Rồi **tạo link job mới**.
5. **Máy ở studio cắm dây LAN**, không dùng Wi-Fi.
6. **Địa chỉ WAN của router có phải IP công cộng?** Nếu là `100.64.x.x` / `10.x` / `192.168.x` thì nhà mạng đang CGNAT: xin IP công cộng hoặc dùng relay riêng.
7. **Tắt VPN / proxy** ở cả hai đầu. Máy ở xa đang ở mạng công ty, khách sạn, quán cà phê hoặc dùng 4G thường chặn UDP hoặc NAT đối xứng: chấp nhận Relay hoặc dùng relay riêng.
8. **Máy ở xa ngồi cùng LAN với máy ở studio** mà vẫn Relay: kiểm tra cùng một mạng (không phải Wi-Fi khách / AP isolation / VLAN khác); tắt "client isolation" trên Wi-Fi.
9. Trên máy ở xa bấm **Thử nối tới máy đó** và đọc dòng *Lý do*.
10. Vẫn không được: bấm **Xuất log** và gửi file zip cho người hỗ trợ.

---

## 4. Link hoạt động thế nào

- Link web dạng `https://<trang>/#<ticket>`. Phần sau dấu `#` **không bao giờ gửi lên server**; trang chỉ chuyển nó thành `toss://t/<ticket>` để mở app Toss trên máy bạn.
- Nếu 1,5 giây sau Toss chưa mở, trang hiện nút **Mở lại trong Toss** và nút tải bản cài. Cài xong, **mở Toss một lần**, rồi bấm lại link.
- Ticket là một chuỗi tự chứa quyền (folder nào, nhận hay gửi, hết hạn khi nào), ký bằng khoá của máy ở studio. Không có server trung gian. File đi thẳng từ máy ở studio tới máy ở xa.

---

## 5. Xử lý sự cố

| Triệu chứng | Nguyên nhân thường gặp / cách xử lý |
| --- | --- |
| "Host offline" / không kết nối được | Máy ở studio đang tắt, đang restart, hoặc kẹt ở màn hình đăng nhập. Kiểm tra auto-login và Scheduled Task (`Get-ScheduledTask Toss`). Toss tự thử lại, cứ để cửa sổ mở. |
| Báo "Busy" / xếp hàng | Máy ở studio giới hạn số job chạy cùng lúc (mặc định 2). Toss tự thử lại mỗi 30 giây trong tối đa 1 giờ. |
| Tốc độ chậm | Xem nhãn kết nối trong tiến trình: **Direct** là nhanh, **Relay** là đi vòng nên chậm. Làm theo "Checklist nhanh khi thẻ truyền hiện Relay" ở mục 3 (firewall, UPnP / forward UDP 41820, cắm LAN, tắt VPN, relay riêng). Đôi khi sau vài giây kết nối tự nâng từ Relay lên Direct. |
| Bấm link mà Toss không mở | Chưa cài hoặc chưa mở Toss lần nào. Cài, mở một lần, bấm lại link. Trình duyệt hỏi "Open Toss?" thì chọn Open. Dùng nút **Copy ticket** rồi dán vào Toss nếu vẫn không được. |
| "Link hết hạn" / "Đã thu hồi" | Nhờ studio tạo link mới. |
| Mac nói Toss "đã hỏng" / "không thể mở" | Chuột phải → Open (xem mục Mac). Nếu vẫn lỗi: `xattr -dr com.apple.quarantine /Applications/Toss.app`. |
| Tải xong file báo lỗi hash | Toss tự tải lại file đó một lần. Nếu vẫn lỗi, kiểm tra ổ đĩa đầy hoặc phần mềm diệt virus đang khoá file. |
| Windows Update khởi động lại máy ở studio | Bình thường: máy tự đăng nhập, Task chạy Toss, online lại sau ~1 phút. Máy ở xa đang tải sẽ tự nối tiếp. |
