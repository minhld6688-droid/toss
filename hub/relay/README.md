# Relay riêng cho Toss (iroh-relay 1.4.1)

Toss nối **trực tiếp** giữa máy tổng và editor khi có thể. Khi không đục được NAT, dữ liệu phải đi vòng qua một **relay**. Relay công cộng của n0 (mặc định) ở xa Việt Nam và bị giới hạn tốc độ nên chỉ ~1 MB/s. Một relay riêng đặt ở VPS trong nước thì nhanh hơn nhiều, và **Toss vẫn tự dùng relay của n0 làm dự phòng** nếu relay riêng sập, nên không có rủi ro làm hỏng Toss.

Relay chỉ là đường vòng. Cách nhanh nhất vẫn là Direct (mở cổng UDP 41820 trên router studio, xem `docs/INSTALL.md` mục "Tốc độ & kết nối"). Relay riêng là lưới an toàn cho editor nào mà Direct không được.

Bộ này chỉ chạy được trên Ubuntu 22.04 hoặc 24.04 (VPS mới tinh là đẹp nhất).

## Chọn VPS

| Việc | Khuyến nghị |
| --- | --- |
| Nhà cung cấp | **VPS trong nước**: Viettel IDC, VNG Cloud (vServer), Bizfly Cloud. Ping từ Hà Nội và các tỉnh thường dưới 10-30 ms. |
| Cấu hình | 1-2 vCPU, 1-2 GB RAM, ổ 20 GB, Ubuntu 22.04 hoặc 24.04, IP public IPv4 (IPv6 càng tốt) |
| Băng thông | **Cổng mạng từ 100 Mbps trở lên** (100 Mbps ≈ 11 MB/s cho mỗi chiều; nên chọn gói không giới hạn hoặc quota lớn, vì file ảnh nặng) |
| Phương án khác | **Singapore** (DigitalOcean, Vultr, Linode, AWS Lightsail...): ping 30-60 ms từ VN, giá khoảng 5-12 USD/tháng, thường có 1-2 TB quota. Dùng khi nhà cung cấp trong nước chặn cổng hoặc quota thấp. |

Lưu ý khi mua:
- Cổng mạng **và quota/tháng** quan trọng hơn CPU. Relay gần như không tốn CPU.
- Nhiều nhà cung cấp có **firewall/security group riêng trên trang quản lý**. Phải mở trên đó: TCP 80, TCP 443, **UDP 7842** (ngoài TCP 22 để SSH). Quên bước này là lỗi phổ biến nhất.
- Nếu máy tổng chỉ có 5-10 editor và đa số nối Direct thì gói nhỏ nhất là đủ.

## 6 bước cho chủ studio

1. **Mua VPS** (xem bảng trên), chọn Ubuntu 22.04/24.04, ghi lại **IP public** và mật khẩu/SSH key của `root`.
2. **Trỏ DNS**: tại nơi quản lý tên miền, tạo bản ghi **A**: `relay.<tên-miền-của-bạn>` → IP của VPS (ví dụ `relay.studio2007.vn`). Nếu dùng Cloudflare phải để **DNS only (đám mây xám)**, không bật proxy (proxy làm hỏng kết nối relay). Đợi vài phút cho DNS cập nhật; kiểm tra bằng `ping relay.<tên-miền>` thấy đúng IP.
3. **Chạy `install.sh`** trên VPS:
   ```bash
   ssh root@<ip-vps>
   apt-get update && apt-get install -y git
   git clone https://github.com/minhld6688-droid/toss.git
   cd toss/hub/relay
   sudo ./install.sh relay.<tên-miền> email-cua-ban@example.com
   ```
   Script tự: cài Rust và build `iroh-relay` 1.4.1 (5-15 phút, VPS ít RAM sẽ tự tạo swap tạm), tạo user `toss-relay`, cấp chứng chỉ HTTPS Let's Encrypt (relay tự gia hạn, không cần certbot), cài dịch vụ systemd `toss-relay` tự chạy lại khi VPS khởi động, mở UFW: `22/tcp`, `80/tcp`, `443/tcp`, `7842/udp`.
   Muốn bỏ qua build: chạy workflow **relay-build** trên GitHub (Actions → relay-build → Run workflow, tick *publish*), rồi chạy `sudo TOSS_RELAY_BIN_URL=<link asset> TOSS_RELAY_BIN_SHA256=<mã trong file .sha256> ./install.sh ...`.
4. **Kiểm tra**: mở trình duyệt vào `https://relay.<tên-miền>/`. Phải thấy trang "Iroh Relay", ổ khoá hợp lệ. Hoặc chạy `./check.sh relay.<tên-miền>` (trên VPS hoặc trên Mac), tất cả dòng phải `[ok]`.
5. **Dán vào Toss** trên **máy tổng và tất cả editor**: *Cài đặt → Kết nối → Nâng cao → Relay riêng*, dán `https://relay.<tên-miền>` rồi lưu (Toss tự khởi động lại kết nối khi không có job đang chạy). Với link job tạo **sau** bước này, relay riêng được ghi trong link.
6. **Thử**: trên máy editor bấm **Thử nối tới máy tổng** (cùng mục Kết nối). Kết quả mong muốn: *Trực tiếp* (tốt nhất) hoặc *Relay* với RTT thấp (vài chục ms). Nếu vẫn chậm thì xem mục "Xử lý sự cố" bên dưới.

## Chi phí (ước tính, kiểm tra bảng giá hiện tại của nhà cung cấp)

| Khoản | Chi phí |
| --- | --- |
| VPS trong nước 1-2 vCPU, 1-2 GB RAM | khoảng 100.000-350.000đ/tháng |
| VPS Singapore tương đương | khoảng 5-12 USD/tháng |
| Tên miền | 0đ nếu dùng subdomain của tên miền studio đã có; mua mới khoảng 250.000-700.000đ/năm |
| Chứng chỉ HTTPS | 0đ (Let's Encrypt, tự gia hạn) |
| Phần mềm | 0đ |

Relay **chỉ chở dữ liệu của những lần không nối Direct được**, nên băng thông thực tế thấp hơn tổng dung lượng job rất nhiều.

## Ai được dùng relay: mở hay chỉ danh sách cho phép

Mặc định relay **mở** (`access = "everyone"`): ai biết địa chỉ cũng dùng được, nhưng có giới hạn mềm để tránh lạm dụng (xem `relay.toml.in`): tối đa **50 MB/s dữ liệu nhận từ mỗi client**, tối đa 50 kết nối mới/giây. Nội dung đi qua relay luôn được Toss mã hoá đầu-cuối nên relay không đọc được file.

Muốn **chỉ máy của bạn dùng được** (tránh người lạ ăn băng thông VPS):

1. Lấy ID đầy đủ (64 ký tự hex) của máy tổng và từng editor: chạy `toss-cli id` trên máy đó, cột đầu tiên (app Toss chỉ hiện ID rút gọn).
2. Trên VPS: `sudo nano /etc/toss-relay/allowed.txt`, mỗi dòng một ID (mẫu: `allowed.txt.example`; chữ sau `#` là ghi chú).
3. `sudo /opt/toss-relay/update-allowed.sh` (tạo lại `relay.toml` và khởi động lại dịch vụ; cấu hình lỗi sẽ tự quay về bản cũ).

Khi `allowed.txt` có ít nhất một ID, **chỉ những ID đó** dùng được relay, các máy khác (kể cả editor mới chưa thêm) tự rơi về relay n0, chậm nhưng không hỏng. Xoá hết ID để mở lại. Relay 1.4.1 chỉ hỗ trợ danh sách ID nằm ngay trong `relay.toml`, nên file `allowed.txt` là nguồn dữ liệu và `update-allowed.sh` ghi nó vào `relay.toml`.

## Theo dõi

| Việc | Lệnh |
| --- | --- |
| Tình trạng nhanh | `/opt/toss-relay/check.sh relay.<tên-miền>` |
| Dịch vụ có chạy không | `systemctl status toss-relay` |
| Log trực tiếp | `journalctl -u toss-relay -f` |
| Số liệu (chỉ nghe ở localhost) | `curl -s http://127.0.0.1:9090/metrics \| grep -E 'relayserver_(accepts\|disconnects\|bytes_sent\|bytes_recv\|bytes_rx_ratelimited_total)'` |
| Đang có bao nhiêu client | `accepts - disconnects` (số kết nối đã nhận trừ số đã ngắt) |
| Cảnh báo khi sập | Đăng ký miễn phí UptimeRobot/BetterStack: HTTPS monitor tới `https://relay.<tên-miền>/healthz` |
| Chứng chỉ | Tự gia hạn. Hạn dùng hiện ở `check.sh`. File nằm ở `/var/lib/toss-relay/certs` |
| Băng thông VPS | Xem biểu đồ trên trang quản lý nhà cung cấp, hoặc `vnstat` nếu bạn cài |

Cổng metrics `9090` chỉ bind `127.0.0.1`, không mở ra ngoài. Đừng mở nó trên firewall.

## Cổng đã dùng

| Cổng | Dùng để | Bắt buộc |
| --- | --- | --- |
| TCP 443 | HTTPS + WebSocket `/relay`, cấp chứng chỉ (ACME TLS-ALPN-01) | có |
| UDP 7842 | QUIC address discovery (client biết IP public của mình, giúp đục NAT) | có (không có thì relay chạy nhưng đục NAT kém hơn) |
| TCP 80 | trang chào, bản HTTP thường | nên mở |
| TCP 22 | SSH quản trị | có |
| TCP 9090 | metrics | không mở ra ngoài |

Bản iroh 1.x dùng QUIC address discovery thay cho STUN, nên **không còn UDP 3478**.

## Cập nhật / gỡ

- **Nâng phiên bản**: phiên bản `iroh-relay` phải cùng dòng với `iroh` mà Toss dùng (hiện 1.4.1). Khi Toss nâng iroh: `sudo IROH_RELAY_VERSION=<mới> ./install.sh relay.<tên-miền> <email>` (giữ nguyên `allowed.txt`).
- **Đổi tên miền / email / mức giới hạn**: sửa `/etc/toss-relay/relay.env` (`RELAY_DOMAIN`, `CONTACT_EMAIL`, `RATE_BPS` tính bằng byte/giây), rồi chạy `update-allowed.sh`.
- **Gỡ hoàn toàn**: `systemctl disable --now toss-relay; rm -rf /etc/systemd/system/toss-relay.service /usr/local/bin/iroh-relay /etc/toss-relay /opt/toss-relay /var/lib/toss-relay; userdel toss-relay`. Trong Toss xoá ô Relay riêng.

## Xử lý sự cố

| Triệu chứng | Cách xử lý |
| --- | --- |
| `install.sh` báo không trả lời `/healthz` | DNS chưa trỏ đúng IP, hoặc firewall của nhà cung cấp chưa mở TCP 443. Sửa rồi `systemctl restart toss-relay`, xem `journalctl -u toss-relay -n 50` |
| Trình duyệt báo chứng chỉ không hợp lệ | Chưa lấy được chứng chỉ (xem log, mục `acme`). Chạy thử với `TOSS_RELAY_STAGING=1` để tránh bị giới hạn của Let's Encrypt, sau đó bỏ biến này đi chạy lại |
| `check.sh` báo `UDP 7842 is not listening` | Dịch vụ chưa chạy hoặc `enable_quic_addr_discovery` bị tắt trong `relay.toml` |
| Toss hiện "Chưa nối được relay" | Sai URL (phải bắt đầu `https://`, không thêm đường dẫn), VPS tắt, hoặc ID của máy này không có trong `allowed.txt` |
| Vẫn chậm dù qua relay riêng | Cổng mạng/quota VPS thấp, hoặc editor ở nhà mạng có đường đi tới VPS tệ. Thử VPS nhà cung cấp khác, hoặc mở UDP 41820 để nối Direct |
