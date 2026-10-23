//! The [`Node`]: one iroh endpoint, optional host mode, client transfers,
//! remote control.

use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc, Weak,
    },
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use iroh::{
    address_lookup::MemoryLookup,
    endpoint::{presets, Connection, PortmapperConfig},
    Endpoint, EndpointAddr, EndpointId, SecretKey, Watcher,
};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch};

use crate::{
    client,
    contacts::{Contact, ContactBook, IncomingPush, Invite, InviteInfo, InviteStore},
    fsutil::{is_hidden, HashCache, PART_SUFFIX},
    host,
    jobs::{JobRecord, JobStore},
    paths::{canonicalize, resolve_inside},
    peer::PeerState,
    resume::TransferStore,
    ticket::{HostAddr, Ticket, TicketSpec},
    transfer::{conn_path, Tracker, TransferEvent, TransferId, TransferState},
    util::unix_of,
    wire::{self, DirEntry},
    CtlClient, ALPN_CTL, ALPN_PEER, ALPN_XFER, ALPN_XFER2,
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
    /// v0.4 network settings (relays, mDNS, port mapping, tuning).
    #[serde(default)]
    pub net: crate::net::NetConfig,
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
    /// UDP port a host binds: [`HostConfig::bind_port`] or
    /// [`crate::net::DEFAULT_HOST_PORT`] (41820) when unset (v0.4). Use with
    /// [`Node::start_host`], which falls back to a random port if it is taken.
    pub fn effective_bind_port(&self) -> u16 {
        self.bind_port.unwrap_or(crate::net::DEFAULT_HOST_PORT)
    }

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

/// Where a client transfer goes.
#[derive(Debug, Clone)]
pub(crate) enum Target {
    /// get/put with a ticket.
    Ticket(Ticket),
    /// v0.5 push to a contact (`op` = Put).
    Contact {
        /// Contact endpoint id.
        id: EndpointId,
        /// Push id; reused when the push resumes.
        push_id: [u8; 16],
        /// Job (folder) name announced to the receiver.
        job: String,
    },
}

/// A client-side transfer (get, put or push) that can be paused/resumed.
#[derive(Debug)]
pub(crate) struct ClientJob {
    pub target: Target,
    pub op: wire::Op,
    /// get: `dest/<job>` (created at start).
    pub job_dir: Option<PathBuf>,
    /// put: selected paths.
    pub paths: Vec<PathBuf>,
    /// `.part` files this transfer may have created (deleted on cancel).
    pub part_files: Mutex<BTreeSet<PathBuf>>,
    pub running: tokio::sync::Mutex<Option<Running>>,
}

impl ClientJob {
    /// The ticket of a get/put (None for a push).
    pub fn ticket(&self) -> Option<&Ticket> {
        match &self.target {
            Target::Ticket(t) => Some(t),
            Target::Contact { .. } => None,
        }
    }

    /// The contact of a push.
    pub fn contact(&self) -> Option<EndpointId> {
        match &self.target {
            Target::Ticket(_) => None,
            Target::Contact { id, .. } => Some(*id),
        }
    }
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
    /// v0.4: mDNS lookup (None = off or failed to start, see `net_notes`).
    pub mdns: Option<iroh_mdns_address_lookup::MdnsAddressLookup>,
    /// v0.4: start-time network problems for diagnostics (Vietnamese, one
    /// sentence each): host port taken, mDNS unavailable, bad relay URL.
    pub net_notes: Vec<String>,
    /// v0.5: paired devices (`contacts.json`).
    pub contacts: ContactBook,
    /// v0.5: issued pairing invites (`invites.json`).
    pub invites: InviteStore,
    /// v0.5: `toss/peer/1` runtime state (pending pushes, presence).
    pub peer: PeerState,
    /// v0.5: persisted client transfers (`transfers.json`).
    pub store: Arc<TransferStore>,
    /// v0.5: macOS Local Network access denied (lanprobe).
    pub lan_blocked: AtomicBool,
}

impl Inner {
    /// Emits an event to all subscribers.
    pub fn emit(&self, ev: TransferEvent) {
        let _ = self.events.send(ev);
    }

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

    /// This node's current relay + direct addresses, as put into tickets.
    pub fn host_addr(&self) -> HostAddr {
        HostAddr::from_endpoint_addr(&self.endpoint.addr())
    }

    /// Dials `addr` with ALPN, offering `additional` ALPNs after `alpn` (the host picks the
    /// first one it supports; read it back with `Connection::alpn()`). v0.4: get/put dial
    /// `toss/xfer/2` with `toss/xfer/1` as additional ALPN.
    pub async fn connect_alpns(
        &self,
        addr: impl Into<EndpointAddr>,
        alpn: &[u8],
        additional: Vec<Vec<u8>>,
    ) -> Result<Connection> {
        let addr = addr.into();
        let id = addr.id;
        let fut = async {
            let opts = iroh::endpoint::ConnectOptions::new().with_additional_alpns(additional);
            let connecting = self
                .endpoint
                .connect_with_opts(addr, alpn, opts)
                .await
                .map_err(|e| anyhow!(e))?;
            connecting.await.map_err(|e| anyhow!(e))
        };
        match tokio::time::timeout(CONNECT_TIMEOUT, fut).await {
            Ok(Ok(c)) => Ok(c),
            Ok(Err(e)) => Err(anyhow!(e)).with_context(|| format!("connect to {}", id.fmt_short())),
            Err(_) => bail!(
                "connect to {}: timed out after {}s",
                id.fmt_short(),
                CONNECT_TIMEOUT.as_secs()
            ),
        }
    }

    /// Dials the host of `ticket`: its relay URL and direct addresses (v2
    /// ticket) are handed to iroh together with the id, so it can start via
    /// the relay at once and hole-punch in parallel; the n0 DNS/pkarr lookup
    /// still runs as a fallback (iroh starts it whenever no path is selected).
    /// Logs every attempt at info level.
    pub async fn dial_host(&self, ticket: &Ticket, alpn: &[u8]) -> Result<Connection> {
        self.dial_host_alpns(ticket, alpn, Vec::new()).await
    }

    /// [`Inner::dial_host`] offering additional ALPNs (see [`Inner::connect_alpns`]).
    pub async fn dial_host_alpns(
        &self,
        ticket: &Ticket,
        alpn: &[u8],
        additional: Vec<Vec<u8>>,
    ) -> Result<Connection> {
        let addr = ticket.endpoint_addr().map_err(|e| anyhow!(e))?;
        let id = addr.id;
        let host = id.fmt_short().to_string();
        let alpn_s = String::from_utf8_lossy(alpn).to_string();
        let relay = addr
            .relay_urls()
            .next()
            .map(|u| u.to_string())
            .unwrap_or_else(|| "-".into());
        let direct = addr.ip_addrs().count();
        tracing::info!(
            %host,
            alpn = %alpn_s,
            %relay,
            direct,
            online = self.endpoint.home_relay_status().get().iter().any(|r| r.is_connected()),
            "dialing host (ticket addresses{}; n0 DNS/pkarr lookup as fallback)",
            if relay == "-" && direct == 0 { ": none" } else { "" }
        );
        let started = Instant::now();
        match self.connect_alpns(addr, alpn, additional).await {
            Ok(conn) => {
                tracing::info!(
                    %host,
                    alpn = %alpn_s,
                    path = ?conn_path(&conn),
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "connected to host"
                );
                // v0.4: still on the relay after 10 s → re-add ticket + mDNS addresses once.
                crate::netdiag::spawn_repunch(self, ticket, &conn);
                Ok(conn)
            }
            Err(e) => {
                let known = match self.endpoint.remote_info(id).await {
                    Some(info) => {
                        let v: Vec<String> = info
                            .addrs()
                            .map(|a| format!("{:?} ({:?})", a.addr(), a.usage()))
                            .collect();
                        if v.is_empty() {
                            "none".to_string()
                        } else {
                            v.join(", ")
                        }
                    }
                    None => "none".to_string(),
                };
                tracing::info!(
                    %host,
                    alpn = %alpn_s,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    known_paths = %known,
                    "dial failed: {e:#}"
                );
                // Diagnose discovery separately, without delaying the retry.
                tokio::spawn(log_lookup(self.endpoint.clone(), id));
                Err(e)
            }
        }
    }
}

/// Logs home relay connection changes at info level. iroh itself logs
/// "home is now relay …" when it *selects* a relay (state `Connecting`) and
/// relay dial failures only at debug level, so without this a host that never
/// reached its relay looks healthy in the logs.
async fn log_relay_status(endpoint: Endpoint) {
    let mut w = endpoint.home_relay_status();
    drop(endpoint); // the watcher alone; it disconnects when the endpoint is dropped
    let mut last: Vec<(String, bool)> = Vec::new();
    loop {
        let cur = w.get();
        let now: Vec<(String, bool)> = cur
            .iter()
            .map(|r| (r.url().to_string(), r.is_connected()))
            .collect();
        if now != last {
            for r in &cur {
                if r.is_connected() {
                    tracing::info!(relay = %r.url(), "home relay connected");
                } else if let Some(reason) = r.auth_denied_reason() {
                    tracing::warn!(relay = %r.url(), "home relay refused this endpoint: {reason}");
                } else if let Some(e) = r.last_error() {
                    tracing::warn!(relay = %r.url(), "home relay not connected: {e:#}");
                } else {
                    tracing::info!(relay = %r.url(), "home relay connecting");
                }
            }
            last = now;
        }
        if w.updated().await.is_err() {
            return;
        }
    }
}

/// Connect timeout per dial attempt. Generous on purpose: the first attempt
/// may need a DNS/pkarr lookup (staggered, several seconds), a fresh
/// connection to the host's relay (iroh allows 10 s) and the QUIC handshake
/// over it, on a busy machine.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(45);

/// Runs one n0 address lookup (DNS + pkarr, plus the in-memory table) for
/// `id` and logs what it found, for diagnosing failed dials.
async fn log_lookup(endpoint: Endpoint, id: EndpointId) {
    use n0_future::StreamExt;
    let host = id.fmt_short().to_string();
    let Ok(services) = endpoint.address_lookup() else {
        return;
    };
    let mut stream = services.resolve(id);
    let res = tokio::time::timeout(Duration::from_secs(15), async {
        let mut found = Vec::new();
        let mut errors = Vec::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok(Ok(item)) => {
                    let a = item.to_endpoint_addr();
                    found.push(format!(
                        "{}: relay={} direct={}",
                        item.provenance(),
                        a.relay_urls()
                            .next()
                            .map(|u| u.to_string())
                            .unwrap_or_else(|| "-".into()),
                        a.ip_addrs().count()
                    ));
                }
                Ok(Err(e)) => errors.push(format!("{e:#}")),
                Err(e) => errors.push(format!("{e:#}")),
            }
        }
        (found, errors)
    })
    .await;
    match res {
        Ok((found, _)) if !found.is_empty() => {
            tracing::info!(%host, "address lookup: {}", found.join("; "));
        }
        Ok((_, errors)) => {
            tracing::info!(%host, "address lookup found nothing: {}", errors.join("; "));
        }
        Err(_) => tracing::info!(%host, "address lookup: no answer within 15s"),
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
    /// hosts ([`HostConfig::bind_port`]). Fails if the IPv4 port is taken
    /// (see [`Node::start_host`] for the variant that falls back).
    pub async fn start_with_port(cfg: NodeConfig, port: Option<u16>) -> Result<Node> {
        Node::start_inner(cfg, port, Vec::new()).await
    }

    /// v0.4 host start: binds `port` or, when `None`,
    /// [`crate::net::DEFAULT_HOST_PORT`] (41820, the port the firewall rule,
    /// port mapping and router forwards expect). If that port cannot be bound
    /// (another program or a second Toss holds it) the node starts on a random
    /// port instead and [`Node::diagnostics`] explains why. Pass
    /// `host_cfg.bind_port`.
    pub async fn start_host(cfg: NodeConfig, port: Option<u16>) -> Result<Node> {
        let port = port.unwrap_or(crate::net::DEFAULT_HOST_PORT);
        match Node::start_inner(cfg.clone(), Some(port), Vec::new()).await {
            Ok(node) => Ok(node),
            Err(e) => {
                tracing::warn!("cannot bind UDP port {port}, using a random port: {e:#}");
                let note = format!(
                    "Cổng UDP {port} đang bị chương trình khác dùng — Toss chạy trên cổng ngẫu \
                     nhiên nên mở cổng/port forward không khớp; tắt chương trình kia rồi mở lại Toss"
                );
                Node::start_inner(cfg, None, vec![note]).await
            }
        }
    }

    async fn start_inner(
        cfg: NodeConfig,
        port: Option<u16>,
        mut net_notes: Vec<String>,
    ) -> Result<Node> {
        std::fs::create_dir_all(&cfg.data_dir)
            .with_context(|| format!("create data dir {}", cfg.data_dir.display()))?;
        let secret = load_or_create_secret_key(&cfg.data_dir)?;
        let lookup = MemoryLookup::new();
        let mut builder = Endpoint::builder(presets::N0)
            .secret_key(secret.clone())
            .address_lookup(lookup.clone())
            .transport_config(crate::tuning::transport_config(&cfg.net.tuning))
            .portmapper_config(if cfg.net.portmap {
                PortmapperConfig::default()
            } else {
                PortmapperConfig::Disabled
            })
            .alpns(vec![ALPN_PEER.to_vec()]);
        if !cfg.net.relay_urls.is_empty() {
            let (mode, bad) = crate::netdiag::relay_mode(&cfg.net.relay_urls);
            for b in bad {
                tracing::warn!(url = %b, "ignoring bad relay URL");
                net_notes.push(format!("Địa chỉ relay riêng không hợp lệ: {b}"));
            }
            builder = builder.relay_mode(mode);
        }
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
        let mdns = if cfg.net.mdns {
            match crate::mdns::start(&endpoint) {
                Ok(m) => Some(m),
                Err(e) => {
                    tracing::warn!("mDNS unavailable, same-LAN discovery off: {e}");
                    net_notes.push(
                        "mDNS không chạy được (mạng hoặc máy chặn multicast) — máy cùng mạng LAN \
                         vẫn nối qua địa chỉ trong link"
                            .into(),
                    );
                    None
                }
            }
        } else {
            None
        };
        tracing::info!(
            id = %endpoint.id(),
            port = ?port,
            mdns = mdns.is_some(),
            portmap = cfg.net.portmap,
            own_relays = cfg.net.relay_urls.len(),
            "toss node started"
        );
        let (events, _) = broadcast::channel(1024);
        let inner = Arc::new(Inner {
            endpoint: endpoint.clone(),
            secret,
            jobs: JobStore::open(&cfg.data_dir),
            contacts: ContactBook::open(&cfg.data_dir),
            invites: InviteStore::open(&cfg.data_dir),
            peer: PeerState::new(&cfg.data_dir),
            store: Arc::new(TransferStore::open(&cfg.data_dir)),
            lan_blocked: AtomicBool::new(false),
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
            mdns,
            net_notes,
        });
        tokio::spawn(log_relay_status(endpoint.clone()));
        tokio::spawn(accept_loop(endpoint, Arc::downgrade(&inner)));
        // v0.5: restore persisted transfers, presence pings, Local Network probe.
        client::restore_all(&inner);
        crate::peer::spawn_presence(&inner);
        crate::lanprobe::spawn(&inner);
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

    /// This node's current home relay + direct addresses (what tickets carry).
    pub fn host_addr(&self) -> HostAddr {
        self.inner.host_addr()
    }

    /// Waits until [`Node::host_addr`] differs from `prev` and returns the new
    /// value; `None` when the endpoint is gone.
    pub async fn host_addr_changed(&self, prev: &HostAddr) -> Option<HostAddr> {
        let mut w = self.inner.endpoint.watch_addr();
        loop {
            let cur = HostAddr::from_endpoint_addr(&w.get());
            if &cur != prev {
                return Some(cur);
            }
            w.updated().await.ok()?;
        }
    }

    /// Waits until the endpoint is connected to its home relay (so tickets
    /// carry a relay URL), at most `timeout`. Returns whether it is online.
    pub async fn wait_online(&self, timeout: Duration) -> bool {
        tokio::time::timeout(timeout, self.inner.endpoint.online())
            .await
            .is_ok()
    }

    /// Adds addressing info for a peer (direct addresses / relay) so it can be
    /// dialed without discovery. Useful on a LAN and in tests.
    pub fn add_peer_addr(&self, addr: EndpointAddr) {
        self.inner.lookup.add_endpoint_info(addr);
    }

    /// v0.4: this node's network situation (relay, addresses, port mapping, NAT, likelihood of direct).
    pub async fn diagnostics(&self) -> crate::net::NetDiag {
        crate::netdiag::diagnostics(&self.inner).await
    }

    /// v0.4: dials the host of `ticket` (ALPN xfer, no transfer), waits briefly for a direct
    /// path and reports path / RTT / reason. Does not touch any job.
    pub async fn test_connect(&self, ticket: &Ticket) -> crate::net::ConnectReport {
        crate::netdiag::test_connect(&self.inner, ticket).await
    }

    /// v0.4: waits until the first net report (NAT / UDP / public address
    /// probe against the relays) is in, at most `timeout`, so that
    /// [`Node::diagnostics`] is not `unknown`. Returns whether one is available.
    pub async fn wait_net_report(&self, timeout: Duration) -> bool {
        crate::netdiag::wait_net_report(&self.inner, timeout).await
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
                self.inner.endpoint.set_alpns(vec![
                    ALPN_XFER2.to_vec(),
                    ALPN_XFER.to_vec(),
                    ALPN_CTL.to_vec(),
                    ALPN_PEER.to_vec(),
                ]);
                tracing::info!("host mode on");
                self.inner.emit_host_status();
            }
            None => {
                self.inner.endpoint.set_alpns(vec![ALPN_PEER.to_vec()]);
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
        // v0.4: also starts hashing the job's source folder in the background,
        // so the manifest is ready when the editor clicks the link.
        host::issue_ticket_and_prehash(&self.inner, &rt, spec)
    }

    /// Creates an admin ticket (control channel access). Errors if host mode is off.
    pub fn create_admin_ticket(&self, days: u32) -> Result<Ticket> {
        if self.inner.host.read().is_none() {
            bail!("host mode is off");
        }
        host::issue_admin_ticket(
            &self.inner.secret,
            &self.inner.jobs,
            days,
            &self.inner.host_addr(),
        )
    }

    /// Re-signs the ticket of a stored job with the host's *current* relay and
    /// direct addresses (same id, job and permissions as when created; the
    /// string only differs if the addresses changed).
    pub fn job_ticket(&self, id: [u8; 16]) -> Option<Ticket> {
        let mut body = self.inner.jobs.body(id, *self.endpoint_id().as_bytes())?;
        body.set_host_addr(&self.inner.host_addr());
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

    /// Removes finished (done/error/cancelled) transfers from the list
    /// (and from `transfers.json`).
    pub fn clear_finished(&self) {
        let mut gone = Vec::new();
        self.inner.transfers.lock().retain(|id, e| {
            let keep = !e.tracker.status().is_final();
            if !keep {
                gone.push(id.clone());
            }
            keep
        });
        self.inner.store.remove(&gone);
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

    /// Pauses all client transfers (keeping `.part` files) and closes the
    /// endpoint. `transfers.json` keeps their pre-shutdown status, so running
    /// ones resume on the next start.
    pub async fn shutdown(self) {
        self.inner.store.freeze();
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

/// v0.5 "Danh bạ máy" (contracts/contacts-v0.5.md §3): pair devices once,
/// then push files to them by id; pushes from contacts land in
/// `<inbox_dir>/<contact name>/<job>/`.
impl Node {
    /// Where pushes from contacts land (default `~/Toss`). Takes effect for
    /// the next push.
    pub fn set_inbox_dir(&self, dir: PathBuf) {
        *self.inner.peer.inbox_dir.write() = dir;
    }

    /// Current inbox folder (see [`Node::set_inbox_dir`]).
    pub fn inbox_dir(&self) -> PathBuf {
        self.inner.peer.inbox_dir.read().clone()
    }

    /// All paired devices, oldest first, with their presence.
    pub fn contacts(&self) -> Vec<Contact> {
        self.inner.contacts.list()
    }

    /// Creates a pairing invite valid for `days` (0 = never expires), usable
    /// by any number of devices until it expires or [`Node::revoke_invites`].
    /// It carries this node's current relay + direct addresses.
    pub fn create_invite(&self, days: u32, page_host: &str) -> Result<InviteInfo> {
        let inv = crate::contacts::issue_invite(
            &self.inner.invites,
            &self.inner.secret,
            &self.inner.cfg.device_name,
            days,
            &self.inner.host_addr(),
        )?;
        tracing::info!(
            exp = inv.body.exp,
            "pairing invite issued ({})",
            inv.host_addr().summary()
        );
        Ok(inv.info(page_host))
    }

    /// Revokes every invite this device issued (existing contacts stay).
    pub fn revoke_invites(&self) -> Result<()> {
        self.inner.invites.revoke_all()
    }

    /// Parses and checks an invite (`tossinv1…`, `toss://i/…` or a web link;
    /// tolerant like `Ticket::from_str`): signature and expiry. `web_url` uses
    /// [`crate::contacts::DEFAULT_PAGE_HOST`].
    pub fn parse_invite(text: &str) -> Result<InviteInfo> {
        let inv: Invite = text.parse()?;
        inv.verify()?;
        Ok(inv.info(crate::contacts::DEFAULT_PAGE_HOST))
    }

    /// Pairs with the device that issued `text`: both sides add each other as
    /// contacts (and emit `ContactsChanged`). Returns the new contact.
    pub async fn accept_invite(&self, text: &str) -> Result<Contact> {
        crate::peer::pair(&self.inner, text).await
    }

    /// Renames a contact locally.
    pub fn rename_contact(&self, id: &str, name: &str) -> Result<()> {
        let name = crate::util::clean_text(name.trim(), crate::contacts::MAX_NAME);
        if name.trim().is_empty() {
            bail!("tên máy không được để trống");
        }
        self.inner.contacts.update(id, |c| c.name = name)?;
        self.inner.emit(TransferEvent::ContactsChanged {});
        Ok(())
    }

    /// Accept pushes from this contact without asking (`true`) or queue them
    /// for [`Node::answer_push`].
    pub fn set_contact_auto_accept(&self, id: &str, on: bool) -> Result<()> {
        self.inner.contacts.update(id, |c| c.auto_accept = on)?;
        self.inner.emit(TransferEvent::ContactsChanged {});
        Ok(())
    }

    /// Removes a contact; its pushes from this device that are not finished
    /// are cancelled and forgotten, pushes from it waiting for an answer are
    /// declined. It can no longer reach this device until paired again.
    pub fn remove_contact(&self, id: &str) -> Result<()> {
        crate::peer::remove_contact(&self.inner, id)
    }

    /// Sends files/folders to a contact. Starts at once when it is online,
    /// else waits ("Chờ <name> online") until presence sees it.
    pub async fn send_to_contact(&self, id: &str, paths: Vec<PathBuf>) -> Result<crate::Transfer> {
        client::start_push(&self.inner, id, paths).await
    }

    /// Pushes from contacts waiting for an answer, oldest first.
    pub fn incoming_pushes(&self) -> Vec<IncomingPush> {
        self.inner.peer.incoming()
    }

    /// Accepts or declines a waiting push.
    pub fn answer_push(&self, push_id: &str, accept: bool) -> Result<()> {
        self.inner.peer.answer(push_id, accept)
    }

    /// Pings every contact now and waits for the answers (≤ ~15 s).
    pub async fn refresh_presence(&self) {
        crate::peer::ping_all(&self.inner).await;
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
                    tracing::info!("incoming connection failed (alpn): {e:#}");
                    return;
                }
            };
            let alpn_s = String::from_utf8_lossy(&alpn).to_string();
            let conn = match accepting.await {
                Ok(c) => c,
                Err(e) => {
                    tracing::info!(alpn = %alpn_s, "incoming connection failed (handshake): {e:#}");
                    return;
                }
            };
            let remote = conn.remote_id().fmt_short().to_string();
            if alpn == ALPN_PEER {
                // Presence pings arrive every minute per contact: keep them out of info logs
                // (peer.rs logs pairings, pushes and denials at info).
                tracing::debug!(%remote, path = ?conn_path(&conn), "incoming peer connection");
            } else {
                tracing::info!(
                    %remote,
                    alpn = %alpn_s,
                    path = ?conn_path(&conn),
                    "incoming connection accepted"
                );
            }
            let Some(inner) = inner.upgrade() else { return };
            if alpn == ALPN_PEER {
                // v0.5: contacts / pairing, on every node (host mode or not).
                crate::peer::handle_peer(inner, conn).await;
                return;
            }
            if inner.host.read().is_none() {
                tracing::info!(%remote, alpn = %alpn_s, "denied: host mode off");
                conn.close(1u32.into(), b"host mode off");
                return;
            }
            if alpn == ALPN_XFER || alpn == ALPN_XFER2 {
                // xfer/2 (v0.4) vs xfer/1: host::handle_xfer branches on `conn.alpn()`.
                host::handle_xfer(inner, conn).await;
            } else if alpn == ALPN_CTL {
                crate::ctl::handle_ctl(inner, conn).await;
            } else {
                tracing::info!(%remote, alpn = %alpn_s, "denied: unknown alpn");
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
/// validates `spec` against `cfg`, signs and appends to `jobs.json`. `addr` is
/// the running host's last known relay/direct addresses (the CLI persists them
/// in `cli-host.json`); pass `HostAddr::default()` if unknown.
pub fn issue_ticket_offline(
    data_dir: &Path,
    cfg: &HostConfig,
    spec: TicketSpec,
    addr: &HostAddr,
) -> Result<Ticket> {
    let secret = load_or_create_secret_key(data_dir)?;
    let rt = HostRuntime::new(cfg.clone())?;
    host::issue_ticket(&secret, &JobStore::open(data_dir), &rt, spec, addr)
}

/// Creates an admin ticket without a running node (see [`issue_ticket_offline`]).
pub fn issue_admin_ticket_offline(data_dir: &Path, days: u32, addr: &HostAddr) -> Result<Ticket> {
    let secret = load_or_create_secret_key(data_dir)?;
    host::issue_admin_ticket(&secret, &JobStore::open(data_dir), days, addr)
}
