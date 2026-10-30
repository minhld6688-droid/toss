# Wire contract — ticket format, URL shape, ALPN protocols

## 1. Ticket

A ticket is a postcard-serialized `TicketBody` plus an Ed25519 signature by the
host's iroh secret key (the same key that is the host's `EndpointId`).

```rust
// crates/toss-core/src/ticket.rs
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TicketBody {
    pub v: u8,                 // = 2
    pub id: [u8; 16],          // random, identifies the job; hex is the job id shown in UIs
    pub host: [u8; 32],        // host EndpointId (public key)
    pub job: String,           // human name, <= 80 chars, used as folder name (sanitized)
    pub root: String,          // host root alias, e.g. "RAW"
    pub path: String,          // path under root, '/'-separated, no '..', may be ""
    pub ret: String,           // path under the host's return root where put lands; "" = <job>
    pub get: bool,             // client may download root/path
    pub put: bool,             // client may upload into return root/ret
    pub admin: bool,           // control-channel access (see §4); get/put are false when true
    pub exp: u64,              // unix seconds; 0 = never
    // --- v2 (appended; postcard field order is the wire order) ---
    pub relay: String,         // host's home relay URL when signed, e.g. "https://use1-1.relay.n0.iroh.link./"; "" = unknown
    pub addrs: Vec<String>,    // host's direct socket addrs as text ("1.2.3.4:5678", "[2001:db8::1]:5678"),
                               // best first (public v4, public v6, LAN), loopback/link-local omitted; 0..=8
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Ticket {
    pub body: TicketBody,
    pub sig: [u8; 64],         // ed25519 over postcard(body)
}
```

String form: `toss1` + base32 (RFC 4648, no padding, **lowercase**) of
`postcard(Ticket)`. Length: ~230 chars without addresses, typically ~350–550
with a relay URL and a few direct addresses (≤ ~900 worst case). `Ticket::from_str` accepts
upper or lower case, trims whitespace, accepts the string with or without the
`toss://t/` or `https://<any host>/#` prefix. Only `v == 2` is accepted: the
first byte of `postcard(Ticket)` is `v`, and a v1 ticket is refused with
`TicketError::BadVersion(1)` ("ticket format v1 is not supported … ask the host
for a new link") instead of a decode error.

`relay`/`addrs` are covered by the signature like every other field. They are a
dialing hint, not an authority: the QUIC/TLS handshake authenticates the host
key, so stale or wrong addresses can only make a dial fail, never reach another
host. They are filled from the live endpoint (`Endpoint::addr()`: home relay +
direct addresses) whenever a ticket is signed — host-local `create_ticket` /
`create_admin_ticket`, ctl `CreateTicket`, and the re-sign in `job_ticket` /
ctl `JobTicket` (so a re-signed ticket carries the host's *current* addresses
and its string differs from the original if they changed; id, job and
permissions are identical). The offline CLI path (`toss-cli ticket` while
`toss-cli host` runs) reads the last known values that the running host keeps
in `<data_dir>/cli-host.json`. Clients dial `EndpointAddr { id: host, relay,
addrs }` (see §5). Tickets reveal the host's IP addresses to their holders
(they are bearer secrets anyway); pkarr publishing stays relay-only.

Verification (host side, `Ticket::verify(&self, host_pk)`):
1. `body.host == host_pk` and signature valid over `postcard(body)`.
2. `exp == 0 || now < exp`.
3. `id` not in the host's revocation list (`<data_dir>/revoked.json`).
4. `root` exists in the host's roots; `path` and `ret` contain no `..`, no
   absolute segments, no NUL; resolved path stays inside the root after
   canonicalization (symlinks are resolved and must stay inside).

## 2. URL shapes

- Deep link: `toss://t/<ticket>` — registered by the desktop app on both OSes.
- Web link: `https://<page-host>/#<ticket>` — the page never sees the ticket
  (fragment). It redirects to `toss://t/<ticket>`; if the app does not open
  within 1500 ms it shows download buttons. `web/` owns the page.
- QR code encodes the web link.

## 3. ALPN `toss/xfer/1` — transfer

All control messages are postcard-encoded, prefixed by a u32 little-endian
length, max 4 MiB. File bytes are raw.

Connection = one iroh connection from client to host. Stream 0 (first bi
stream opened by the client) is the control stream.

```rust
pub enum ClientHello { v1 { ticket: Ticket, device: String, op: Op, app_version: String } }
pub enum Op { Get, Put }
pub enum HostHello { Ok { job: String }, Denied { reason: DenyReason } }
pub enum DenyReason { BadSignature, Expired, Revoked, UnknownRoot, BadPath, NotAllowed, Busy, Other(String) }

pub struct FileEntry { pub rel: String, pub size: u64, pub hash: [u8; 32] } // rel uses '/'
pub struct Manifest { pub files: Vec<FileEntry>, pub total: u64 }
pub struct Want { pub items: Vec<WantItem> }           // files the receiver still needs
pub struct WantItem { pub index: u32, pub offset: u64 } // resume from offset
pub struct Done { pub files: u32, pub bytes: u64, pub ok: bool, pub msg: String }
```

### Get (host → client)
1. client: `ClientHello{op: Get}` on the control stream.
2. host: `HostHello`. On `Denied` the host closes the connection.
3. host: `Manifest` (walks `root/path` recursively; hidden files and
   `.part` files skipped; hashes blake3 of each file; symlinks skipped).
4. client: compares with `dest/<job>/`: a file whose size and blake3 match is
   skipped; a `<rel>.part` with size < target resumes from its size;
   sends `Want`.
5. For each `WantItem`, the **host opens a uni stream** carrying:
   u32 LE `index`, u64 LE `offset`, then the file bytes from `offset` to end.
   Up to `max_parallel` (default 4) uni streams in flight; order free.
   Client writes to `<rel>.part`, on completion verifies blake3 == manifest
   hash, renames to `<rel>`; mismatch → deletes `.part` and re-requests once
   on a fresh connection, then fails that file.
6. client: `Done` on the control stream; host logs it; both close.

### Put (client → host)
Mirror image: client sends `Manifest` of its selected paths (rel paths are
relative to the selection's parent; a dropped folder keeps its name as the
first segment). Host answers `Want` after checking `return_root/<ret>/`.
Client opens uni streams per `WantItem` with the same header. Host writes
`.part` files, verifies, renames. **Host never overwrites a complete file**:
if `<rel>` exists with a different hash, the incoming file is written as
`<stem> (2).<ext>`, `(3)`, …; the `Done` message from the host (sent on the
control stream after all files) reports the renames in `msg`.

### Resume
A transfer id is `hex(ticket.id)`. The client persists nothing beyond `.part`
files; a retry is simply a new connection with the same ticket — the `Want`
exchange rebuilds the plan.

### Limits
- A host runs at most `max_parallel_jobs` (default 2) connections; extra
  clients receive `Denied{Busy}` and retry every 30 s for up to 1 h.
- Path sanitization applies to every `rel` on both sides.

## 3b. ALPN `toss/xfer/2` — segmented data streams (v0.4)

The ALPN is the version. Hosts accept `toss/xfer/2` and `toss/xfer/1`; v0.4
clients dial `toss/xfer/2` with `toss/xfer/1` offered as an additional ALPN and
follow whichever the host picks (`Connection::alpn()`), so v0.3 hosts and
clients keep working. Control stream, `ClientHello`, `HostHello`, `Manifest`,
`Want` and `Done` and their framing are exactly as in §3.

Data: the sender (host on get, client on put) opens uni streams. Each stream
carries one or more *segments* back to back and ends with FIN:

```
u32 LE index | u64 LE offset | u64 LE len | <len bytes>      (20-byte header)
```

Rules: `index` is a `Want` item; `WantItem.offset <= offset` and
`offset + len <= size`; segments of one file never overlap and together cover
`[WantItem.offset, size)` exactly; every wanted file gets at least one segment
(`len = 0` for a file with nothing left, e.g. empty files). The receiver stops
accepting streams once every wanted file is verified (or failed verification),
then sends `Done` as in §3.

Sender policy (not a receiver requirement, `NodeConfig.net.tuning`): files
larger than `range_split_mb` are split into segments of
`min(range_split_mb, 64 MiB)`, one per stream; files smaller than
`small_file_kb` are batched (≤ 16 MiB and ≤ 4096 files per stream); earlier
streams get higher QUIC send priority. Streams in flight: `tuning.streams`
(8), `tuning.streams_high_rtt` (16) while the path is relay or RTT > 30 ms;
a host `max_parallel_streams` other than 0/4 caps it.

Receiver: writes each segment at its offset into `<rel>.part`, keeps a per-file
contiguous watermark and trims `.part` to it whenever a session ends before the
file is verified, so `.part`-size resume (§3 Resume) stays exact. blake3 is
checked incrementally (rolling over the contiguous prefix for split files);
`sync_data` once per file; host never overwrites on put (same as §3).

Transport (v0.4 defaults, `tuning.rs`): per-stream receive window 2 MiB
(hard cap — noq closes a connection with more than 1024 gaps in one stream
buffer), connection receive window = streams_high_rtt × stream window, send
window 64 MiB, Cubic (BBRv3 opt-in), initial RTT 50 ms.

## 4. ALPN `toss/ctl/1` — control (admin devices → host)

Control stream only, request/response, postcard with the same length prefix.

```rust
pub enum CtlRequest {
    Auth { ticket: Ticket, device: String },       // must be admin = true, valid
    Status,
    ListDir { root: String, path: String },        // one level
    CreateTicket(TicketSpec),
    ListJobs,
    Revoke { id: [u8; 16] },
}
pub enum CtlResponse {
    Ok,
    Denied { reason: DenyReason },
    Status(HostStatus),
    Dir { entries: Vec<DirEntry> },                // name, is_dir, size, mtime
    Ticket(Ticket),
    Jobs(Vec<JobRecord>),
}
pub struct TicketSpec { pub job: String, pub root: String, pub path: String, pub ret: String, pub get: bool, pub put: bool, pub days: u32 /* 0 = never */ }
pub struct HostStatus { pub device: String, pub version: String, pub roots: Vec<String>, pub active: u32, pub queued: u32, pub uptime_s: u64 }
pub struct JobRecord { pub id: [u8;16], pub job: String, pub root: String, pub path: String, pub get: bool, pub put: bool, pub exp: u64, pub created: u64, pub revoked: bool, pub last: Option<JobEvent> }
pub struct JobEvent { pub at: u64, pub device: String, pub op: Op, pub files: u32, pub bytes: u64, pub ok: bool, pub msg: String }
```

The host persists `JobRecord`s in `<data_dir>/jobs.json` (append on create,
update `last` on each `Done`). Creating a ticket **on the host itself** (CLI or
desktop UI running in host mode) uses the same `TicketSpec` without the
control channel.

## 5. Discovery, relay, identity

- Secret key: `<data_dir>/key` (32 bytes, mode 0600; created on first run).
  `data_dir` = `dirs::data_dir()/toss` (`~/Library/Application Support/toss`,
  `%APPDATA%\toss`, `~/.local/share/toss`); the CLI accepts `--data-dir`.
- iroh `Endpoint` built with `presets::N0` (pkarr publish of the home relay
  URL to n0 + pkarr/DNS resolve, default n0 relays) plus an in-memory
  `MemoryLookup` (`Node::add_peer_addr`); ALPNs `toss/xfer/1` and `toss/ctl/1`
  on hosts, none on pure clients. A client that is also an admin device
  accepts no ALPN.
- Dialing (get, put, ctl): the client connects to the `EndpointAddr` built from
  the ticket (host id + `relay` + `addrs`), so iroh starts via the host's relay
  immediately and tries the direct addresses / hole-punches in parallel. n0
  DNS/pkarr lookup is **not** required but still runs as a fallback (iroh
  starts it whenever no path is selected, e.g. when the ticket's addresses are
  stale or empty). Each attempt is bounded by 45 s (iroh's own handshake gives
  up after ~30 s idle); the retry policy (every 30 s; Busy 1 h, HostOffline
  24 h) is unchanged.
- Logging (info level, target `toss_core`): client — every dial attempt (ticket
  relay / number of direct addrs, home relay online?), success (path,
  elapsed) or failure (error, paths iroh knew), plus a background n0 lookup
  report after a failure; host — every accepted connection (remote id, ALPN,
  path) and every denial with its reason; every node — home relay
  connected / not connected (with the last error). iroh's own
  `home is now relay …` only means the relay was *selected*.
- `TransferEvent::Progress.path` reports `Direct` or `Relay` from the
  connection's selected path (`Connection::paths()`, see CHANGES.md).

## 6. ALPN `toss/peer/1` — contacts (v0.5)

Pairing, presence and push between contacts ("Danh bạ máy"). Invite string
`tossinv1…`, `PeerReq`/`PeerResp`, push data phase = xfer/2 put semantics:
see `contracts/contacts-v0.5.md` §2 and `contracts/CHANGES.md` (2026-10-23).

## 7. v0.8 — nearby (mDNS), pushes and text to non-contacts (`contracts/nearby-v0.8.md` §1)

### 7.1 mDNS user data
Every node with `net.mdns` advertises, next to its addresses under
`_toss._udp.local`, the TXT user data (iroh `UserData`, ≤ 245 bytes):

```
n=<device name>;p=<platform>;v=<app version>        e.g. "n=Đạt – Mac;p=macos;v=0.8.0"
```
`;` and `=` in a value are replaced by `,` and `-`; control characters are
dropped; the name is cut (at a character boundary) so the whole string fits
245 bytes. Unknown keys are ignored when parsing; a missing key is "". A Toss
< 0.8 advertises no user data (shown with version "" — it cannot receive
`PushFrom`/`Text`). The user data is set **only on the mDNS service** (a
wrapper around `MdnsAddressLookup` adds it to what iroh publishes), not via
`Endpoint::set_user_data_for_address_lookup`, which would also publish the
device name to the n0 DNS/pkarr servers. It is re-published at once on
`Node::set_device_name`.

Nearby store (runtime only): `MdnsAddressLookup::subscribe()` `Discovered` →
upsert (`id`, name/platform/version from the user data, direct addrs),
`Expired` → remove. iroh only reports *changes* to subscribers, so every 30 s
entries not announced for 60 s are looked up again (the mDNS cache answers
while the peer has not expired) and entries not seen for **3 min** are dropped.
A contact seen nearby is marked online at once.

### 7.2 `toss/peer/1` additions (append-only)
Postcard encodes enum variants by index; the v0.5 indices are unchanged, the
new variants are appended (checked by `wire::tests::peer_variant_indices_are_stable`):

```
PeerReq  = Pair(0) | Hello(1) | Push(2)
         | PushFrom(3) { push_id: [u8;16], job, files: u32, bytes: u64, name, platform, version }
         | Text(4)     { name, text }                      // text ≤ 60 KiB UTF-8 (MAX_TEXT)
PeerResp = Paired(0) | Pong(1) | PushAccepted(2) | PushDeclined(3) | PushQueued(4) | Denied(5)
         | PushAcceptedPaired(6) { name, platform, version }
         | TextOk(7)
```
- Sender: a **contact** always gets `Push` (so 0.6 contacts keep working);
  `PushFrom` goes only to a non-contact taken from the nearby store, dialed at
  the addresses mDNS saw (plus any contact hint). `Text` goes to contacts and
  nearby devices. `PushAcceptedPaired` / `TextOk` are only sent in reply to
  `PushFrom` / `Text`.
- Receiver gate: `PushFrom` and `Text` from a non-contact are allowed only when
  its endpoint id is in the receiver's nearby store; otherwise `Denied`. `Hello`
  and `Push` from a non-contact are `Denied` as in v0.5. A `PushFrom` from a
  contact is handled as a contact push.
- Receive policy (`Node::set_receive_policy`): `off` → every push asks
  (contacts too); `contacts` (default) → contact with `auto_accept` at once,
  everyone else asks; `everyone` → like `contacts`, plus nearby strangers at
  once (not added to contacts). A push id accepted once is not asked again when
  it resumes. Ask = `PushQueued` + `PushIncoming{stranger}` event, answered by
  `answer_push(push_id, accept, always)` within 10 min.
- `always` on a contact sets its `auto_accept = true`. On a stranger it adds the
  sender as a contact (name/platform from `PushFrom`, addresses from mDNS,
  `auto_accept = true`) and the receiver answers **`PushAcceptedPaired`**
  (sent whenever the request was `PushFrom` and the sender is a contact of the
  receiver after the answer); the sender then adds the receiver as a contact
  too. The data phase after `PushAccepted` / `PushAcceptedPaired` is the v0.5
  push data phase (xfer/2 put semantics). Stranger pushes land in
  `<inbox>/<sender name>/<job>/`.
- `Text`: contacts and nearby strangers → `TextIncoming` event on the receiver
  (any policy; control characters except `\n`/`\t` removed), answered `TextOk`.
  Nothing is stored.

### 7.3 Compatibility with 0.6 peers
- 0.6 → 0.8 and 0.8 → 0.6 between contacts: unchanged messages (`Pair`,
  `Hello`, `Push`, …), verified by decoding v0.8 encodings with the 0.6 enum
  (`tests/nearby.rs::old_variants_decode_as_0_6`).
- 0.8 → 0.6 non-contact: the 0.6 node cannot decode `PushFrom`/`Text` and
  closes the connection with code 1 / reason `"bad request"`. The 0.8 sender
  detects that close reason and reports (Vietnamese, final, no retry) "Máy kia
  đang dùng Toss bản cũ — cập nhật Toss trên máy đó (bản 0.8 trở lên) rồi thử
  lại" (`tests/nearby.rs::old_peer_gets_a_clear_error`).
- 0.6 never sends the new variants. A 0.6 node advertises no mDNS user data;
  a 0.8 node lists it in `nearby()` with version "".

### 7.4 Single-file tickets
A ticket's `root/path` may name a **file**: the manifest then has exactly one
entry whose `rel` is the file name, and `get` saves `<dest>/<job>/<file name>`.
Quick share of a file = the file's parent folder as root, the file name as
`path`, job = the file name without extension (`toss_core::paths::share_target`).
This is how a host has always built manifests (`host::build_manifest`), and the
client get path (`client::plan_get`) is byte-for-byte the one shipped in 0.6.0,
so **0.6 (and 0.4/0.5) clients download file links fine** — no version bump
(`tests/nearby.rs::single_file_ticket_get` also runs the xfer/1 path).

### 7.5 Error texts
Every error that reaches the UI (transfer `Error.msg`, `Node`/`CtlClient` API
errors, `DenyReason`'s `Display`, `TicketError`'s `Display`) is Vietnamese since
0.8. Messages *on the wire* that old peers parse (`Done.msg` with "hash
mismatch" / "skipped") stay English; they are logged and replaced by
Vietnamese text before they reach the UI.
