# Wire contract — ticket format, URL shape, ALPN protocols

## 1. Ticket

A ticket is a postcard-serialized `TicketBody` plus an Ed25519 signature by the
host's iroh secret key (the same key that is the host's `EndpointId`).

```rust
// crates/toss-core/src/ticket.rs
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TicketBody {
    pub v: u8,                 // = 1
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
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Ticket {
    pub body: TicketBody,
    pub sig: [u8; 64],         // ed25519 over postcard(body)
}
```

String form: `toss1` + base32 (RFC 4648, no padding, **lowercase**) of
`postcard(Ticket)`. Example length: ~330 chars. `Ticket::from_str` accepts
upper or lower case, trims whitespace, accepts the string with or without the
`toss://t/` or `https://<any host>/#` prefix.

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
- iroh `Endpoint` built with default discovery (n0 DNS publish + resolve) and
  default relays; ALPNs `toss/xfer/1` and `toss/ctl/1` on hosts, none on
  pure clients. A client that is also an admin device accepts no ALPN.
- `TransferEvent::Progress.path` reports `Direct` or `Relay` from
  `endpoint.conn_type(remote)`.
