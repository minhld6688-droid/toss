# Core API contract — `toss-core` (Rust) as used by `toss-cli` and `apps/desktop/src-tauri`

Crate `toss-core`, lib only, tokio-based. Public surface (exact names):

```rust
// crates/toss-core/src/lib.rs
pub mod ticket;   // Ticket, TicketBody, TicketSpec, TicketError (wire.md §1)
pub mod wire;     // all protocol enums/structs from wire.md §3–§4
pub mod paths;    // sanitize_rel(&str) -> Result<RelPath>, resolve_inside(root, rel) -> Result<PathBuf>
pub mod node;     // Node, NodeConfig, HostConfig, Root
pub mod transfer; // Transfer, TransferEvent, TransferState, ConnPath
pub mod ctl;      // CtlClient
pub mod jobs;     // JobRecord, JobEvent, JobStore

pub use node::{Node, NodeConfig, HostConfig, Root};
pub use ticket::{Ticket, TicketBody, TicketSpec};
pub use transfer::{Transfer, TransferEvent, TransferState, ConnPath, TransferId};
pub use ctl::CtlClient;
pub use jobs::{JobRecord, JobEvent};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const ALPN_XFER: &[u8] = b"toss/xfer/1";
pub const ALPN_CTL: &[u8] = b"toss/ctl/1";
```

```rust
pub struct NodeConfig {
    pub data_dir: PathBuf,        // see wire.md §5
    pub device_name: String,      // shown to the other side
    pub app_version: String,
}

pub struct Root { pub alias: String, pub path: PathBuf }   // alias: [A-Za-z0-9_-]{1,32}

pub struct HostConfig {
    pub roots: Vec<Root>,
    pub return_root: PathBuf,
    pub max_parallel_jobs: u32,   // default 2
    pub max_parallel_streams: u32,// default 4 (per transfer)
}

impl Node {
    pub async fn start(cfg: NodeConfig) -> anyhow::Result<Node>;
    pub fn endpoint_id(&self) -> iroh::EndpointId;
    pub fn device_name(&self) -> &str;

    /// Turn host mode on (accept xfer + ctl) or off (None). Idempotent; replaces config.
    pub async fn set_host(&self, cfg: Option<HostConfig>) -> anyhow::Result<()>;
    pub fn host_config(&self) -> Option<HostConfig>;
    pub fn host_status(&self) -> Option<wire::HostStatus>;

    /// Host-local ticket creation (signs with this node's key). Errors if host mode is off.
    pub fn create_ticket(&self, spec: TicketSpec) -> anyhow::Result<Ticket>;
    pub fn list_jobs(&self) -> Vec<JobRecord>;
    pub fn revoke(&self, id: [u8; 16]) -> anyhow::Result<()>;
    pub fn list_dir(&self, root: &str, rel: &str) -> anyhow::Result<Vec<wire::DirEntry>>;

    /// Client side. `dest` is the parent folder; files land in `dest/<sanitized job>/`.
    pub async fn get(&self, ticket: Ticket, dest: PathBuf) -> anyhow::Result<Transfer>;
    /// Client side. `paths` are files/folders to send; see wire.md Put for rel-path rules.
    pub async fn put(&self, ticket: Ticket, paths: Vec<PathBuf>) -> anyhow::Result<Transfer>;

    /// Remote control of a host via an admin ticket.
    pub async fn ctl(&self, admin: Ticket) -> anyhow::Result<CtlClient>;

    /// Every event of every transfer (client- and host-side) plus host events.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<TransferEvent>;
    pub fn transfers(&self) -> Vec<TransferState>;
    pub fn transfer(&self, id: &TransferId) -> Option<TransferState>;
    pub async fn pause(&self, id: &TransferId) -> anyhow::Result<()>;   // closes the connection, keeps .part files
    pub async fn resume(&self, id: &TransferId) -> anyhow::Result<()>;  // reconnects with the same ticket
    pub async fn cancel(&self, id: &TransferId) -> anyhow::Result<()>;  // closes, deletes this transfer's .part files
    pub async fn shutdown(self);
}

pub type TransferId = String;   // hex(ticket.id) + "-" + 4 random hex chars (one ticket may run twice)

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TransferEvent {
    Started   { id: TransferId, job: String, op: wire::Op, side: Side, peer: String /* device name or endpoint id */ },
    Manifest  { id: TransferId, files: u32, bytes: u64, to_do_files: u32, to_do_bytes: u64 },
    Progress  { id: TransferId, bytes_done: u64, bytes_total: u64, files_done: u32, files_total: u32, speed_bps: u64, eta_s: Option<u64>, path: ConnPath },
    Done      { id: TransferId, files: u32, bytes: u64, elapsed_s: u64, dest: Option<String> },
    Error     { id: TransferId, code: ErrorCode, msg: String, retry_in_s: Option<u64> },
    Paused    { id: TransferId },
    HostStatus(wire::HostStatus),
}
#[derive(Clone, Copy, Debug, Serialize)] #[serde(rename_all = "snake_case")]
pub enum Side { Client, Host }
#[derive(Clone, Copy, Debug, Serialize)] #[serde(rename_all = "snake_case")]
pub enum ConnPath { Direct, Relay, Mixed, Unknown }
#[derive(Clone, Copy, Debug, Serialize)] #[serde(rename_all = "snake_case")]
pub enum ErrorCode { Denied, Expired, Revoked, HostOffline, Busy, Io, HashMismatch, Cancelled, Other }

#[derive(Clone, Debug, Serialize)]
pub struct TransferState {   // snapshot for UIs
    pub id: TransferId, pub job: String, pub op: wire::Op, pub side: Side, pub peer: String,
    pub status: Status, pub bytes_done: u64, pub bytes_total: u64, pub files_done: u32, pub files_total: u32,
    pub speed_bps: u64, pub eta_s: Option<u64>, pub path: ConnPath, pub dest: Option<String>, pub error: Option<String>,
    pub started_at: u64,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)] #[serde(rename_all = "snake_case")]
pub enum Status { Connecting, Running, Paused, Done, Error, Cancelled }

pub struct Transfer { pub id: TransferId /* handle; await .wait() for the final state */ }
impl Transfer { pub async fn wait(&self) -> TransferState; }

impl CtlClient {
    pub async fn status(&self) -> anyhow::Result<wire::HostStatus>;
    pub async fn list_dir(&self, root: &str, rel: &str) -> anyhow::Result<Vec<wire::DirEntry>>;
    pub async fn create_ticket(&self, spec: TicketSpec) -> anyhow::Result<Ticket>;
    pub async fn list_jobs(&self) -> anyhow::Result<Vec<JobRecord>>;
    pub async fn revoke(&self, id: [u8; 16]) -> anyhow::Result<()>;
}

impl Ticket {
    pub fn to_string(&self) -> String;                 // "toss1..." (Display)
    pub fn from_str(s: &str) -> Result<Ticket, TicketError>;  // FromStr, tolerant (wire.md §1)
    pub fn web_url(&self, page_host: &str) -> String;  // "https://<page_host>/#toss1..."
    pub fn deep_link(&self) -> String;                 // "toss://t/toss1..."
    pub fn id_hex(&self) -> String;
    pub fn is_expired(&self) -> bool;
}
```

Behavioural requirements:
- Progress events at most every 500 ms per transfer, and always one Progress
  right before Done.
- Speed = moving average over the last 5 s.
- `get` creates `dest/<job>/` immediately so the UI can offer "open folder".
- Host side runs transfers on a queue: more than `max_parallel_jobs` → the
  client gets `Denied{Busy}`; the client-side `Transfer` stays `Connecting`
  and retries every 30 s (event `Error{code: Busy, retry_in_s: 30}` each time)
  for up to 1 h.
- Host offline (dial fails): same retry pattern with `HostOffline`, up to 24 h.
- Everything logs through `tracing`; nothing prints to stdout from the lib.

CLI `toss-cli` (thin, for tests on two real machines):
```
toss-cli id                                   # prints endpoint id + device name
toss-cli host --root RAW=/path --root Tra=/other --return /path/return [--max-jobs 2]
toss-cli ticket --job "12A3 RAW" --root RAW --path 2026/12A3 [--ret 12A3] [--get] [--put] [--days 7] [--admin]
toss-cli jobs | toss-cli revoke <id-hex>
toss-cli get <ticket> --dest ~/Toss
toss-cli put <ticket> PATH...
toss-cli ctl <admin-ticket> status | ls ROOT [PATH] | ticket ... | jobs
```
`host` runs until Ctrl-C and prints each job event as one line. Global flags:
`--data-dir`, `--name`.
