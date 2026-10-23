# v0.5 contract — Danh bạ máy (Blip-style contacts), job resume, Local Network warning

Owner asked (23/10): like Blip remembering people, but by *machine*: pair once,
then drag files onto a named device and they arrive. No server, no email.
Also: transfers must survive an app restart, and the app must say clearly when
macOS blocks it from the local network (root cause of the 1 MB/s relay
transfers seen on 0.4.0/0.4.1: unsigned app, Local Network privacy denied →
`EHOSTUNREACH` on every LAN send; Direct + 53–64 MB/s once unblocked).

Everything stays compatible with 0.4 peers (they just can't be contacts).

## 1. Ownership

| WS | Agent | Owns | Must not edit |
| --- | --- | --- | --- |
| E — core | Opus | `crates/toss-core/**` (new `contacts.rs`, `peer.rs`, `outbox.rs`, `resume.rs`, `lanprobe.rs` …), `crates/toss-cli/**` (`invite`, `pair`, `contacts`, `send` subcommands), tests | `apps/**`, `web/**` |
| F — app | Sonnet | `apps/desktop/**`, `web/**`, `contracts/ipc.ts` (append the v0.5 block exactly as §4) | `crates/**` |

Shared: root `Cargo.toml`/`Cargo.lock` add-only; coordinator merges. F codes
the shell against §3 signatures; until E lands, F may stub them behind a
`#[cfg(any())]`-free shim **in the shell only** (e.g. `contacts_shim.rs`) that
the coordinator deletes at merge — or simply code against the signatures and
keep `npm run build` + mock mode green; the Rust shell must compile at merge.

## 2. Protocol (WS-E)

Identity = iroh `EndpointId` (already persistent in `<data_dir>/key`). A
contact is trusted by endpoint id: QUIC/TLS authenticates `conn.remote_id()`,
so no ticket is needed once paired.

### 2.1 Invite (pairing link)
`Invite { v: u8 = 1, id: [u8;32] /*inviter endpoint*/, name: String /*≤64*/,
platform: String /*"macos"|"windows"|"linux"*/, secret: [u8;16], exp: u64,
relay: String, addrs: Vec<String> /*same ordering rules as tickets*/ }`,
signed by the inviter key like tickets: `{body, sig}` postcard → string
`tossinv1` + lowercase base32. Deep link `toss://i/<invite>`; web link
`https://<page_host>/#<invite>` (page redirects `tossinv1…` to `toss://i/`).
Default validity 7 days; an invite may pair **multiple** devices until it
expires or is revoked (studio use: one invite pinned for all editors), each
pairing is recorded. `<data_dir>/invites.json` keeps issued secrets.

### 2.2 ALPN `toss/peer/1`
Hosts *and* plain clients accept it (every node with ≥ 1 contact or an active
invite; simplest: always). Control messages: postcard, u32-LE length prefix,
max 64 KiB, first bi stream.
```
PeerReq  = Pair { secret, name, platform, version }   // only from strangers holding an invite
         | Hello { name, version }                     // presence ping from a contact
         | Push { push_id: [u8;16], job: String, files: u32, bytes: u64 }
PeerResp = Paired { name, platform, version }
         | Pong { name, version, busy: bool }
         | PushAccepted | PushDeclined { reason } | PushQueued /*waiting for user*/
         | Denied { reason }
```
- Pair: inviter checks secret/exp, adds the dialer (`remote_id`) as contact
  with the given name; dialer adds the inviter (from the invite) — both sides
  emit `ContactsChanged`.
- Anything but `Pair` from a non-contact → `Denied` and close.
- Push: receiver (contact) answers `PushAccepted` immediately when
  `auto_accept`, else `PushQueued` and emits `PushIncoming`; the user answers
  within 10 min (`answer_push`) → the receiver sends `PushAccepted`/
  `PushDeclined` on the same stream; timeout → declined.
- After `PushAccepted` the data phase is **exactly xfer/2 put semantics** on
  the same connection: sender sends `Manifest`, receiver answers `Want` (resume
  against existing `.part`s), sender streams xfer/2 segments, receiver sends
  `Done`. Reuse `xfer.rs`/host put code (no-clobber + numbered names). Files
  land in `<inbox_dir>/<contact name sanitized>/<job>/` (`inbox_dir` default
  `<dest_dir>`, i.e. `~/Toss/Từ Đạt – Mac/<job>`; `job` = dropped folder name,
  or "N file" for loose files).
- Presence: every node pings each contact with `Hello` on start, on network
  change and every 60 s while the app runs (short-lived connection, close code
  0 "hello"); online = Pong within 5 s; also mark online whenever any
  connection with that id is alive. Never log at warn for offline contacts.

### 2.3 Outbox + transfer persistence (job resume)
- `<data_dir>/transfers.json`: every client transfer (get/put via ticket and
  pushes to contacts) with `{id, kind: get|put|push, ticket?, contact?, paths?,
  dest?, job, status, bytes_done, bytes_total, created}`; written on status
  change (atomic tmp+rename).
- On `Node::start*`: transfers that were `running`/`connecting` resume
  automatically; `paused` stay paused (resumable from the UI); `done`/
  `cancelled`/`error` kept for the list (max 50 finished, oldest dropped).
- Pushes to an offline contact stay `connecting` ("Chờ <name> online") and
  start when presence sees the contact online (no 30 s polling of offline
  contacts beyond the 60 s presence ping).

### 2.4 Local Network denial probe (`lanprobe.rs`)
On start and every 60 s (macOS only; other OS → false): send one UDP datagram
from a fresh `std::net::UdpSocket` to `224.0.0.251:5353` (and to the default
gateway's LAN IP port 9 if known); `EHOSTUNREACH`/`ENETUNREACH`/`EPERM` on
*both* while the machine has a private IPv4 → `lan_blocked = true`. Fill
`NetDiag.lan_blocked` and prepend reason "macOS đang chặn Toss truy cập mạng
cục bộ — vào Cài đặt hệ thống → Quyền riêng tư & Bảo mật → Mạng cục bộ và bật
Toss, rồi mở lại Toss". Emit `TransferEvent::LanBlocked { blocked }` on change.

## 3. Core API (WS-E → used by WS-F)

```rust
pub struct Contact {               // Serialize/Deserialize, persisted in <data_dir>/contacts.json
    pub id: String,                // endpoint id, hex (64)
    pub name: String,              // display name (editable locally)
    pub their_name: String,        // name the device reports
    pub platform: String,
    pub added_at: u64,
    pub auto_accept: bool,         // default true
    pub online: bool,              // runtime
    pub last_seen: u64,            // unix s, 0 = never
    pub rtt_ms: Option<u32>,       // runtime, last presence ping
}
pub struct InviteInfo { pub invite: String, pub web_url: String, pub deep_link: String, pub exp: u64, pub name: String }
pub struct IncomingPush { pub push_id: String, pub from_id: String, pub from_name: String, pub job: String, pub files: u32, pub bytes: u64, pub at: u64 }

impl Node {
    pub fn set_inbox_dir(&self, dir: PathBuf);
    pub fn contacts(&self) -> Vec<Contact>;
    pub fn create_invite(&self, days: u32, page_host: &str) -> Result<InviteInfo>;
    pub fn revoke_invites(&self) -> Result<()>;
    pub fn parse_invite(text: &str) -> Result<InviteInfo>;           // assoc fn, tolerant like Ticket::from_str
    pub async fn accept_invite(&self, text: &str) -> Result<Contact>;
    pub fn rename_contact(&self, id: &str, name: &str) -> Result<()>;
    pub fn set_contact_auto_accept(&self, id: &str, on: bool) -> Result<()>;
    pub fn remove_contact(&self, id: &str) -> Result<()>;            // also forgets queued pushes to it
    pub async fn send_to_contact(&self, id: &str, paths: Vec<PathBuf>) -> Result<Transfer>;
    pub fn incoming_pushes(&self) -> Vec<IncomingPush>;
    pub fn answer_push(&self, push_id: &str, accept: bool) -> Result<()>;
    pub async fn refresh_presence(&self);
}
// TransferEvent additions (serde tag "type", snake_case):
//   ContactsChanged {}                                   -> {"type":"contacts_changed"}
//   ContactPresence { id, online, rtt_ms: Option<u32> }   -> {"type":"contact_presence",...}
//   PushIncoming(IncomingPush)                            -> {"type":"push_incoming",...}
//   LanBlocked { blocked: bool }                          -> {"type":"lan_blocked",...}
// TransferState/Started gain `contact: Option<String>` (contact id) for pushes (both sides),
// `op` is "put" for the sender and "get"-like receive shows side "host" as today.
```
CLI: `toss-cli invite [--days 7]`, `toss-cli pair <invite>`, `toss-cli contacts`,
`toss-cli send <contact-id-or-name> PATH...`, and `toss-cli host` auto-accepts
pushes from contacts into `--return`.

## 4. IPC v0.5 (WS-F appends to contracts/ipc.ts)

```ts
export interface Contact { id: string; name: string; their_name: string; platform: string; added_at: number; auto_accept: boolean; online: boolean; last_seen: number; rtt_ms: number | null }
export interface InviteInfo { invite: string; web_url: string; deep_link: string; exp: number; name: string }
export interface IncomingPush { push_id: string; from_id: string; from_name: string; job: string; files: number; bytes: number; at: number }
// commands
list_contacts(): Promise<Contact[]>;
create_invite(args: { days: number }): Promise<InviteInfo>;     // copies web_url to clipboard
revoke_invites(): Promise<void>;
accept_invite(args: { text: string }): Promise<Contact>;
rename_contact(args: { id: string; name: string }): Promise<void>;
set_contact_auto_accept(args: { id: string; on: boolean }): Promise<void>;
remove_contact(args: { id: string }): Promise<void>;
send_to_contact(args: { id: string; paths: string[] }): Promise<string>;   // transfer id
list_incoming_pushes(): Promise<IncomingPush[]>;
answer_push(args: { push_id: string; accept: boolean }): Promise<void>;
open_local_network_settings(): Promise<void>;   // macOS: open "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork"; else no-op
// events: "contacts:changed" {}, "contact:presence" {id, online, rtt_ms}, "push:incoming" IncomingPush, "net:lan_blocked" {blocked}
// deep-link event gains invites: { ticket } for toss://t/…, new "invite" { invite } for toss://i/… or a pasted tossinv1…
// TransferState gains contact: string | null; NetDiag gains lan_blocked: boolean
```

## 5. UI (WS-F) — keep the 0.3.1 design language (Fraunces + Bricolage, own icons, aura, motion, reduced-motion, light/dark, 400×600)

- **Home → "Danh bạ"** section on top: avatar chips/rows per contact (initials,
  platform glyph, green/grey presence dot, RTT when online), "+ Thêm máy"
  button. Each row is a **drop target**: dragging files/folders over the
  window highlights rows; dropping on a row → `send_to_contact`, comet
  animation to the Chuyển file tab (existing pattern). Click row → sheet:
  rename, "Tự nhận file" toggle, "Xoá khỏi danh bạ", last seen.
- **Thêm máy** sheet: tab "Mời" (create_invite → QR + link + copy, expiry,
  "Huỷ mọi lời mời") and tab "Nhập lời mời" (paste field; also auto-opens on
  `invite` deep-link event with a confirm "Kết nối với <name>?").
- **Incoming push** (not auto-accepted): sheet "<name> muốn gửi <job> · N file ·
  X GB" with Nhận / Từ chối; queued list if several.
- **Transfers**: pushes show "Gửi tới <name>" / "Nhận từ <name>"; queued ones
  "Chờ <name> online". Restored transfers after restart appear (paused ones
  with a Tiếp tục button).
- **LAN blocked banner** (Home + Transfers + Kết nối section): when
  `lan_blocked`, a prominent card "macOS đang chặn Toss trong mạng nội bộ —
  chuyển file sẽ rất chậm" + button "Mở cài đặt Mạng cục bộ"
  (`open_local_network_settings`) + one-line steps (bật Toss, mở lại app).
- Old drop behaviour with a put-capable ticket stays.
- web/index.html: fragment starting `tossinv1` → redirect `toss://i/<invite>`
  and show "Lời mời kết nối từ <name>" (decode name client-side is optional;
  generic text is fine) with download buttons fallback as today.
- Mock IPC implements everything (3 contacts: one online, one offline, one
  with an incoming push; `?lan=blocked`); screenshots `12-contacts*.png`,
  `13-invite*.png`, `14-push-incoming.png`, `15-lan-blocked.png` (+ dark).

## 6. Acceptance
- `cargo test --workspace`, clippy -D warnings, `npm run build`, desktop
  `cargo test` green.
- Core tests (`tests/contacts.rs`): invite round-trip + signature; pair two
  loopback nodes → both have each other; non-contact `Push`/`Hello` denied;
  push auto-accept lands in `<inbox>/<name>/<job>` byte-exact incl. a split
  file; push with auto_accept=false waits, `answer_push(false)` declines;
  push to offline contact queues and starts when it comes online; restart a
  node mid-get → transfer restored from transfers.json and resumes to done;
  `lan_blocked` false on Linux.
