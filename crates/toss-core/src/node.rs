//! The [`Node`]: one iroh endpoint, optional host mode, client transfers,
//! remote control.

use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc, Weak,
    },
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use iroh::{
    address_lookup::MemoryLookup,
    endpoint::{presets, Connection},
    Endpoint, EndpointAddr, EndpointId, SecretKey,
};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch};

use crate::{
    client,
    fsutil::{is_hidden, HashCache, PART_SUFFIX},
    host,
    jobs::{JobRecord, JobStore},
    paths::{canonicalize, resolve_inside},
    ticket::{Ticket, TicketSpec},
    transfer::{Tracker, TransferEvent, TransferId, TransferState},
    util::unix_of,
    wire::{self, DirEntry},
    CtlClient, ALPN_CTL, ALPN_XFER,
};

/// Node configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeConfig {
    /// Data directory (key, jobs.json, revoked.json); see [`default_data_dir`].
    pub data_dir: PathBuf,
    /// Shown to the other side.
    pub device_name: String,
    /// App version, sent in hellos and reported in host status.
    pub app_version: String,
}

/// A named top folder a host may serve from.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Root {
    /// `[A-Za-z0-9_-]{1,32}`
    pub alias: String,
    /// Folder.
    pub path: PathBuf,
}

/// Host mode configuration.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostConfig {
    /// Roots that tickets may reference.
    pub roots: Vec<Root>,
    /// Where puts land (`return_root/<ret>/…`).
    pub return_root: PathBuf,
    /// Concurrent transfers; extra clients get `Busy` (default 2).
    #[serde(default = "default_jobs")]
    pub max_parallel_jobs: u32,
    /// Concurrent uni streams per transfer (default 4).
    #[serde(default = "default_streams")]
    pub max_parallel_streams: u32,
    /// Fixed UDP port to bind (IPv4 and IPv6), e.g. for a port forward on the
    /// studio router. `None` = random port. The endpoint is bound when the node
    /// starts, so this only takes effect through [`Node::start_with_port`]:
    /// changing it on a running node (via [`Node::set_host`]) is stored but
    /// needs a node restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_port: Option<u16>,
}

fn default_jobs() -> u32 {
    2
}
fn default_streams() -> u32 {
    4
}

impl HostConfig {
    /// Config with default limits.
    pub fn new(roots: Vec<Root>, return_root: PathBuf) -> HostConfig {
        HostConfig {
            roots,
            return_root,
            max_parallel_jobs: default_jobs(),
            max_parallel_streams: default_streams(),
            bind_port: None,
        }
    }
}

/// `dirs::data_dir()/toss` (falls back to `./toss-data`).
pub fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .map(|d| d.join("toss"))
        .unwrap_or_else(|| PathBuf::from("toss-data"))
}

/// Loads `<data_dir>/key` or creates it (32 random bytes, mode 0600).
pub fn load_or_create_secret_key(data_dir: &Path) -> Result<SecretKey> {
    let path = data_dir.join("key");
    match std::fs::read(&path) {
        Ok(bytes) => {
            let arr: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| anyhow!("{} is not a 32-byte key", path.display()))?;
            Ok(SecretKey::from_bytes(&arr))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(data_dir)
                .with_context(|| format!("create {}", data_dir.display()))?;
            let key = SecretKey::generate();
            write_key(&path, &key.to_bytes())?;
            Ok(key)
        }
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

fn write_key(path: &Path, bytes: &[u8; 32]) -> Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .with_context(|| format!("create {}", path.display()))?;
    f.write_all(bytes)
        .with_context(|| format!("write {}", path.display()))?;
    f.sync_all().ok();
    Ok(())
}

/// Validated host config with canonical paths.
#[derive(Debug, Clone)]
pub(crate) struct HostRuntime {
    pub cfg: HostConfig,
    pub roots: Vec<(String, PathBuf)>,
    pub return_root_c: PathBuf,
}

impl HostRuntime {
    pub fn new(cfg: HostConfig) -> Result<HostRuntime> {
        let mut roots = Vec::new();
        let mut seen = BTreeSet::new();
        for r in &cfg.roots {
            if !valid_alias(&r.alias) {
                bail!(
                    "invalid root alias {:?} (use A-Z a-z 0-9 _ -, max 32)",
                    r.alias
                );
            }
            if !seen.insert(r.alias.clone()) {
                bail!("duplicate root alias {:?}", r.alias);
            }
            let p = canonicalize(&r.path).with_context(|| format!("root {}", r.alias))?;
            if !p.is_dir() {
                bail!("root {} ({}) is not a folder", r.alias, p.display());
            }
            roots.push((r.alias.clone(), p));
        }
        std::fs::create_dir_all(&cfg.return_root)
            .with_context(|| format!("create return root {}", cfg.return_root.display()))?;
        let return_root_c = canonicalize(&cfg.return_root)?;
        Ok(HostRuntime {
            cfg,
            roots,
            return_root_c,
        })
    }

    pub fn root(&self, alias: &str) -> Option<&Path> {
        self.roots
            .iter()
            .find(|(a, _)| a == alias)
            .map(|(_, p)| p.as_path())
    }
}

/// `[A-Za-z0-9_-]{1,32}`.
pub fn valid_alias(a: &str) -> bool {
    (1..=32).contains(&a.len())
        && a.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Client-side control of a running transfer task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ctrl {
    Run,
    Pause,
    Cancel,
}

#[derive(Debug)]
pub(crate) struct Running {
    pub ctrl: watch::Sender<Ctrl>,
    pub handle: tokio::task::JoinHandle<()>,
}

/// A client-side transfer (get or put) that can be paused/resumed.
#[derive(Debug)]
pub(crate) struct ClientJob {
    pub ticket: Ticket,
    pub op: wire::Op,
    /// get: `dest/<job>` (created at start).
    pub job_dir: Option<PathBuf>,
    /// put: selected paths.
    pub paths: Vec<PathBuf>,
    /// `.part` files this transfer may have created (deleted on cancel).
    pub part_files: Mutex<BTreeSet<PathBuf>>,
    pub running: tokio::sync::Mutex<Option<Running>>,
}

#[derive(Debug)]
pub(crate) enum EntryKind {
    Client(Arc<ClientJob>),
    Host(Connection),
}

#[derive(Debug)]
pub(crate) struct Entry {
    pub seq: u64,
    pub tracker: Arc<Tracker>,
    pub kind: EntryKind,
}

/// Shared node state.
#[derive(Debug)]
pub(crate) struct Inner {
    pub endpoint: Endpoint,
    pub secret: SecretKey,
    pub cfg: NodeConfig,
    pub lookup: MemoryLookup,
    pub host: RwLock<Option<HostRuntime>>,
    pub events: broadcast::Sender<TransferEvent>,
    pub transfers: Mutex<HashMap<TransferId, Entry>>,
    pub seq: AtomicU64,
    pub jobs: JobStore,
    pub hashes: Arc<HashCache>,
    pub started: Instant,
    pub active: AtomicU32,
    pub busy_seen: Mutex<HashMap<EndpointId, Instant>>,
    /// Targets of running puts (see `host::PutReservations`).
    pub put_reserved: host::PutReservations,
    /// Fixed UDP port the endpoint was bound to (None = random).
    pub bind_port: Option<u16>,
}

impl Inner {
    pub fn register(&self, tracker: Arc<Tracker>, kind: EntryKind) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        self.transfers
            .lock()
            .insert(tracker.id.clone(), Entry { seq, tracker, kind });
    }

    pub fn host_runtime(&self) -> Option<HostRuntime> {
        self.host.read().clone()
    }

    pub fn host_status(&self) -> Option<wire::HostStatus> {
        let host = self.host.read();
        let rt = host.as_ref()?;
        let queued = {
            let mut seen = self.busy_seen.lock();
            seen.retain(|_, t| t.elapsed() < Duration::from_secs(45));
            seen.len() as u32
        };
        Some(wire::HostStatus {
            device: self.cfg.device_name.clone(),
            version: self.cfg.app_version.clone(),
            roots: rt.roots.iter().map(|(a, _)| a.clone()).collect(),
            active: self.active.load(Ordering::Relaxed),
            queued,
            uptime_s: self.started.elapsed().as_secs(),
        })
    }

    pub fn emit_host_status(&self) {
        if let Some(st) = self.host_status() {
            let _ = self.events.send(TransferEvent::HostStatus(st));
        }
    }

    /// Dials `addr` with ALPN; times out after 30 s.
    pub async fn connect(&self, addr: impl Into<EndpointAddr>, alpn: &[u8]) -> Result<Connection> {
        let addr = addr.into();
        let id = addr.id;
        match tokio::time::timeout(Duration::from_secs(30), self.endpoint.connect(addr, alpn)).await
        {
            Ok(Ok(c)) => Ok(c),
            Ok(Err(e)) => Err(anyhow!(e)).with_context(|| format!("connect to {}", id.fmt_short())),
            Err(_) => bail!("connect to {}: timed out", id.fmt_short()),
        }
    }
}

/// A Toss node. Cheap to clone (shared state).
#[derive(Clone, Debug)]
pub struct Node {
    pub(crate) inner: Arc<Inner>,
}

impl Node {
    /// Creates the data dir, loads/creates the key, binds the iroh endpoint
    /// (n0 preset: DNS/pkarr discovery + default relays) on a random UDP port
    /// and starts accepting.
    pub async fn start(cfg: NodeConfig) -> Result<Node> {
        Node::start_with_port(cfg, None).await
    }

    /// Like [`Node::start`], binding UDP `port` (IPv4 `0.0.0.0:port`, plus
    /// `[::]:port` when IPv6 is available) instead of a random one; meant for
    /// hosts ([`HostConfig::bind_port`]). Fails if the IPv4 port is taken.
    pub async fn start_with_port(cfg: NodeConfig, port: Option<u16>) -> Result<Node> {
        std::fs::create_dir_all(&cfg.data_dir)
            .with_context(|| format!("create data dir {}", cfg.data_dir.display()))?;
        let secret = load_or_create_secret_key(&cfg.data_dir)?;
        let lookup = MemoryLookup::new();
        let mut builder = Endpoint::builder(presets::N0)
            .secret_key(secret.clone())
            .address_lookup(lookup.clone())
            .alpns(vec![]);
        if let Some(port) = port {
            use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};
            builder = builder
                .bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port))
                .map_err(|e| anyhow!(e))
                .context("bind address")?
                .bind_addr_with_opts(
                    SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, port, 0, 0),
                    iroh::endpoint::BindOpts::default().set_is_required(false),
                )
                .map_err(|e| anyhow!(e))
                .context("bind address")?;
        }
        let endpoint =
            builder
                .bind()
                .await
                .map_err(|e| anyhow!(e))
                .with_context(|| match port {
                    Some(p) => format!("bind iroh endpoint on UDP port {p}"),
                    None => "bind iroh endpoint".to_string(),
                })?;
        tracing::info!(id = %endpoint.id(), "toss node started");
        let (events, _) = broadcast::channel(1024);
        let inner = Arc::new(Inner {
            endpoint: endpoint.clone(),
            secret,
            jobs: JobStore::open(&cfg.data_dir),
            cfg,
            lookup,
            host: RwLock::new(None),
            events,
            transfers: Mutex::new(HashMap::new()),
            seq: AtomicU64::new(0),
            hashes: Arc::new(HashCache::default()),
            started: Instant::now(),
            active: AtomicU32::new(0),
            busy_seen: Mutex::new(HashMap::new()),
            put_reserved: Default::default(),
            bind_port: port,
        });
        tokio::spawn(accept_loop(endpoint, Arc::downgrade(&inner)));
        Ok(Node { inner })
    }

    /// This node's endpoint id (= host public key).
    pub fn endpoint_id(&self) -> EndpointId {
        self.inner.endpoint.id()
    }

    /// Device name from the config.
    pub fn device_name(&self) -> &str {
        &self.inner.cfg.device_name
    }

    /// The underlying iroh endpoint (advanced use: `online()`, `addr()`, …).
    pub fn endpoint(&self) -> &Endpoint {
        &self.inner.endpoint
    }

    /// Adds addressing info for a peer (direct addresses / relay) so it can be
    /// dialed without discovery. Useful on a LAN and in tests.
    pub fn add_peer_addr(&self, addr: EndpointAddr) {
        self.inner.lookup.add_endpoint_info(addr);
    }

    /// The fixed UDP port this node was started with (`None` = random).
    pub fn bind_port(&self) -> Option<u16> {
        self.inner.bind_port
    }

    /// Turn host mode on (accept xfer + ctl) or off (None). Idempotent; replaces config.
    /// `cfg.bind_port` is not applied here (see [`HostConfig::bind_port`]).
    pub async fn set_host(&self, cfg: Option<HostConfig>) -> Result<()> {
        match cfg {
            Some(cfg) => {
                if cfg.bind_port.is_some() && cfg.bind_port != self.inner.bind_port {
                    tracing::warn!(
                        "bind port {:?} takes effect after a restart (bound: {:?})",
                        cfg.bind_port,
                        self.inner.bind_port
                    );
                }
                let rt = tokio::task::spawn_blocking(move || HostRuntime::new(cfg)).await??;
                *self.inner.host.write() = Some(rt);
                self.inner
                    .endpoint
                    .set_alpns(vec![ALPN_XFER.to_vec(), ALPN_CTL.to_vec()]);
                tracing::info!("host mode on");
                self.inner.emit_host_status();
            }
            None => {
                self.inner.endpoint.set_alpns(vec![]);
                *self.inner.host.write() = None;
                tracing::info!("host mode off");
            }
        }
        Ok(())
    }

    /// Current host config (None = host mode off).
    pub fn host_config(&self) -> Option<HostConfig> {
        self.inner.host.read().as_ref().map(|h| h.cfg.clone())
    }

    /// Host status (None = host mode off).
    pub fn host_status(&self) -> Option<wire::HostStatus> {
        self.inner.host_status()
    }

    /// Host-local ticket creation (signs with this node's key). Errors if host mode is off.
    pub fn create_ticket(&self, spec: TicketSpec) -> Result<Ticket> {
        let rt = self
            .inner
            .host_runtime()
            .ok_or_else(|| anyhow!("host mode is off"))?;
        host::issue_ticket(&self.inner.secret, &self.inner.jobs, &rt, spec)
    }

    /// Creates an admin ticket (control channel access). Errors if host mode is off.
    pub fn create_admin_ticket(&self, days: u32) -> Result<Ticket> {
        if self.inner.host.read().is_none() {
            bail!("host mode is off");
        }
        host::issue_admin_ticket(&self.inner.secret, &self.inner.jobs, days)
    }

    /// Re-signs the ticket of a stored job (same string as when created).
    pub fn job_ticket(&self, id: [u8; 16]) -> Option<Ticket> {
        let body = self.inner.jobs.body(id, *self.endpoint_id().as_bytes())?;
        Some(Ticket::sign(body, &self.inner.secret))
    }

    /// All jobs issued by this host (from `jobs.json`).
    pub fn list_jobs(&self) -> Vec<JobRecord> {
        self.inner.jobs.list()
    }

    /// Revokes a ticket id (persisted in `revoked.json`).
    pub fn revoke(&self, id: [u8; 16]) -> Result<()> {
        self.inner.jobs.revoke(id)
    }

    /// One level of `root/rel` (hidden, `.part` and symlinks skipped; folders first).
    pub fn list_dir(&self, root: &str, rel: &str) -> Result<Vec<DirEntry>> {
        let rt = self
            .inner
            .host_runtime()
            .ok_or_else(|| anyhow!("host mode is off"))?;
        list_dir(&rt, root, rel)
    }

    /// Client side. `dest` is the parent folder; files land in `dest/<sanitized job>/`.
    pub async fn get(&self, ticket: Ticket, dest: PathBuf) -> Result<crate::Transfer> {
        client::start(&self.inner, ticket, wire::Op::Get, Some(dest), vec![]).await
    }

    /// Client side. `paths` are files/folders to send; a folder keeps its name as first segment.
    pub async fn put(&self, ticket: Ticket, paths: Vec<PathBuf>) -> Result<crate::Transfer> {
        client::start(&self.inner, ticket, wire::Op::Put, None, paths).await
    }

    /// Remote control of a host via an admin ticket.
    pub async fn ctl(&self, admin: Ticket) -> Result<CtlClient> {
        CtlClient::connect(self.inner.clone(), admin).await
    }

    /// Every event of every transfer (client- and host-side) plus host events.
    pub fn subscribe(&self) -> broadcast::Receiver<TransferEvent> {
        self.inner.events.subscribe()
    }

    /// Snapshots of all transfers, oldest first.
    pub fn transfers(&self) -> Vec<TransferState> {
        let map = self.inner.transfers.lock();
        let mut v: Vec<_> = map
            .values()
            .map(|e| (e.seq, e.tracker.snapshot()))
            .collect();
        v.sort_by_key(|(s, _)| *s);
        v.into_iter().map(|(_, s)| s).collect()
    }

    /// Snapshot of one transfer.
    pub fn transfer(&self, id: &TransferId) -> Option<TransferState> {
        self.inner
            .transfers
            .lock()
            .get(id)
            .map(|e| e.tracker.snapshot())
    }

    /// Removes finished (done/error/cancelled) transfers from the list.
    pub fn clear_finished(&self) {
        self.inner
            .transfers
            .lock()
            .retain(|_, e| !e.tracker.status().is_final());
    }

    /// Closes the connection, keeps `.part` files (client transfers only).
    pub async fn pause(&self, id: &TransferId) -> Result<()> {
        let (job, _) = self.client_job(id)?;
        client::pause(job).await
    }

    /// Reconnects with the same ticket (paused or failed client transfers).
    pub async fn resume(&self, id: &TransferId) -> Result<()> {
        let (job, tracker) = self.client_job(id)?;
        client::resume(&self.inner, job, tracker).await
    }

    /// Closes, deletes this transfer's `.part` files. On a host-side transfer it
    /// just drops the client's connection.
    pub async fn cancel(&self, id: &TransferId) -> Result<()> {
        let kind = {
            let map = self.inner.transfers.lock();
            let e = map
                .get(id)
                .ok_or_else(|| anyhow!("unknown transfer {id}"))?;
            match &e.kind {
                EntryKind::Client(j) => Ok((j.clone(), e.tracker.clone())),
                EntryKind::Host(c) => Err((c.clone(), e.tracker.clone())),
            }
        };
        match kind {
            Ok((job, tracker)) => client::cancel(job, tracker).await,
            Err((conn, tracker)) => {
                conn.close(2u32.into(), b"cancelled by host");
                tracker.error(
                    crate::transfer::ErrorCode::Cancelled,
                    "cancelled".into(),
                    None,
                );
                Ok(())
            }
        }
    }

    fn client_job(&self, id: &TransferId) -> Result<(Arc<ClientJob>, Arc<Tracker>)> {
        let map = self.inner.transfers.lock();
        let e = map
            .get(id)
            .ok_or_else(|| anyhow!("unknown transfer {id}"))?;
        match &e.kind {
            EntryKind::Client(j) => Ok((j.clone(), e.tracker.clone())),
            EntryKind::Host(_) => bail!("host-side transfers cannot be paused or resumed"),
        }
    }

    /// Pauses all client transfers (keeping `.part` files) and closes the endpoint.
    pub async fn shutdown(self) {
        let jobs: Vec<_> = {
            let map = self.inner.transfers.lock();
            map.values()
                .filter_map(|e| match &e.kind {
                    EntryKind::Client(j) => Some(j.clone()),
                    EntryKind::Host(_) => None,
                })
                .collect()
        };
        for j in jobs {
            let _ = client::pause(j).await;
        }
        self.inner.endpoint.close().await;
    }
}

async fn accept_loop(endpoint: Endpoint, inner: Weak<Inner>) {
    while let Some(incoming) = endpoint.accept().await {
        let inner = inner.clone();
        tokio::spawn(async move {
            let mut accepting = match incoming.accept() {
                Ok(a) => a,
                Err(e) => {
                    tracing::debug!("incoming failed: {e:#}");
                    return;
                }
            };
            let alpn = match accepting.alpn().await {
                Ok(a) => a,
                Err(e) => {
                    tracing::debug!("alpn failed: {e:#}");
                    return;
                }
            };
            let conn = match accepting.await {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!("handshake failed: {e:#}");
                    return;
                }
            };
            let Some(inner) = inner.upgrade() else { return };
            if inner.host.read().is_none() {
                conn.close(1u32.into(), b"host mode off");
                return;
            }
            if alpn == ALPN_XFER {
                host::handle_xfer(inner, conn).await;
            } else if alpn == ALPN_CTL {
                crate::ctl::handle_ctl(inner, conn).await;
            } else {
                conn.close(1u32.into(), b"unknown alpn");
            }
        });
    }
    tracing::debug!("accept loop ended");
}

pub(crate) fn list_dir(rt: &HostRuntime, root: &str, rel: &str) -> Result<Vec<DirEntry>> {
    let base = rt
        .root(root)
        .ok_or_else(|| anyhow!("unknown root {root:?}"))?;
    let dir = resolve_inside(base, rel)?;
    let mut out = Vec::new();
    for e in std::fs::read_dir(&dir).with_context(|| format!("read dir {}", dir.display()))? {
        let Ok(e) = e else { continue };
        let Ok(name) = e.file_name().into_string() else {
            continue;
        };
        let Ok(meta) = e.path().symlink_metadata() else {
            continue;
        };
        if meta.file_type().is_symlink() || is_hidden(&name, &meta) || name.ends_with(PART_SUFFIX) {
            continue;
        }
        out.push(DirEntry {
            name,
            is_dir: meta.is_dir(),
            size: if meta.is_dir() { 0 } else { meta.len() },
            mtime: meta.modified().map(unix_of).unwrap_or(0),
        });
    }
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
    Ok(out)
}

/// Creates a job ticket without a running node (e.g. `toss-cli ticket` while
/// `toss-cli host` runs in another process): loads the key from `data_dir`,
/// validates `spec` against `cfg`, signs and appends to `jobs.json`.
pub fn issue_ticket_offline(data_dir: &Path, cfg: &HostConfig, spec: TicketSpec) -> Result<Ticket> {
    let secret = load_or_create_secret_key(data_dir)?;
    let rt = HostRuntime::new(cfg.clone())?;
    host::issue_ticket(&secret, &JobStore::open(data_dir), &rt, spec)
}

/// Creates an admin ticket without a running node.
pub fn issue_admin_ticket_offline(data_dir: &Path, days: u32) -> Result<Ticket> {
    let secret = load_or_create_secret_key(data_dir)?;
    host::issue_admin_ticket(&secret, &JobStore::open(data_dir), days)
}
