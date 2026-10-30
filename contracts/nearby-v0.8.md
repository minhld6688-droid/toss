# v0.8 contract — "Gần là thấy, thả là xong" (LocalSend core + nearby + simplification)

Owner approved (30/10/2026) the plan `claude/toss-plan-v0.7-localsend.md`, merged into ONE release **0.8.0**:
- autostart ON for every machine, forced once by the 0.8 update;
- default receive policy "Chỉ danh bạ" (contacts auto-accept, nearby strangers ask);
- everything in v0.7 + v0.8 lists, done thoroughly.

Design rule: keep the better tech. Toss↔Toss transfer stays iroh (faster, resumable, WAN).
LocalSend's proven Rust core (`third_party/localsend-core`, Apache-2.0, vendored UNMODIFIED) provides
LAN discovery + interop with every LocalSend app (iOS/Android/Win/Mac/Linux) + browser share.

## 0. Ownership (each WS works in its own clone/branch; coordinator merges)

| WS | Owns | Must not edit |
|---|---|---|
| G core | `crates/toss-core/**`, `crates/toss-cli/**`, `contracts/wire.md` (append v0.8 section) | `apps/**`, `crates/toss-ls/**`, `third_party/**` |
| H ls | `crates/toss-ls/**` | everything else (`third_party/**` read-only) |
| I shell | `apps/desktop/src-tauri/**`, `installer/**`, `.github/**`, `docs/**`, `README.md` | `apps/desktop/src/**`, `crates/**` |
| J ui | `apps/desktop/src/**`, `apps/desktop/*.json|*.ts|*.js|index.html|screenshots/**`, `web/**` | `crates/**`, `src-tauri/**` |

`contracts/ipc.ts`: J appends the v0.8 block (§4) verbatim; I implements it. Root `Cargo.toml`/`Cargo.lock`: add-only.
Code against the signatures below; your crate/app must build on its own. Stubs allowed only inside your own tree.
Every user-visible string is Vietnamese, short, no jargon.

## 1. Core (WS-G) — Toss↔Toss nearby over iroh

### 1.1 Nearby discovery (mDNS, already running)
- Advertise mDNS user data `n=<device name>;p=<platform>;v=<version>` (≤ 245 bytes, name truncated) via the
  endpoint's address-lookup user data; refresh when the name changes (`Node::set_device_name`).
- Subscribe to `MdnsAddressLookup::subscribe()`; keep a nearby store: Discovered → upsert, Expired → remove;
  also drop entries not re-seen for 3 min. A contact seen nearby is `online` immediately (no 60 s lag).
- When `net.mdns` is off the store stays empty (shell falls back to the LocalSend list, §3.3).

```rust
pub struct NearbyPeer {            // Serialize
    pub id: String,                // endpoint id hex (same format as Contact.id)
    pub name: String, pub platform: String, pub version: String,
    pub addrs: Vec<String>,        // direct socket addrs seen via mDNS ("192.168.1.5:51234")
    pub last_seen: u64,
    pub contact: bool,             // already in contacts
}
impl Node {
    pub fn nearby(&self) -> Vec<NearbyPeer>;               // excludes self
    pub fn set_device_name(&self, name: &str);             // updates Hello/Pair name + mDNS user data
    pub async fn send_to_peer(&self, id: &str, paths: Vec<PathBuf>) -> Result<crate::Transfer>; // contact OR nearby stranger
    pub async fn send_text(&self, id: &str, text: &str) -> Result<()>;   // contact or nearby; ≤ 60 KiB
    pub fn set_receive_policy(&self, p: ReceivePolicy);    // persisted by the shell, pushed on start
    pub fn answer_push(&self, push_id: &str, accept: bool, always: bool) -> Result<()>; // REPLACES the 2-arg form
    pub async fn resend(&self, transfer_id: &TransferId) -> Result<crate::Transfer>;     // same paths, same target (push or put)
}
#[derive(Serialize, Deserialize, Clone, Copy, Default)] #[serde(rename_all = "snake_case")]
pub enum ReceivePolicy { Off, #[default] Contacts, Everyone }
```
`send_to_contact` keeps working (delegates). Events (TransferEvent, serde tag "type"):
`NearbyChanged` → `{type:"nearby_changed"}`; `TextIncoming{from_id, from_name, text, at}` → `"text_incoming"`.
`IncomingPush` gains `stranger: bool` (serde default false).

### 1.2 Wire (append-only, old peers unaffected) — document in `contracts/wire.md`
- `PeerReq::PushFrom { push_id, job, files, bytes, name, platform, version }` — new last variant; sent ONLY to
  non-contacts (contacts still get `Push`, so 0.6 contacts keep working).
- `PeerReq::Text { name, text }` — new variant, ≤ 60 KiB.
- `PeerResp::PushAcceptedPaired { name, platform, version }`, `PeerResp::TextOk` — new last variants, only sent
  in reply to the new requests.
- Receiver, request from a non-contact: allowed only if its id is in the nearby store (seen on this LAN);
  otherwise `Denied` as today.
- Policy: `Off` → every push asks (contacts too). `Contacts` → contact with `auto_accept` auto, others ask.
  `Everyone` → nearby strangers auto-accepted too (not added to contacts).
- Ask = `PushQueued` + `PushIncoming{stranger}`; `answer_push(accept, always)`: `always` on a stranger adds
  it as contact (name/platform from `PushFrom`, auto_accept=true) and replies `PushAcceptedPaired`; the
  sender then adds the receiver as contact too. `always` on a contact sets its `auto_accept=true`.
- Text: `Off`/`Contacts`: contacts → `TextIncoming`; strangers nearby → also `TextIncoming` (display only).
- Stranger pushes land in `<inbox>/<sender name>/<job>/` like contacts.

### 1.3 Other core items
- Receiver-side `done` for pushes carries `dest` = the job folder (shell uses it for notification + "Mở folder").
- Tickets may point at a single FILE (manifest of one entry; `get` saves `<dest>/<job>/<file>`); quick share of a
  file uses the parent as root and the file as path. Old clients: verify a 0.6 client can still download it, or
  if not, document that file links need ≥ 0.8.
- Every error string that can reach the UI is Vietnamese (e.g. `peer.rs:314`).
- Fold the v0.5 leftovers only inside core (no API removals the shell still uses).
- Tests: nearby store, stranger ask/always, policy matrix, text, resend, file ticket; existing tests stay green.

## 2. LocalSend bridge (WS-H) — `crates/toss-ls`

Built on `third_party/localsend-core` (features `discovery`, `http`). Spike already in `src/lib.rs` (replace freely).

```rust
pub struct LsConfig {
    pub alias: String,           // device name
    pub data_dir: PathBuf,       // <toss data_dir>; cert persisted at ls/cert.pem + ls/key.pem
    pub dest_dir: PathBuf,       // inbox root (same as Toss inbox)
    pub port: u16,               // preferred 53317; if busy try 53318..=53327, then 0
}
pub struct LsBridge { .. }       // Clone (Arc inside)
impl LsBridge {
    pub async fn start(cfg: LsConfig) -> Result<(LsBridge, mpsc::Receiver<LsEvent>)>;
    pub fn port(&self) -> u16;  pub fn fingerprint(&self) -> String;
    pub fn devices(&self) -> Vec<LsDevice>;     pub async fn rescan(&self);   // multicast burst + /24 scan
    pub async fn set_alias(&self, alias: &str); pub fn set_dest(&self, dir: PathBuf);
    pub fn answer(&self, req_id: &str, accept: bool);   // for LsEvent::Incoming
    pub async fn send(&self, fingerprint: &str, items: Vec<PathBuf>) -> Result<String>; // transfer id; folders keep structure
    pub async fn send_text(&self, fingerprint: &str, text: &str) -> Result<()>;
    pub fn cancel(&self, transfer_id: &str);
    pub async fn web_share_start(&self, items: Vec<PathBuf>) -> Result<WebShare>;  // browser download on LAN, auto-accept
    pub async fn web_share_stop(&self);
    pub async fn stop(&self);
}
#[derive(Serialize, Clone)] pub struct LsDevice {
    pub fingerprint: String, pub alias: String, pub model: Option<String>, pub kind: String /*mobile|desktop|web|headless|server*/,
    pub ip: String, pub port: u16, pub https: bool, pub is_toss: bool /* model == "Toss" */, pub last_seen: u64,
}
#[derive(Serialize, Clone)] pub struct WebShare { pub urls: Vec<String> /* http://<lan-ip>:<port> */ }
pub enum LsEvent {
    DevicesChanged,
    Incoming { req_id: String, fingerprint: String /* mTLS-verified when present */, alias: String, model: Option<String>,
               ip: String, files: u32, bytes: u64, names: Vec<String> /* ≤5 */, text: Option<String> },
    Started  { id: String, outgoing: bool, peer: String, job: String, files: u32, bytes: u64 },
    Progress { id: String, bytes_done: u64, bytes_total: u64, files_done: u32, files_total: u32, speed_bps: u64 },
    Done     { id: String, dest: Option<PathBuf>, files: u32, bytes: u64 },
    Error    { id: String, msg: String /* Vietnamese */ },
    Text     { fingerprint: String, alias: String, text: String },   // received text message (no file saved)
    Port     { port: u16, fallback: bool },                          // fallback=true when 53317 was busy
}
```
- Server: HTTPS (LocalSend default) with a persisted self-signed cert (`localsend::crypto::cert`); announce
  `device_model = "Toss"`, `device_type = desktop`. Multicast discovery + answer announcements; `rescan` = burst +
  subnet scan. Restart the server by itself on `ListenerFailed`. Self is never listed.
- Receive: every prepare-upload emits `Incoming`; the shell answers (`answer`) — no answer in 60 s = decline.
  A single text/plain file with `preview` and no real content = text message → `Text` event, nothing saved.
  Files go to `<dest>/<sanitized alias>/<relative path>`, path-checked (reject `..`/absolute, sanitize every
  component with `localsend::util::filename`), no clobber (`name (2).ext`), checksums verified.
- Send: try HTTPS with fingerprint pinning, fall back to HTTP for http peers; 401 (PIN) → Error "Máy kia yêu cầu
  mã PIN — tắt PIN trong LocalSend của máy đó"; 403 → "Máy kia đã từ chối"; 409 → "Máy kia đang bận". Uploads
  2 files in parallel, progress ≤ 4 events/s, cancel stops the session (`/cancel`).
- Web share: LocalSend web mode (download page), auto-accept, URLs for every private LAN IPv4.
- Tests: roundtrip folder (structure + names), text, decline, traversal names rejected, cancel, port fallback,
  discovery between two bridges in-process (loopback multicast may not work in CI → mark `#[ignore]` if so,
  but the subnet/known-address path must be tested).

## 3. Shell (WS-I) — Tauri
### 3.1 Startup
- Start node + `LsBridge` (alias = device name, dest = inbox). Forward LsEvents and core events to the UI.
- Settings: `receive_policy` (default "contacts"), `ls_enabled` (default true), `ls_trusted: Vec<{fingerprint, alias}>`,
  `migrated_v08: bool`. Migration on first 0.8 start: `autostart = true` (plugin enable, with `--hidden`), set
  `migrated_v08`. New installs: autostart true by default; autostart launch starts hidden in tray.
- Windows firewall rule: add TCP+UDP 53317–53327 for Toss.exe to the existing rule logic (installer + `add_firewall_rule`).
- macOS: trigger the Local Network prompt early (wizard step calls a command that sends one multicast packet).
- Single instance: a second launch forwards its args (files/folders, `--share`, `--send`) to the running one.
### 3.2 Policy applied to LocalSend incoming
`everyone` → accept; `contacts` → accept if fingerprint ∈ `ls_trusted`, else ask; `off` → ask. Answer from UI
`answer_incoming({id, accept, always})`: `always` adds to `ls_trusted`.
### 3.3 Unified nearby list (`list_nearby`)
= core `nearby()` (kind "toss") ∪ LocalSend devices. A LocalSend device with `is_toss` whose IP matches a core
nearby peer's addr IP is hidden (same machine). `is_toss` devices without a match are shown as kind "toss_lan"
(sent over the LocalSend path — works when mDNS is blocked).
### 3.4 Receive UX
On receive done (core host-side push or LsEvent::Done incoming): OS notification "Đã nhận <job> từ <name>"
(click → open folder), add to received history (`<data_dir>/received.json`, last 30), emit `received`.
Incoming ask → show + focus window + notification "<name> muốn gửi <job>".
### 3.5 Other
- File support: `pick_files`; right-click menu for files too (Windows `*\shell`, macOS `public.item`);
  Windows **Send to → Toss** shortcut in `%APPDATA%\Microsoft\Windows\SendTo` (toggle with the context-menu setting).
- Transfers list = core transfers ∪ LocalSend transfers (`TransferState` with `contact: null`, `path: "direct"`).
- Clean-up: fold `core_v05.rs`, drop dead host-era code paths no longer reachable from the UI, delete
  `installer/host-setup/windows-host.ps1` if nothing references it; version 0.8.0 everywhere.

## 4. IPC additions (J appends to `contracts/ipc.ts`; I implements)
```ts
// ===========================================================================
// ---- v0.8 nearby + LocalSend (contracts/nearby-v0.8.md) -------------------
export type ReceivePolicy = "off" | "contacts" | "everyone";
export type NearbyKind = "toss" | "toss_lan" | "localsend";
export interface Nearby {
  key: string;                 // "t:<endpoint hex>" | "l:<fingerprint>"
  kind: NearbyKind;
  name: string;
  model: string | null;        // "iPhone", "Windows", "Toss"…
  platform: string;            // "macos"|"windows"|"linux"|"ios"|"android"|"web"|""
  contact_id: string | null;   // set when this device is already a contact (kind toss)
  trusted: boolean;            // contact with auto_accept, or LocalSend device in ls_trusted
}
export interface Incoming {    // unified ask (replaces IncomingPush in the UI)
  id: string;                  // push_id or LocalSend req_id
  source: "toss" | "localsend";
  from_key: string;            // Nearby.key / contact id
  from_name: string;
  stranger: boolean;
  job: string; files: number; bytes: number; names: string[]; at: number;
}
export interface Received { id: string; from_name: string; job: string; files: number; bytes: number; dest: string; at: number }
export interface TextMsg { from_name: string; text: string; at: number }
export interface WebShare { urls: string[]; qr_svg: string }
// AppState gains: receive_policy: ReceivePolicy; ls_enabled: boolean; ls_port: number | null; ls_trusted: { fingerprint: string; alias: string }[]

export interface CommandsV08 {
  list_nearby(): Promise<Nearby[]>;
  rescan_nearby(): Promise<void>;
  send_to(args: { key: string; paths: string[] }): Promise<string>;     // key = Nearby.key or contact id; transfer id
  send_text(args: { key: string; text: string }): Promise<void>;
  list_incoming(): Promise<Incoming[]>;
  answer_incoming(args: { id: string; accept: boolean; always: boolean }): Promise<void>;
  set_receive_policy(args: { policy: ReceivePolicy }): Promise<void>;
  set_ls_enabled(args: { on: boolean }): Promise<void>;
  forget_trusted(args: { fingerprint: string }): Promise<void>;
  list_received(): Promise<Received[]>;
  resend(args: { id: string }): Promise<string>;
  pick_files(args: { title: string }): Promise<string[]>;
  web_share_start(args: { paths: string[] }): Promise<WebShare>;
  web_share_stop(): Promise<void>;
  request_local_network(): Promise<void>;   // macOS: triggers the OS prompt; else no-op
}
export type EventsV08 = {
  "nearby:changed": Record<string, never>;
  "incoming": Incoming;
  "incoming:closed": { id: string };        // answered elsewhere / timed out
  "received": Received;
  "text": TextMsg;
  "ls:port": { port: number; fallback: boolean };
};
```
Legacy commands stay (UI may stop using them). `send_to_contact`/`answer_push`/`list_incoming_pushes` remain as aliases.

## 5. UI (WS-J) — target flow
- **Wizard: 1 screen** — name (prefilled), receive folder, "Mở cùng máy" ON (pre-checked), button "Bắt đầu".
  Firewall/Local Network handled inline afterwards only if needed (non-blocking banner).
- **Home**: drop zone + "Gần đây" (nearby, live, refresh button) + "Danh bạ" (contacts, online dot) +
  one button "Mời máy ở xa" (invite). One single paste field that accepts toss1…/tossinv1…/short links.
  Empty nearby state explains in one line what to do.
- **"Gửi tới ai?"**: sections Gần đây · Danh bạ · "Tạo link" (only for ONE file or folder) ·
  "Chia sẻ qua trình duyệt" (LAN). Also "Gửi chữ" (text box) when opened from the text button / paste.
- **Incoming sheet**: "<name> muốn gửi <job> · N file · size" — buttons **Nhận** · **Luôn nhận từ máy này** · Từ chối.
  Text message sheet with "Sao chép".
- **Transfers**: includes LocalSend transfers; "Gửi lại" on finished outgoing; "Mở folder"; fixed empty text.
- **Settings: 3 sections** — Máy này (tên, folder nhận, mở cùng máy) · Nhận (ai được gửi thẳng: Tắt / Chỉ danh
  bạ / Mọi người cùng mạng; máy LocalSend tin cậy + Quên; bật/tắt "Hiện với máy LocalSend") · Cập nhật.
  Everything else (Kết nối, Folder cố định, Link, Link điều khiển, Điều khiển máy khác, Xuất log) under one
  collapsed **"Chẩn đoán & nâng cao"**.
- Remove duplicates: one "how it works" text, single `ipc-types.ts` (the contracts copy), unused store fields.
- Mock mode (VITE_MOCK=1) covers every new command/event; screenshots of new screens (light/dark).
