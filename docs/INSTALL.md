# Cài đặt Toss

Từ **v0.6** máy nào cài Toss cũng như nhau: máy nào cũng gửi, nhận, tạo link được. Không còn "máy tổng" hay "editor".
Từ **v0.8**: máy cùng mạng tự hiện ra trong mục **Gần đây** (cả điện thoại / máy tính chạy app **LocalSend**), Toss tự mở cùng máy, và bản cập nhật 0.8 tự bật "Mở cùng máy" một lần cho mọi máy.

Tải bản mới nhất: `https://github.com/minhld6688-droid/toss/releases/latest`
(trực tiếp: `.../releases/latest/download/Toss.dmg` cho Mac, `.../Toss-setup.exe` cho Windows). Từ 0.5.2 trở đi Toss tự cập nhật; muốn kiểm tra ngay thì bấm **Cập nhật** ở góc Trang chính.

---

## 1. Cài

### Mac

1. Mở `Toss.dmg`, kéo **Toss** vào **Applications**.
2. Bản chưa ký Apple nên lần đầu macOS chặn: vào **Applications**, **chuột phải vào Toss → Open → Open** (một lần). Vẫn bị chặn: System Settings → Privacy & Security → **Open Anyway**.
3. Mở Toss: một màn hình duy nhất — tên máy (người khác thấy tên này), folder nhận file (mặc định `~/Toss`), **Mở cùng máy** (đã bật sẵn) → **Bắt đầu**.
4. macOS hỏi **"Toss muốn tìm thiết bị trong mạng cục bộ"**: bấm **Cho phép** (không cho thì máy cùng mạng không thấy nhau và chuyển file rất chậm). Lỡ bấm từ chối: System Settings → Privacy & Security → **Local Network** → bật Toss, rồi mở lại Toss.

### Windows

1. Chạy `Toss-setup.exe`. Nếu hiện **Windows protected your PC**: **More info → Run anyway**.
2. Mở Toss một lần sau khi cài. Nếu Toss hỏi **Cho phép qua firewall**, bấm đồng ý để máy khác nối thẳng (nhanh hơn) và để điện thoại / máy LocalSend gửi được vào máy này.

---

## 2. Dùng

- **Gần đây**: máy cùng mạng (Wi-Fi / LAN) đang mở Toss hoặc **LocalSend** tự hiện ở Trang chính, không cần ghép. Kéo folder/file thả vào tên máy là gửi. Không thấy máy nào: hai máy phải cùng một mạng (không phải Wi-Fi khách), bấm nút làm mới; trên Mac kiểm tra quyền **Mạng cục bộ** (mục 1).
- **Điện thoại**: cài app **LocalSend** (miễn phí, iOS/Android). Máy Toss hiện trong LocalSend như một máy bình thường: gửi ảnh từ điện thoại vào máy tính, hoặc từ Toss chọn tên điện thoại để gửi sang.
- **Danh bạ (máy ở xa)**: Trang chính → **Mời máy ở xa** → gửi lời mời cho máy kia. Từ đó hai máy thấy nhau trong **Danh bạ** dù ở đâu (qua Internet).
- **Gửi**: kéo folder hoặc file thả vào **tên một máy**. Hoặc:
  - Mac: thả lên **icon Toss trên thanh menu** (hoặc icon ở Dock) → **Gửi tới ai?**.
  - **Chuột phải vào folder hoặc file → Gửi bằng Toss** (Windows 11: *Show more options*; Mac: *Quick Actions*) → **Gửi tới ai?**
  - Windows: chọn nhiều file → chuột phải → **Send to → Toss**.
  - Thả vào chỗ trống trong cửa sổ Toss → **Gửi tới ai?**
- **Gửi tới ai?**: *Gần đây* · *Danh bạ* · **Tạo link** (một file hoặc một folder; link tự copy sẵn, dán vào Zalo/Messenger) · **Chia sẻ qua trình duyệt** (máy cùng mạng mở link/quét QR là tải được, không cần cài gì). Có ô **Gửi chữ** để gửi nhanh một đoạn chữ (địa chỉ, ghi chú).
- **Nhận**: máy lạ gửi tới → hiện bảng "**<tên> muốn gửi <job>**": **Nhận** · **Luôn nhận từ máy này** · Từ chối. Xong có thông báo "Đã nhận …", bấm vào là mở folder. File nằm trong `<folder nhận>/<tên máy gửi>/<job>/`.
- **Ai được gửi thẳng** (Cài đặt → Nhận): *Tắt* (lần nào cũng hỏi) · **Chỉ danh bạ** (mặc định: máy trong danh bạ và máy LocalSend đã "Luôn nhận" thì tự nhận, máy lạ phải hỏi) · *Mọi người cùng mạng*. Máy LocalSend tin cậy có nút **Quên**. Tắt "**Hiện với máy LocalSend**" nếu không muốn máy LocalSend thấy máy này.
- **Link**: bấm link người khác gửi (hoặc dán vào ô trên Trang chính).
- Đứt mạng / tắt máy giữa chừng (giữa hai máy Toss): mở lại Toss, Toss chạy tiếp từ chỗ dở. Gửi cho máy trong danh bạ đang offline: file chờ sẵn, máy đó mở Toss là tự đi. Chuyển qua LocalSend không nối tiếp được: bấm **Gửi lại** trong danh sách chuyển.

### Máy luôn bật, không có người ngồi (vd. máy ở studio)

Không cần cài thêm gì: Toss mở cùng máy và nằm ẩn ở khay hệ thống (bản 0.8 tự bật "Mở cùng máy" cho mọi máy).

1. Cài `Toss-setup.exe` **cho tất cả người dùng** (tự thêm luật Windows Firewall cho `Toss.exe`).
2. Windows: tắt Sleep khi cắm điện (Settings → System → Power → *Never*).
3. Bật tự đăng nhập Windows: `Win + R` → `netplwiz` → bỏ tick *Users must enter a user name and password…* (không thấy ô tick: Settings → Accounts → Sign-in options → tắt *only allow Windows Hello sign-in*). Toss chỉ chạy sau khi tài khoản đã đăng nhập. BIOS: **Restore on AC Power Loss = Power On**.
4. Muốn điều khiển máy này từ xa (tạo link trong folder của nó, bảo nó tự cập nhật): trên máy đó **Cài đặt → Chẩn đoán & nâng cao → Link điều khiển**; dán link vào máy của bạn ở **Chẩn đoán & nâng cao → Điều khiển máy khác**. Muốn tạo link cho folder con bên trong một kho lớn: thêm kho đó ở **Folder cố định**.

Windows Update tự khởi động lại máy: máy bật lại → tự đăng nhập → Toss tự chạy, online lại sau vài chục giây; máy đang tải sẽ tự nối tiếp.

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

- Cài Toss bằng **"cho tất cả người dùng"** (có UAC) thì trình cài tự thêm luật cho chương trình `Toss.exe`, chiều vào: `Toss` (**UDP**, mọi cổng — chuyển file Toss) và `Toss LocalSend` (**TCP + UDP 53317–53327** — để điện thoại / máy LocalSend thấy và gửi vào máy này).
- Cài "chỉ mình tôi" không có quyền Administrator, máy nâng cấp từ bản cũ, hoặc ai đó đã xoá luật: mở Toss → **Cài đặt → Chẩn đoán & nâng cao → Kết nối**, dòng **Firewall**. Nếu hiện "Thiếu", bấm **Cho phép qua firewall** (một lần hỏi quyền Administrator; tạo cả hai luật).
- Tự kiểm tra: PowerShell `Get-NetFirewallRule -DisplayName Toss | Get-NetFirewallPortFilter` phải thấy `Protocol : UDP`; `Get-NetFirewallRule -DisplayName 'Toss LocalSend' | Get-NetFirewallPortFilter` thấy TCP và UDP `53317-53327`.
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

### Đọc bảng Kết nối (Cài đặt → Chẩn đoán & nâng cao → Kết nối)

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

Một số mạng (NAT đối xứng, CGNAT, mạng khách sạn/công ty chặn UDP) không bao giờ nối trực tiếp được. Khi đó thay vì relay công cộng ở xa, studio có thể thuê một VPS nhỏ trong nước làm **relay riêng**: nhanh hơn nhiều, chi phí khoảng vài trăm nghìn đồng mỗi tháng, và Toss vẫn tự quay về relay công cộng nếu relay riêng sập. Cách làm 6 bước, chi phí và theo dõi: [`hub/relay/README.md`](../hub/relay/README.md). Cuối cùng dán địa chỉ vào **Cài đặt → Chẩn đoán & nâng cao → Kết nối → Relay riêng** trên **cả máy ở studio và các máy ở xa**.

### Checklist nhanh khi thẻ truyền hiện "Relay"

1. **Đợi 10-20 giây**: Toss thử lại địa chỉ sau 10 giây và nhiều khi tự nâng lên Trực tiếp. Không cần dừng việc đang tải.
2. **Máy ở studio đang bật và Toss chạy ở chế độ máy ở studio?** Cài đặt → Chẩn đoán & nâng cao → Kết nối → *Cổng UDP* phải là **41820**.
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
| "Host offline" / không kết nối được | Máy ở studio đang tắt, đang restart, hoặc kẹt ở màn hình đăng nhập. Kiểm tra auto-login và **Mở cùng máy** (Cài đặt → Máy này). Toss tự thử lại, cứ để cửa sổ mở. |
| Không thấy máy cùng mạng trong **Gần đây** | Hai máy khác mạng (Wi-Fi khách / AP isolation / VLAN), firewall thiếu luật `Toss LocalSend` (mục 3), hoặc Mac chưa cho quyền **Mạng cục bộ**. Bấm làm mới; vẫn không thấy thì dùng Danh bạ (qua Internet). |
| Điện thoại LocalSend báo "yêu cầu mã PIN" / "từ chối" | Tắt PIN trong LocalSend của máy kia; "từ chối" = người bên kia bấm Từ chối hoặc không trả lời trong 60 giây. |
| Cổng 53317 bị chiếm | Toss tự dùng 53318–53327 (máy LocalSend vẫn tìm thấy). |
| Báo "Busy" / xếp hàng | Máy ở studio giới hạn số job chạy cùng lúc (mặc định 2). Toss tự thử lại mỗi 30 giây trong tối đa 1 giờ. |
| Tốc độ chậm | Xem nhãn kết nối trong tiến trình: **Direct** là nhanh, **Relay** là đi vòng nên chậm. Làm theo "Checklist nhanh khi thẻ truyền hiện Relay" ở mục 3 (firewall, UPnP / forward UDP 41820, cắm LAN, tắt VPN, relay riêng). Đôi khi sau vài giây kết nối tự nâng từ Relay lên Direct. |
| Bấm link mà Toss không mở | Chưa cài hoặc chưa mở Toss lần nào. Cài, mở một lần, bấm lại link. Trình duyệt hỏi "Open Toss?" thì chọn Open. Dùng nút **Copy ticket** rồi dán vào Toss nếu vẫn không được. |
| "Link hết hạn" / "Đã thu hồi" | Nhờ studio tạo link mới. |
| Mac nói Toss "đã hỏng" / "không thể mở" | Chuột phải → Open (xem mục Mac). Nếu vẫn lỗi: `xattr -dr com.apple.quarantine /Applications/Toss.app`. |
| Tải xong file báo lỗi hash | Toss tự tải lại file đó một lần. Nếu vẫn lỗi, kiểm tra ổ đĩa đầy hoặc phần mềm diệt virus đang khoá file. |
| Windows Update khởi động lại máy ở studio | Bình thường: máy tự đăng nhập, Task chạy Toss, online lại sau ~1 phút. Máy ở xa đang tải sẽ tự nối tiếp. |
