//! Toss <-> LocalSend bridge (contract `contracts/nearby-v0.8.md` §2).
//!
//! Built on the vendored, unmodified LocalSend core (`third_party/localsend-core`):
//! an HTTPS v2 server (persisted self-signed cert) that receives into the Toss inbox,
//! multicast + subnet-scan discovery, sending files/folders/text to any LocalSend app,
//! and the LocalSend browser download page ("web share").
pub use localsend;

mod files;
mod identity;
mod recv;
mod send;
mod web;

use anyhow::{anyhow, Result};
use localsend::discovery::{
    self, DeviceIdentity, DiscoveryConfig, DiscoveryEvent, DiscoveryHandle, HttpChannel,
    StatefulDevice,
};
use localsend::http::server::v2::ServerEventV2;
use localsend::http::server::web::WebConfig;
use localsend::http::server::{start_with_port, ServerConfigV2, ServerHandle, TlsConfig};
use localsend::http::state::ClientInfo;
use localsend::model::discovery::{DeviceType, ProtocolType, PROTOCOL_VERSION_V2};
use localsend::multicast::{MulticastDevice, DEFAULT_MULTICAST_GROUP, DEFAULT_MULTICAST_GROUP_V6};
use localsend::util::interface::{local_interface_addresses, InterfaceFilter};
use serde::Serialize;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// The LocalSend default port.
pub const DEFAULT_PORT: u16 = 53317;
/// Fallback ports tried (in order) when the preferred one is busy, before an OS-picked port.
pub const FALLBACK_PORTS: std::ops::RangeInclusive<u16> = 53318..=53327;
/// The device model Toss announces; `LsDevice::is_toss` matches on it.
pub const TOSS_MODEL: &str = "Toss";

const EVENT_CAPACITY: usize = 512;
/// Devices not confirmed for this long are no longer listed.
const STALE_AFTER: Duration = Duration::from_secs(10 * 60);
/// Background refresh (announce + re-probe known devices).
const REFRESH_EVERY: Duration = Duration::from_secs(120);

pub struct LsConfig {
    /// Device name.
    pub alias: String,
    /// `<toss data_dir>`; cert persisted at `ls/cert.pem` + `ls/key.pem`.
    pub data_dir: PathBuf,
    /// Inbox root (same as the Toss inbox).
    pub dest_dir: PathBuf,
    /// Preferred port, normally 53317; if busy 53318..=53327 are tried, then 0.
    /// 0 = let the OS pick right away.
    pub port: u16,
}

/// Knobs not in the contract (tests, diagnostics). `LsBridge::start` uses the defaults.
#[derive(Clone, Debug)]
pub struct LsOptions {
    /// Join the LocalSend multicast group (announce + hear announcements).
    pub multicast: bool,
    /// UDP multicast port (LocalSend: 53317).
    pub multicast_port: u16,
    /// Scan the /24 of every local IPv4 on start (when nothing answers) and on `rescan`.
    pub subnet_scan: bool,
    /// TCP ports probed by the subnet scan (own port is added on `rescan`).
    pub scan_ports: Vec<u16>,
    /// Addresses whose /24 is scanned; `None` = the local interfaces (tests use loopback).
    pub scan_subnets: Option<Vec<Ipv4Addr>>,
    /// An `Incoming` not answered within this time is declined.
    pub answer_timeout: Duration,
}

impl Default for LsOptions {
    fn default() -> Self {
        Self {
            multicast: true,
            multicast_port: localsend::multicast::DEFAULT_PORT,
            subnet_scan: true,
            scan_ports: vec![DEFAULT_PORT],
            scan_subnets: None,
            answer_timeout: Duration::from_secs(60),
        }
    }
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct LsDevice {
    pub fingerprint: String,
    pub alias: String,
    pub model: Option<String>,
    /// mobile|desktop|web|headless|server
    pub kind: String,
    pub ip: String,
    pub port: u16,
    pub https: bool,
    /// model == "Toss"
    pub is_toss: bool,
    /// unix seconds
    pub last_seen: u64,
}

#[derive(Serialize, Clone, Debug)]
pub struct WebShare {
    /// `http://<lan-ip>:<port>` for every private LAN IPv4.
    pub urls: Vec<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LsEvent {
    DevicesChanged,
    Incoming {
        req_id: String,
        /// mTLS-verified when present.
        fingerprint: String,
        alias: String,
        model: Option<String>,
        ip: String,
        files: u32,
        bytes: u64,
        /// ≤ 5
        names: Vec<String>,
        /// Always `None` here: text messages arrive as [`LsEvent::Text`] without asking.
        text: Option<String>,
    },
    Started {
        id: String,
        outgoing: bool,
        peer: String,
        job: String,
        files: u32,
        bytes: u64,
    },
    Progress {
        id: String,
        bytes_done: u64,
        bytes_total: u64,
        files_done: u32,
        files_total: u32,
        speed_bps: u64,
    },
    Done {
        id: String,
        dest: Option<PathBuf>,
        files: u32,
        bytes: u64,
    },
    /// Vietnamese message. Also used for an `Incoming` that ended without an answer
    /// (`id` = `req_id`: timed out / sender gave up).
    Error {
        id: String,
        msg: String,
    },
    /// Received text message (no file saved).
    Text {
        fingerprint: String,
        alias: String,
        text: String,
    },
    /// fallback=true when the preferred port was busy.
    Port {
        port: u16,
        fallback: bool,
    },
}

/// Clone (Arc inside).
#[derive(Clone)]
pub struct LsBridge {
    inner: Arc<Inner>,
}

struct ServerSlot {
    handle: Arc<ServerHandle>,
    stop: oneshot::Sender<()>,
}

struct DiscoverySlot {
    handle: Arc<DiscoveryHandle>,
    stop: oneshot::Sender<()>,
}

pub(crate) struct Inner {
    pub(crate) rt: tokio::runtime::Handle,
    pub(crate) ident: identity::Identity,
    pub(crate) opts: LsOptions,
    preferred_port: u16,
    pub(crate) alias: Mutex<String>,
    pub(crate) dest: Mutex<PathBuf>,
    pub(crate) port: AtomicU16,
    ev: mpsc::Sender<LsEvent>,
    srv_tx: mpsc::Sender<ServerEventV2>,
    /// Serializes server (re)starts.
    server: tokio::sync::Mutex<Option<ServerSlot>>,
    /// Fast access to the running server for session cancels.
    pub(crate) server_handle: RwLock<Option<Arc<ServerHandle>>>,
    disc_slot: tokio::sync::Mutex<Option<DiscoverySlot>>,
    disc: RwLock<Option<Arc<DiscoveryHandle>>>,
    pub(crate) pending: Mutex<HashMap<String, recv::Pending>>,
    pub(crate) recv: Mutex<HashMap<String, recv::RecvSession>>,
    pub(crate) sends: Mutex<HashMap<String, send::SendCtl>>,
    pub(crate) web: tokio::sync::Mutex<Option<web::WebSlot>>,
    /// fingerprint → (alias, ip, port) last announced to the UI.
    dev_cache: Mutex<HashMap<String, (String, String, u16)>>,
    /// set_alias while a receive was running: restart the server when idle.
    pub(crate) restart_when_idle: AtomicBool,
    pub(crate) stopped: CancellationToken,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn kind_of(t: &Option<DeviceType>) -> &'static str {
    match t {
        Some(DeviceType::Mobile) => "mobile",
        Some(DeviceType::Web) => "web",
        Some(DeviceType::Headless) => "headless",
        Some(DeviceType::Server) => "server",
        Some(DeviceType::Desktop) | None => "desktop",
    }
}

impl Inner {
    /// Never blocks: progress/device events are dropped when the consumer lags,
    /// everything else is delivered from a helper task.
    pub(crate) fn emit(&self, ev: LsEvent) {
        match self.ev.try_send(ev) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(ev)) => {
                if matches!(ev, LsEvent::Progress { .. } | LsEvent::DevicesChanged) {
                    return;
                }
                let tx = self.ev.clone();
                self.rt.spawn(async move {
                    let _ = tokio::time::timeout(Duration::from_secs(60), tx.send(ev)).await;
                });
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }

    pub(crate) fn alias(&self) -> String {
        lock(&self.alias).clone()
    }

    pub(crate) fn client_info(&self) -> ClientInfo {
        ClientInfo {
            alias: self.alias(),
            version: PROTOCOL_VERSION_V2.into(),
            device_model: Some(TOSS_MODEL.into()),
            device_type: Some(DeviceType::Desktop),
            token: self.ident.fingerprint.clone(),
        }
    }

    pub(crate) fn register_dto(&self) -> localsend::http::dto_v2::RegisterDtoV2 {
        localsend::http::dto_v2::RegisterDtoV2 {
            alias: self.alias(),
            version: PROTOCOL_VERSION_V2.into(),
            device_model: Some(TOSS_MODEL.into()),
            device_type: Some(DeviceType::Desktop),
            fingerprint: self.ident.fingerprint.clone(),
            port: self.port.load(Ordering::Relaxed),
            protocol: ProtocolType::Https,
            download: false,
        }
    }

    pub(crate) fn discovery(&self) -> Option<Arc<DiscoveryHandle>> {
        self.disc.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Binds the HTTPS server: preferred port, then the fallback range, then 0.
    async fn bind_server(&self) -> Result<(ServerSlot, u16, bool)> {
        let pref = self.preferred_port;
        let mut ports = vec![pref];
        if pref != 0 {
            ports.extend(FALLBACK_PORTS.filter(|p| *p != pref));
            ports.push(0);
        }
        let mut last_err = None;
        for p in ports {
            let (stop, stop_rx) = oneshot::channel();
            let res = start_with_port(
                p,
                Some(TlsConfig {
                    cert: self.ident.cert_pem.clone(),
                    private_key: self.ident.key_pem.clone(),
                }),
                self.client_info(),
                None,
                Some(ServerConfigV2 {
                    pin: None,
                    verify_checksums: true,
                    event_tx: self.srv_tx.clone(),
                }),
                WebConfig::default(),
                stop_rx,
            )
            .await;
            match res {
                Ok(h) => {
                    let port = h.port();
                    let fallback = pref != 0 && port != pref;
                    return Ok((
                        ServerSlot {
                            handle: Arc::new(h),
                            stop,
                        },
                        port,
                        fallback,
                    ));
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(anyhow!(
            "Không mở được cổng LocalSend: {}",
            last_err.map(|e| format!("{e:#}")).unwrap_or_default()
        ))
    }

    /// (Re)starts the server; restarts discovery when the port changed.
    pub(crate) async fn restart_server(self: &Arc<Self>) -> Result<()> {
        if self.stopped.is_cancelled() {
            return Ok(());
        }
        let mut slot = self.server.lock().await;
        if let Some(old) = slot.take() {
            let _ = old.stop.send(());
            let _ = tokio::time::timeout(Duration::from_secs(3), old.handle.wait_stopped()).await;
        }
        let (new, port, fallback) = self.bind_server().await?;
        let changed = self.port.swap(port, Ordering::Relaxed) != port;
        *self
            .server_handle
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(new.handle.clone());
        *slot = Some(new);
        drop(slot);
        if changed {
            self.emit(LsEvent::Port { port, fallback });
            self.restart_discovery().await;
        }
        Ok(())
    }

    /// Restarts the server once no receive is pending/running (after `set_alias`).
    pub(crate) fn maybe_deferred_restart(self: &Arc<Self>) {
        if !self.restart_when_idle.load(Ordering::Relaxed) {
            return;
        }
        if !lock(&self.pending).is_empty() || !lock(&self.recv).is_empty() {
            return;
        }
        if self.restart_when_idle.swap(false, Ordering::Relaxed) {
            let me = self.clone();
            self.rt.spawn(async move {
                if let Err(e) = me.restart_server().await {
                    tracing::warn!("LocalSend server restart failed: {e:#}");
                }
            });
        }
    }

    /// (Re)starts discovery with the current alias and port.
    pub(crate) async fn restart_discovery(self: &Arc<Self>) {
        if self.stopped.is_cancelled() {
            return;
        }
        let mut slot = self.disc_slot.lock().await;
        if let Some(old) = slot.take() {
            let _ = old.stop.send(());
            let _ = tokio::time::timeout(Duration::from_secs(3), old.handle.wait_stopped()).await;
        }
        let (ev_tx, ev_rx) = mpsc::channel(256);
        let (stop, stop_rx) = oneshot::channel();
        let filter = if self.opts.multicast {
            InterfaceFilter::default()
        } else {
            // A whitelist nothing matches: no multicast socket, HTTP discovery only.
            InterfaceFilter {
                whitelist: Some(vec![]),
                blacklist: None,
            }
        };
        let cfg = DiscoveryConfig {
            group: DEFAULT_MULTICAST_GROUP,
            group_v6: Some(DEFAULT_MULTICAST_GROUP_V6),
            port: self.opts.multicast_port,
            interface_filter: filter,
            device: MulticastDevice {
                alias: self.alias(),
                version: PROTOCOL_VERSION_V2.into(),
                device_model: Some(TOSS_MODEL.into()),
                device_type: Some(DeviceType::Desktop),
                fingerprint: self.ident.fingerprint.clone(),
                port: self.port.load(Ordering::Relaxed),
                protocol: ProtocolType::Https,
                download: false,
            },
            identity: DeviceIdentity {
                cert_pem: self.ident.cert_pem.clone(),
                private_key_pem: self.ident.key_pem.clone(),
            },
            timeout: discovery::DEFAULT_DISCOVERY_TIMEOUT,
            event_tx: Some(ev_tx),
        };
        let handle = Arc::new(discovery::start(cfg, stop_rx).await);
        if let Some(e) = handle.multicast_error() {
            if self.opts.multicast {
                tracing::warn!("LocalSend multicast unavailable: {e:#}");
            }
        }
        // Keep the devices known so far: re-confirm them on the new instance.
        let known = self
            .discovery()
            .map(|d| known_channels(&d.devices()))
            .unwrap_or_default();
        *self.disc.write().unwrap_or_else(|e| e.into_inner()) = Some(handle.clone());
        *slot = Some(DiscoverySlot {
            handle: handle.clone(),
            stop,
        });
        drop(slot);
        self.rt
            .spawn(self.clone().discovery_loop(ev_rx, handle.clone()));
        let me = self.clone();
        self.rt.spawn(async move {
            let ips = me.scan_ips();
            let port = me.opts.scan_ports.first().copied().unwrap_or(DEFAULT_PORT);
            let run = handle.discover_staged(
                known,
                ips,
                port,
                ProtocolType::Https,
                Duration::from_secs(3),
            );
            tokio::select! {
                _ = run => {}
                _ = me.stopped.cancelled() => {}
            }
            me.devices_maybe_changed();
        });
    }

    /// Boxed, to break the async type cycle restart → loop → restart.
    fn restart_discovery_boxed(
        self: Arc<Self>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(async move { self.restart_discovery().await })
    }

    async fn discovery_loop(
        self: Arc<Self>,
        mut rx: mpsc::Receiver<DiscoveryEvent>,
        handle: Arc<DiscoveryHandle>,
    ) {
        loop {
            let ev = tokio::select! {
                ev = rx.recv() => ev,
                _ = self.stopped.cancelled() => return,
            };
            match ev {
                None => return,
                Some(DiscoveryEvent::Discovered { .. }) | Some(DiscoveryEvent::Updated { .. }) => {
                    self.devices_maybe_changed();
                }
                Some(DiscoveryEvent::MulticastFailed) => {
                    // Only restart the instance this loop belongs to.
                    let current = self.discovery().is_some_and(|d| Arc::ptr_eq(&d, &handle));
                    if current {
                        let me = self.clone();
                        self.rt.spawn(async move {
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            me.restart_discovery_boxed().await;
                        });
                    }
                    return;
                }
            }
        }
    }

    /// Emits `DevicesChanged` when the visible list differs from the last one.
    pub(crate) fn devices_maybe_changed(&self) {
        let now: HashMap<String, (String, String, u16)> = self
            .devices()
            .into_iter()
            .map(|d| (d.fingerprint, (d.alias, d.ip, d.port)))
            .collect();
        let changed = {
            let mut cache = lock(&self.dev_cache);
            if *cache != now {
                *cache = now;
                true
            } else {
                false
            }
        };
        if changed {
            self.emit(LsEvent::DevicesChanged);
        }
    }

    pub(crate) fn devices(&self) -> Vec<LsDevice> {
        let Some(d) = self.discovery() else {
            return vec![];
        };
        let own = &self.ident.fingerprint;
        let now = SystemTime::now();
        let mut out = Vec::new();
        for s in d.devices() {
            if s.device.fingerprint.eq_ignore_ascii_case(own) {
                continue;
            }
            let last = s.logs.last().map(|l| l.timestamp).unwrap_or(UNIX_EPOCH);
            if now.duration_since(last).unwrap_or_default() > STALE_AFTER {
                continue;
            }
            let Some(http) = s.get_best_channel().and_then(|c| c.http()) else {
                continue;
            };
            let model = s.device.device_model.clone();
            out.push(LsDevice {
                fingerprint: s.device.fingerprint.clone(),
                alias: files::single_line(&s.device.alias),
                is_toss: model.as_deref() == Some(TOSS_MODEL),
                model,
                kind: kind_of(&s.device.device_type).into(),
                ip: http.host.clone(),
                port: http.port,
                https: http.protocol == ProtocolType::Https,
                last_seen: last
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            });
        }
        out
    }

    pub(crate) fn device(&self, fingerprint: &str) -> Option<StatefulDevice> {
        let d = self.discovery()?;
        d.device_by_fingerprint(fingerprint).or_else(|| {
            d.devices()
                .into_iter()
                .find(|s| s.device.fingerprint.eq_ignore_ascii_case(fingerprint))
        })
    }

    fn scan_ips(&self) -> Vec<Ipv4Addr> {
        match (&self.opts.scan_subnets, self.opts.subnet_scan) {
            (_, false) => vec![],
            (Some(ips), true) => ips.clone(),
            (None, true) => interface_ips(),
        }
    }

    async fn rescan(self: &Arc<Self>) {
        let Some(d) = self.discovery() else { return };
        let known = known_channels(&d.devices());
        let mut ports = self.opts.scan_ports.clone();
        let own = self.port.load(Ordering::Relaxed);
        if !ports.contains(&own) {
            ports.push(own);
        }
        let ips = self.scan_ips();
        let scans = futures_util::future::join_all(ips.into_iter().map(|ip| {
            let d = d.clone();
            let ports = ports.clone();
            async move {
                for p in ports {
                    let _ = d.scan_subnet(ip, p, ProtocolType::Https).await;
                }
            }
        }));
        let work = async {
            let _ = tokio::join!(d.announce(), d.discover_known_http_channels(known), scans);
        };
        tokio::select! {
            _ = work => {}
            _ = self.stopped.cancelled() => {}
        }
        self.devices_maybe_changed();
    }

    async fn refresh_loop(self: Arc<Self>) {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(REFRESH_EVERY) => {}
                _ = self.stopped.cancelled() => return,
            }
            if let Some(d) = self.discovery() {
                let known = known_channels(&d.devices());
                tokio::select! {
                    _ = async { tokio::join!(d.announce(), d.discover_known_http_channels(known)) } => {}
                    _ = self.stopped.cancelled() => return,
                }
            }
            self.devices_maybe_changed();
        }
    }
}

fn known_channels(devs: &[StatefulDevice]) -> Vec<HttpChannel> {
    devs.iter()
        .filter_map(|s| s.get_best_channel().and_then(|c| c.http()).cloned())
        .collect()
}

fn interface_ips() -> Vec<Ipv4Addr> {
    local_interface_addresses(&InterfaceFilter::default())
        .unwrap_or_default()
        .into_iter()
        .filter(|ip| !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified())
        .collect()
}

/// Private LAN IPv4 addresses of this machine (falls back to any non-loopback IPv4).
pub(crate) fn lan_ipv4s() -> Vec<Ipv4Addr> {
    let all: Vec<Ipv4Addr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.ip() {
            std::net::IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_unspecified() => Some(v4),
            _ => None,
        })
        .collect();
    let mut private: Vec<Ipv4Addr> = all.iter().copied().filter(|ip| ip.is_private()).collect();
    if private.is_empty() {
        private = all.into_iter().filter(|ip| !ip.is_link_local()).collect();
    }
    private.sort();
    private.dedup();
    private
}

impl LsBridge {
    pub async fn start(cfg: LsConfig) -> Result<(LsBridge, mpsc::Receiver<LsEvent>)> {
        Self::start_with(cfg, LsOptions::default()).await
    }

    /// [`LsBridge::start`] with non-default [`LsOptions`].
    pub async fn start_with(
        cfg: LsConfig,
        opts: LsOptions,
    ) -> Result<(LsBridge, mpsc::Receiver<LsEvent>)> {
        let dir = cfg.data_dir.join("ls");
        let ident = tokio::task::spawn_blocking(move || identity::load_or_create(&dir))
            .await
            .map_err(|e| anyhow!("tạo chứng chỉ LocalSend: {e}"))??;
        let (ev, ev_rx) = mpsc::channel(EVENT_CAPACITY);
        let (srv_tx, srv_rx) = mpsc::channel(256);
        let alias = {
            let a = files::single_line(&cfg.alias);
            if a.is_empty() {
                "Toss".to_string()
            } else {
                a
            }
        };
        let inner = Arc::new(Inner {
            rt: tokio::runtime::Handle::current(),
            ident,
            opts,
            preferred_port: cfg.port,
            alias: Mutex::new(alias),
            dest: Mutex::new(cfg.dest_dir),
            port: AtomicU16::new(0),
            ev,
            srv_tx,
            server: tokio::sync::Mutex::new(None),
            server_handle: RwLock::new(None),
            disc_slot: tokio::sync::Mutex::new(None),
            disc: RwLock::new(None),
            pending: Mutex::new(HashMap::new()),
            recv: Mutex::new(HashMap::new()),
            sends: Mutex::new(HashMap::new()),
            web: tokio::sync::Mutex::new(None),
            dev_cache: Mutex::new(HashMap::new()),
            restart_when_idle: AtomicBool::new(false),
            stopped: CancellationToken::new(),
        });
        // Server first (port known), then discovery announcing that port.
        {
            let mut slot = inner.server.lock().await;
            let (s, port, fallback) = inner.bind_server().await?;
            inner.port.store(port, Ordering::Relaxed);
            *inner
                .server_handle
                .write()
                .unwrap_or_else(|e| e.into_inner()) = Some(s.handle.clone());
            *slot = Some(s);
            inner.emit(LsEvent::Port { port, fallback });
        }
        inner.rt.spawn(inner.clone().server_loop(srv_rx));
        inner.restart_discovery().await;
        inner.rt.spawn(inner.clone().refresh_loop());
        Ok((LsBridge { inner }, ev_rx))
    }

    pub fn port(&self) -> u16 {
        self.inner.port.load(Ordering::Relaxed)
    }

    /// This device's LocalSend fingerprint (SHA-256 of the cert, uppercase hex).
    pub fn fingerprint(&self) -> String {
        self.inner.ident.fingerprint.clone()
    }

    /// Discovered LocalSend devices (self excluded, stale > 10 min dropped).
    pub fn devices(&self) -> Vec<LsDevice> {
        self.inner.devices()
    }

    /// Multicast burst + re-probe of known devices + /24 subnet scan. Takes a few seconds.
    pub async fn rescan(&self) {
        self.inner.rescan().await
    }

    /// Probes one address (manual "add by IP", or a peer multicast cannot reach).
    /// Returns the device when it answered (and is not this device).
    pub async fn discover_addr(&self, ip: &str, port: u16) -> Option<LsDevice> {
        let d = self.inner.discovery()?;
        let mut found = None;
        for proto in [ProtocolType::Https, ProtocolType::Http] {
            if let Ok(Some(s)) = d.discover(ip, port, proto).await {
                found = Some(s.device.fingerprint);
                break;
            }
        }
        self.inner.devices_maybe_changed();
        let fp = found?;
        self.devices().into_iter().find(|x| x.fingerprint == fp)
    }

    /// New device name: announced again; the server restarts (now, or after the running receive).
    pub async fn set_alias(&self, alias: &str) {
        let a = files::single_line(alias);
        if a.is_empty() || a == self.inner.alias() {
            return;
        }
        *lock(&self.inner.alias) = a;
        let busy = !lock(&self.inner.pending).is_empty() || !lock(&self.inner.recv).is_empty();
        if busy {
            self.inner.restart_when_idle.store(true, Ordering::Relaxed);
        } else if let Err(e) = self.inner.restart_server().await {
            tracing::warn!("LocalSend server restart failed: {e:#}");
        }
        self.inner.restart_discovery().await;
    }

    /// Inbox root for future receives.
    pub fn set_dest(&self, dir: PathBuf) {
        *lock(&self.inner.dest) = dir;
    }

    /// Answers an [`LsEvent::Incoming`].
    pub fn answer(&self, req_id: &str, accept: bool) {
        recv::answer(&self.inner, req_id, accept);
    }

    /// Sends files/folders (structure kept) to a discovered device. Returns the transfer id;
    /// progress/result arrive as events with that id.
    pub async fn send(&self, fingerprint: &str, items: Vec<PathBuf>) -> Result<String> {
        send::start_send(&self.inner, fingerprint, items).await
    }

    /// Sends a text message the LocalSend way (single text/plain file carried in `preview`).
    pub async fn send_text(&self, fingerprint: &str, text: &str) -> Result<()> {
        send::send_text(&self.inner, fingerprint, text).await
    }

    /// Cancels an outgoing transfer, a running receive, or declines a pending `Incoming`.
    pub fn cancel(&self, transfer_id: &str) {
        if let Some(ctl) = lock(&self.inner.sends).get(transfer_id) {
            ctl.cancel.cancel();
            return;
        }
        if lock(&self.inner.pending).contains_key(transfer_id) {
            recv::answer(&self.inner, transfer_id, false);
            return;
        }
        recv::cancel_receive(&self.inner, transfer_id);
    }

    /// Serves `items` on the LocalSend browser download page (plain HTTP, auto-accept).
    /// Replaces a running web share.
    pub async fn web_share_start(&self, items: Vec<PathBuf>) -> Result<WebShare> {
        web::start(&self.inner, items).await
    }

    /// Port of the running web share, if any.
    pub async fn web_share_port(&self) -> Option<u16> {
        web::port(&self.inner).await
    }

    pub async fn web_share_stop(&self) {
        web::stop(&self.inner).await
    }

    /// Stops everything: cancels transfers, declines pending requests, closes the ports.
    pub async fn stop(&self) {
        let inner = &self.inner;
        if inner.stopped.is_cancelled() {
            return;
        }
        for ctl in lock(&inner.sends).values() {
            ctl.cancel.cancel();
        }
        let pend: Vec<String> = lock(&inner.pending).keys().cloned().collect();
        for id in pend {
            recv::answer(inner, &id, false);
        }
        inner.stopped.cancel();
        lock(&inner.recv).clear();
        web::stop(inner).await;
        if let Some(d) = inner.disc_slot.lock().await.take() {
            let _ = d.stop.send(());
            let _ = tokio::time::timeout(Duration::from_secs(3), d.handle.wait_stopped()).await;
        }
        *inner.disc.write().unwrap_or_else(|e| e.into_inner()) = None;
        if let Some(s) = inner.server.lock().await.take() {
            let _ = s.stop.send(());
            let _ = tokio::time::timeout(Duration::from_secs(3), s.handle.wait_stopped()).await;
        }
        *inner
            .server_handle
            .write()
            .unwrap_or_else(|e| e.into_inner()) = None;
    }
}
