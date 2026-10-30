//! The ONLY file that touches `toss-core`. Everything above it (commands.rs, shell.rs)
//! speaks the IPC types from `ipc.rs` and plain `serde_json::Value`s.
//!
//! Written against contracts/core-api.md. Where the contract has no call for something the
//! UI contract needs, the gap is called out with `GAP:` and listed in contracts/CHANGES.md.

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex as StdMutex, OnceLock, Weak,
    },
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_notification::NotificationExt as _;
use tokio::sync::{broadcast, Mutex, RwLock};
use toss_core::{
    net::DEFAULT_HOST_PORT, wire::Op, ConnectReport, CtlClient, HostConfig as CoreHost, NetConfig,
    Node, NodeConfig, Root as CoreRoot, Ticket, TicketSpec as CoreSpec,
};

use crate::{
    config, firewall,
    ipc::{
        AppState, HostConfig, IncomingPush, JobLast, JobRecord, NetDiagUi, Received, Root, TextMsg,
        TicketInfo, TicketSpec,
    },
    logs,
    nearby::{self, Effect, V08},
    settings::{data_dir, QuickShare, Settings},
    shortlink::{self, ShortLinks},
};

/// Core struct -> shell mirror with the same serde shape (and shell value -> core argument).
pub fn via_serde<T: Serialize, U: DeserializeOwned>(t: &T) -> Result<U> {
    Ok(serde_json::from_value(serde_json::to_value(t)?)?)
}

/// Alias prefix reserved for quick-share roots.
pub const QS_PREFIX: &str = "qs-";

/// Return folder (under `dest_dir`) when a quick share turns host mode on.
const QS_RETURN_DIR: &str = "Tra-ve";

/// Prefix of the error text the UI uses to detect "no admin ticket stored".
pub const NO_ADMIN: &str = "no_admin_ticket: Chưa kết nối máy nào để điều khiển";

/// v0.5.1 `remote_update` on a host that cannot update remotely.
pub const REMOTE_TOO_OLD: &str =
    "Máy đó đang chạy bản cũ, chưa hỗ trợ cập nhật từ xa — cập nhật tay lần này";

/// How often a host whose config could not be applied at startup (e.g. a root
/// on a drive that is not mounted yet) tries again.
const HOST_RETRY: Duration = Duration::from_secs(30);

/// How often a deferred node restart (network settings saved during a transfer) is retried.
const RESTART_RETRY: Duration = Duration::from_secs(15);

/// v0.8: true while files are received into the fallback folder (the chosen one is unusable).
static DEST_FALLBACK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Where received files actually go: the chosen folder when usable, else `~/Toss`.
/// A chosen folder on a drive that is not plugged in (macOS `/Volumes/<name>`, a missing
/// Windows drive letter) is never created — that would make a phantom folder in place of the
/// drive — so receiving keeps working into the fallback until the drive is back.
pub(crate) fn effective_dest(chosen: &str) -> (PathBuf, bool) {
    let p = PathBuf::from(chosen);
    let ok = if p.is_dir() {
        !std::fs::metadata(&p)
            .map(|m| m.permissions().readonly())
            .unwrap_or(true)
    } else if on_removable_root(&p) {
        false
    } else {
        std::fs::create_dir_all(&p).is_ok()
    };
    if ok {
        return (p, false);
    }
    let fb = dirs::home_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Toss");
    let _ = std::fs::create_dir_all(&fb);
    (fb, true)
}

/// `/Volumes/<name>/…` (macOS) or a drive letter whose root is missing (Windows).
fn on_removable_root(p: &Path) -> bool {
    let mut comps = p.components();
    if cfg!(target_os = "macos") || cfg!(test) {
        let parts: Vec<_> = p.components().take(3).collect();
        if parts.len() >= 3 && parts[1].as_os_str() == "Volumes" {
            return !PathBuf::from_iter(&parts).is_dir();
        }
    }
    if cfg!(windows) {
        if let Some(std::path::Component::Prefix(pre)) = comps.next() {
            return !Path::new(&format!("{}\\", pre.as_os_str().to_string_lossy())).is_dir();
        }
    }
    false
}

pub struct Core {
    data_dir: PathBuf,
    app: OnceLock<AppHandle>,
    settings: StdMutex<Settings>,
    node: RwLock<Option<Arc<Node>>>,
    start_lock: Mutex<()>,
    ctl: Mutex<Option<Arc<CtlClient>>>,
    /// Tickets of remote jobs fetched with `CtlClient::job_ticket` (they never
    /// change for a given id); cleared when the admin ticket changes.
    remote_tickets: StdMutex<HashMap<[u8; 16], String>>,
    /// Serializes host-config changes (user `set_host`, quick share add/remove).
    host_lock: Mutex<()>,
    /// The fixed UDP port the last start asked for but could not bind (fell back to random).
    port_fallback: StdMutex<Option<u16>>,
    /// v0.5.2 short links this device made (`shortlinks.json`).
    short: Arc<ShortLinks>,
    /// v0.8: LocalSend bridge, its asks and transfers, received history.
    pub(crate) v08: V08,
}

impl Core {
    pub fn new() -> Core {
        Core::with_dir(data_dir())
    }

    pub fn with_dir(dir: PathBuf) -> Core {
        let settings = Settings::load(&dir);
        let short = Arc::new(ShortLinks::load(&dir));
        let v08 = V08::new(&dir);
        Core {
            data_dir: dir,
            app: OnceLock::new(),
            settings: StdMutex::new(settings),
            node: RwLock::new(None),
            start_lock: Mutex::new(()),
            ctl: Mutex::new(None),
            remote_tickets: StdMutex::new(HashMap::new()),
            host_lock: Mutex::new(()),
            port_fallback: StdMutex::new(None),
            short,
            v08,
        }
    }

    pub fn attach(&self, app: AppHandle) {
        let _ = self.app.set(app);
    }

    fn app(&self) -> Result<&AppHandle> {
        self.app.get().ok_or_else(|| anyhow!("app chưa sẵn sàng"))
    }

    /// The app handle once `attach`ed (None in unit tests).
    pub(crate) fn app_opt(&self) -> Option<&AppHandle> {
        self.app.get()
    }

    pub(crate) fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    // ---- settings helpers ------------------------------------------------
    pub fn settings(&self) -> Settings {
        self.settings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Mutates and saves under one lock: concurrent updates (e.g. two commands
    /// from the UI) must neither write an older snapshot over a newer one nor
    /// race on the temp file. A failed save rolls the in-memory change back.
    pub(crate) fn update_settings<R>(&self, f: impl FnOnce(&mut Settings) -> R) -> Result<R> {
        let mut s = self.settings.lock().unwrap_or_else(|p| p.into_inner());
        let before = s.clone();
        let r = f(&mut s);
        if let Err(e) = s.save(&self.data_dir) {
            *s = before;
            return Err(e);
        }
        Ok(r)
    }

    pub fn first_run(&self) -> bool {
        self.settings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .first_run
    }

    // ---- node lifecycle -----------------------------------------------------
    /// The running node; starts it on first use (also called once at startup in the background).
    pub async fn node(&self) -> Result<Arc<Node>> {
        if let Some(n) = self.node.read().await.clone() {
            return Ok(n);
        }
        let _g = self.start_lock.lock().await;
        if let Some(n) = self.node.read().await.clone() {
            return Ok(n);
        }
        let n = self.start_node().await?;
        *self.node.write().await = Some(n.clone());
        Ok(n)
    }

    async fn start_node(&self) -> Result<Arc<Node>> {
        let (name, host, net) = {
            let s = self.settings.lock().unwrap_or_else(|p| p.into_inner());
            // User roots + quick-share roots (missing folders are tolerated here and
            // picked up by the retry once their drive is mounted).
            (
                s.device_name.clone(),
                effective_host(&s, false),
                s.net.clone(),
            )
        };
        std::fs::create_dir_all(&self.data_dir).context("tạo thư mục dữ liệu")?;
        let cfg = NodeConfig {
            data_dir: self.data_dir.clone(),
            device_name: name,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            net,
        };
        let port = host_port(host.as_ref());
        let start = |port| {
            let cfg = cfg.clone();
            async move {
                tokio::time::timeout(Duration::from_secs(20), Node::start_with_port(cfg, port))
                    .await
                    .map_err(|_| anyhow!("khởi động mạng quá lâu"))?
                    .context("không khởi động được mạng P2P")
            }
        };
        let node = match start(port).await {
            Ok(n) => {
                *self.port_fallback.lock().unwrap_or_else(|p| p.into_inner()) = None;
                n
            }
            // A taken port must not leave the app without a network: fall back to a random one.
            Err(e) if port.is_some() => {
                tracing::error!("cổng UDP {port:?} không dùng được ({e:#}); dùng cổng ngẫu nhiên");
                let n = start(None).await?;
                *self.port_fallback.lock().unwrap_or_else(|p| p.into_inner()) = port;
                n
            }
            Err(e) => return Err(e),
        };
        let node = Arc::new(node);
        // v0.5: pushes land under the receive folder; v0.8: who may send without asking.
        let s = self.settings();
        node.set_inbox_dir(self.inbox());
        node.set_receive_policy(s.receive_policy);
        // Forward events before host mode starts emitting them.
        self.spawn_forwarder(&node);
        if let Some(h) = host {
            if !self.apply_host_at_start(&node, &h).await {
                self.spawn_host_retry(Arc::downgrade(&node), h);
            }
        }
        Ok(node)
    }

    /// Turns host mode on from the saved config. If that fails (typically a
    /// root on a drive that is not mounted yet when the app autostarts), host
    /// mode starts with the roots that are available. Returns true when the
    /// full config is active.
    async fn apply_host_at_start(&self, node: &Node, h: &HostConfig) -> bool {
        let err = match to_core_host(h, false) {
            Ok(c) => match node.set_host(Some(c)).await {
                Ok(()) => return true,
                Err(e) => e,
            },
            Err(e) => e,
        };
        tracing::error!("bật phục vụ link thất bại: {err:#}");
        if let Some(partial) = available_roots(h) {
            if let Ok(c) = to_core_host(&partial, false) {
                match node.set_host(Some(c)).await {
                    Ok(()) => tracing::warn!(
                        "phục vụ link chạy với {}/{} thư mục; sẽ thử lại",
                        partial.roots.len(),
                        h.roots.len()
                    ),
                    Err(e) => tracing::error!("bật một phần phục vụ link thất bại: {e:#}"),
                }
            }
        }
        false
    }

    /// Re-applies the saved host config every `HOST_RETRY` until it works, the
    /// user changes it, or the node is replaced.
    fn spawn_host_retry(&self, node: Weak<Node>, h: HostConfig) {
        let Some(app) = self.app.get().cloned() else {
            return;
        };
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(HOST_RETRY).await;
                let Some(node) = node.upgrade() else { return };
                let core = app.state::<Arc<Core>>().inner().clone();
                if effective_host(&core.settings(), false).as_ref() != Some(&h) {
                    return; // changed by the user meanwhile
                }
                if let Ok(c) = to_core_host(&h, false) {
                    if node.set_host(Some(c)).await.is_ok() {
                        tracing::info!("phục vụ link đã bật đầy đủ");
                        return;
                    }
                }
            }
        });
    }

    /// Restarts the node with the current settings (device name, host port, `net`). Deferred
    /// (`Ok(false)`) while transfers are active; `Ok(true)` once restarted.
    async fn restart_node(&self) -> Result<bool> {
        let _g = self.start_lock.lock().await;
        let Some(cur) = self.node.read().await.clone() else {
            return Ok(true); // not running: the next start reads the settings
        };
        if cur
            .transfers()
            .iter()
            .any(|t| is_active(&serde_json::to_value(t).unwrap_or(Value::Null)))
        {
            tracing::info!("settings saved; node restart deferred (transfers active)");
            return Ok(false);
        }
        drop(cur);
        let old = self.node.write().await.take();
        *self.ctl.lock().await = None;
        if let Some(n) = old {
            // `Node` is a cheap handle: shut the shared node down even if a command
            // still holds another `Arc` (otherwise two endpoints with the same key
            // would run side by side).
            n.as_ref().clone().shutdown().await;
        }
        let n = self.start_node().await?;
        *self.node.write().await = Some(n);
        Ok(true)
    }

    /// After a deferred restart (transfers were active): checks every `RESTART_RETRY` and
    /// restarts once the node is idle. One watcher at a time.
    fn spawn_restart_when_idle(&self) {
        static WATCHING: AtomicBool = AtomicBool::new(false);
        let Some(app) = self.app.get().cloned() else {
            return;
        };
        if WATCHING.swap(true, Ordering::SeqCst) {
            return;
        }
        tauri::async_runtime::spawn(async move {
            let core = app.state::<Arc<Core>>().inner().clone();
            loop {
                tokio::time::sleep(RESTART_RETRY).await;
                match core.restart_node().await {
                    Ok(false) => continue,
                    Ok(true) => tracing::info!("node restarted with the new network settings"),
                    Err(e) => tracing::error!("node restart failed: {e:#}"),
                }
                break;
            }
            WATCHING.store(false, Ordering::SeqCst);
        });
    }

    /// Pauses client transfers (keeping `.part` files) and closes the endpoint.
    /// Idempotent; called from tray "Thoát" and on app exit.
    pub async fn shutdown(&self) {
        let old = self.node.write().await.take();
        *self.ctl.lock().await = None;
        if let Some(n) = old {
            n.as_ref().clone().shutdown().await;
        }
    }

    // ---- event forwarding -----------------------------------------------------
    fn spawn_forwarder(&self, node: &Arc<Node>) {
        let Some(app) = self.app.get().cloned() else {
            return;
        };
        let mut rx = node.subscribe();
        let node = Arc::downgrade(node);
        tauri::async_runtime::spawn(async move {
            let mut seen = Seen::default();
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        if let Ok(v) = serde_json::to_value(&ev) {
                            forward(&app, &mut seen, v);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // Dropped events may include a Started or a final Done/Error: rebuild
                        // the UI's view from snapshots instead of leaving it stale.
                        tracing::warn!("event stream lagged by {n}; resyncing");
                        let Some(node) = node.upgrade() else { break };
                        let snaps: Vec<Value> = node
                            .transfers()
                            .iter()
                            .filter_map(|t| serde_json::to_value(t).ok())
                            .collect();
                        drop(node);
                        for v in resync_events(&snaps, &seen) {
                            forward(&app, &mut seen, v);
                        }
                        // the lag may have eaten contact / presence / push events too
                        let _ = app.emit("contacts:changed", serde_json::json!({}));
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    // ---- app state ---------------------------------------------------------------
    pub async fn state(&self) -> Result<AppState> {
        let s = self.settings();
        let endpoint_id = match self.node().await {
            Ok(n) => n.endpoint_id().to_string(),
            Err(e) => {
                tracing::warn!("state without node: {e:#}");
                String::new()
            }
        };
        let autostart = self
            .app()
            .ok()
            .and_then(|a| a.autolaunch().is_enabled().ok())
            .unwrap_or(s.autostart);
        let receive_policy = s.receive_policy;
        Ok(AppState {
            version: env!("CARGO_PKG_VERSION").to_string(),
            endpoint_id,
            device_name: s.device_name.clone(),
            dest_dir: s.dest_dir.clone(),
            host: user_host(&s),
            page_host: s.page_host(),
            autostart,
            first_run: s.first_run,
            context_menu: s.context_menu,
            auto_receive: s.auto_receive,
            net: s.net.clone(),
            auto_update: s.auto_update(),
            receive_policy,
            ls_enabled: s.ls_enabled,
            ls_port: self.v08.ls_port(),
            ls_trusted: s.ls_trusted.clone(),
        })
    }

    /// v0.5.1: effective auto-update setting.
    pub fn auto_update(&self) -> bool {
        self.settings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .auto_update()
    }

    pub fn set_auto_update(&self, on: bool) -> Result<()> {
        tracing::info!(on, "auto update");
        self.update_settings(|s| s.auto_update = Some(on))
    }

    /// v0.5.1: transfers moving data right now (client and host side). Waiting
    /// ones (connecting, paused) do not count: they resume after a restart.
    /// 0 when the node is not running.
    pub async fn running_transfers(&self) -> usize {
        let Some(node) = self.node.read().await.clone() else {
            return 0;
        };
        node.transfers()
            .iter()
            .filter(|t| t.status == toss_core::transfer::Status::Running)
            .count()
            + self.v08.running()
    }

    pub async fn finish_setup(&self, device_name: String, dest_dir: String) -> Result<AppState> {
        let name = clean_name(&device_name)?;
        std::fs::create_dir_all(&dest_dir)
            .with_context(|| format!("không tạo được thư mục {dest_dir}"))?;
        self.update_settings(|s| {
            s.device_name = name.clone();
            s.dest_dir = dest_dir;
            s.first_run = false;
        })?;
        self.apply_device_name(&name).await;
        self.set_dest_dir_live();
        self.state().await
    }

    pub async fn set_device_name(&self, name: String) -> Result<()> {
        let name = clean_name(&name)?;
        self.update_settings(|s| s.device_name = name.clone())?;
        self.apply_device_name(&name).await;
        Ok(())
    }

    /// v0.8: the running node (Hello/Pair name + mDNS) and the LocalSend alias take a new name
    /// live, no restart.
    async fn apply_device_name(&self, name: &str) {
        if let Some(n) = self.node.read().await.clone() {
            n.set_device_name(name);
        }
        if let Some(ls) = self.v08.ls() {
            ls.set_alias(name).await;
        }
    }

    /// Pushes (Toss and LocalSend) land under the current receive folder.
    /// The folder pushes, LocalSend files and links land in right now (see [`effective_dest`]).
    /// Tells the user once when it switches to / back from the fallback.
    pub(crate) fn inbox(&self) -> PathBuf {
        let chosen = self.settings().dest_dir;
        let (dest, fb) = effective_dest(&chosen);
        let was = DEST_FALLBACK.swap(fb, std::sync::atomic::Ordering::SeqCst);
        if fb != was {
            let msg = if fb {
                format!(
                    "Không ghi được vào “{chosen}” (ổ chưa cắm?) — file nhận tạm vào {}",
                    dest.display()
                )
            } else {
                format!("Đã nhận lại thư mục “{chosen}”")
            };
            tracing::warn!("{msg}");
            if let Some(app) = self.app.get() {
                notify(app, &msg);
            }
        }
        dest
    }

    /// Re-checks the receive folder (drive plugged in / out) and applies it live.
    pub(crate) fn refresh_inbox(&self) {
        self.set_dest_dir_live();
    }

    fn set_dest_dir_live(&self) {
        let dest = self.inbox();
        if let Ok(guard) = self.node.try_read() {
            if let Some(n) = guard.as_ref() {
                n.set_inbox_dir(dest.clone());
            }
        }
        if let Some(ls) = self.v08.ls() {
            ls.set_dest(dest);
        }
    }

    pub fn set_dest_dir(&self, path: String) -> Result<()> {
        std::fs::create_dir_all(&path).with_context(|| format!("không tạo được thư mục {path}"))?;
        self.update_settings(|s| s.dest_dir = path.clone())?;
        // v0.5: files pushed by contacts land under the same folder (v0.8: LocalSend too).
        self.set_dest_dir_live();
        Ok(())
    }

    pub fn set_autostart(&self, on: bool) -> Result<()> {
        let launch = self.app()?.autolaunch();
        if on {
            launch
                .enable()
                .map_err(|e| anyhow!("không bật được tự khởi động: {e}"))?;
        } else {
            launch
                .disable()
                .map_err(|e| anyhow!("không tắt được tự khởi động: {e}"))?;
        }
        self.update_settings(|s| s.autostart = on)
    }

    // ---- host mode ----------------------------------------------------------------
    /// From the Settings UI: `config` holds the user's roots only. Quick-share roots
    /// are merged in here (they keep working) and never stored in `host.roots`.
    /// `None` turns host mode off; quick shares are kept but unreachable until host
    /// mode is on again (a new quick share turns it back on).
    pub async fn set_host(&self, config: Option<HostConfig>) -> Result<()> {
        let _g = self.host_lock.lock().await;
        let node = self.node().await?;
        let shares = self.settings().quick_shares;
        let config = match config {
            Some(mut h) => {
                // Stale copies of quick-share roots are dropped, other `qs-` names refused.
                h.roots
                    .retain(|r| !shares.iter().any(|q| q.alias == r.alias));
                if let Some(r) = h.roots.iter().find(|r| r.alias.starts_with(QS_PREFIX)) {
                    bail!(
                        "Tên gốc “{}” không dùng được (tiền tố “{QS_PREFIX}” dành cho chia sẻ nhanh)",
                        r.alias
                    );
                }
                to_core_host(&h, true)?; // the user's own roots must exist
                Some(h)
            }
            None => None,
        };
        let merged = config
            .as_ref()
            .map(|h| merge_quick_shares(h, &shares, true));
        match &merged {
            Some(h) => node.set_host(Some(to_core_host(h, false)?)).await?,
            None => node.set_host(None).await?,
        }
        let want_port = host_port(config.as_ref());
        self.update_settings(|s| {
            // Still "auto" only while the user has not added roots of their own.
            s.host_auto = s.host_auto && config.as_ref().is_some_and(|h| h.roots.is_empty());
            s.host = config;
        })?;
        // The UDP port is bound when the node starts: restart to apply a new one
        // (deferred to the next launch while transfers are running).
        // (A port that was taken at startup stays random until the user changes it.)
        let fell_back = want_port.is_some()
            && node.bind_port().is_none()
            && *self.port_fallback.lock().unwrap_or_else(|p| p.into_inner()) == want_port;
        if want_port.is_some() && want_port != node.bind_port() && !fell_back {
            drop(node);
            self.restart_node().await?;
        }
        Ok(())
    }

    // ---- quick share (v0.3) ------------------------------------------------------------
    /// Right-click → "Tạo link Toss": serve `path` as a quick-share root (host mode is
    /// turned on if needed) and create a get ticket for the whole folder. v0.8: a single FILE
    /// is shared with its parent folder as the root and the file as the ticket path.
    /// Clipboard and notification are done by the command (needs the app handle).
    pub async fn share_folder(&self, path: String, days: u32, put: bool) -> Result<TicketInfo> {
        let _g = self.host_lock.lock().await;
        let raw = path.trim();
        // root = the folder itself or the file's parent; path = "" or the file name.
        let t = toss_core::paths::share_target(Path::new(raw))?;
        let (canon, file) = (t.root.clone(), t.path.clone());
        let canon_s = canon.to_string_lossy().into_owned();
        let alias = quick_share_alias(&canon_s);
        let job = if t.is_file {
            toss_core::paths::sanitize_component(&t.job, "Toss")
        } else {
            share_job_name(&canon)
        };

        // 1. settings: add/keep the root; turn host mode on if it is off.
        let (added, auto_on, ret) = self.update_settings(|s| {
            let added = !s.quick_shares.iter().any(|q| q.alias == alias);
            if added {
                s.quick_shares.push(QuickShare {
                    alias: alias.clone(),
                    path: canon_s.clone(),
                    job_id: None,
                });
            }
            let auto_on = s.host.is_none();
            if auto_on {
                s.host = Some(HostConfig {
                    roots: Vec::new(),
                    return_root: Path::new(&s.dest_dir)
                        .join(QS_RETURN_DIR)
                        .to_string_lossy()
                        .into_owned(),
                    max_parallel_jobs: 2,
                    bind_port: Some(DEFAULT_HOST_PORT),
                });
                s.host_auto = true;
            }
            let ret = s.host.as_ref().map(|h| h.return_root.clone());
            (added, auto_on, ret)
        })?;
        if let Some(ret) = ret.filter(|_| auto_on) {
            let _ = std::fs::create_dir_all(ret);
        }
        // 2. apply; undo step 1 if the host cannot take it.
        if let Err(e) = self.apply_host().await {
            let _ = self.update_settings(|s| {
                if added {
                    s.quick_shares.retain(|q| q.alias != alias);
                }
                if auto_on {
                    s.host = None;
                    s.host_auto = false;
                }
            });
            let _ = self.apply_host().await;
            return Err(e.context("không bật được chia sẻ"));
        }
        // 3. ticket for the whole folder.
        let info = self
            .create_ticket(TicketSpec {
                job,
                root: alias.clone(),
                path: file,
                ret: String::new(),
                get: true,
                put,
                days,
            })
            .await?;
        self.update_settings(|s| {
            if let Some(q) = s.quick_shares.iter_mut().find(|q| q.alias == alias) {
                q.job_id = Some(info.id.clone());
            }
        })?;
        Ok(info)
    }

    pub fn list_quick_shares(&self) -> Vec<QuickShare> {
        self.settings().quick_shares
    }

    /// Revokes every ticket of the quick share, drops its root and re-applies host
    /// mode (turned off if it was only on for quick shares and none remain).
    pub async fn remove_quick_share(&self, alias: String) -> Result<()> {
        let _g = self.host_lock.lock().await;
        let Some(qs) = self
            .settings()
            .quick_shares
            .into_iter()
            .find(|q| q.alias == alias)
        else {
            bail!("Không tìm thấy chia sẻ “{alias}”");
        };
        let node = self.node().await?;
        let mut ids: Vec<[u8; 16]> = node
            .list_jobs()
            .into_iter()
            .filter(|j| j.root == qs.alias && !j.revoked)
            .map(|j| j.id)
            .collect();
        if let Some(id) = qs.job_id.as_deref().and_then(|i| parse_id(i).ok()) {
            if !ids.contains(&id) && node.list_jobs().iter().any(|j| j.id == id && !j.revoked) {
                ids.push(id);
            }
        }
        for id in ids {
            node.revoke(id)?;
        }
        drop(node);
        self.update_settings(|s| {
            s.quick_shares.retain(|q| q.alias != alias);
            let only_auto = s.host_auto && s.host.as_ref().is_some_and(|h| h.roots.is_empty());
            if s.quick_shares.is_empty() && only_auto {
                s.host = None;
                s.host_auto = false;
            }
        })?;
        self.apply_host().await
    }

    /// Re-applies the saved host config (user roots + existing quick-share folders).
    async fn apply_host(&self) -> Result<()> {
        let node = self.node().await?;
        match effective_host(&self.settings(), true) {
            Some(h) => node.set_host(Some(to_core_host(&h, false)?)).await,
            None => node.set_host(None).await,
        }
    }

    pub fn set_auto_receive(&self, on: bool) -> Result<()> {
        self.update_settings(|s| s.auto_receive = on)
    }

    /// Registers / removes the OS right-click entry, then saves the choice.
    pub async fn set_context_menu(&self, on: bool) -> Result<()> {
        tokio::task::spawn_blocking(move || crate::ctxmenu::apply(on))
            .await?
            .map_err(|e| {
                anyhow!(
                    "Không {} được menu chuột phải: {e:#}",
                    if on { "bật" } else { "tắt" }
                )
            })?;
        self.update_settings(|s| s.context_menu = on)
    }

    pub async fn host_status(&self) -> Result<Value> {
        let node = self.node().await?;
        Ok(match node.host_status() {
            Some(h) => serde_json::to_value(h)?,
            None => Value::Null,
        })
    }

    pub async fn list_dir(&self, root: String, path: String) -> Result<Value> {
        let node = self.node().await?;
        let entries = tokio::task::spawn_blocking(move || node.list_dir(&root, &path)).await??;
        Ok(serde_json::to_value(entries)?)
    }

    pub async fn create_ticket(&self, spec: TicketSpec) -> Result<TicketInfo> {
        let node = self.node().await?;
        wait_for_relay(&node).await;
        let t = node.create_ticket(to_core_spec(spec))?;
        Ok(self.info_short(&t).await)
    }

    pub async fn list_jobs(&self) -> Result<Vec<JobRecord>> {
        let node = self.node().await?;
        Ok(node
            .list_jobs()
            .into_iter()
            .map(|j| {
                // Re-signed ticket string so the UI can offer "copy link" for old jobs too.
                let ticket = node
                    .job_ticket(j.id)
                    .map(|t| t.to_string())
                    .unwrap_or_default();
                let web_url = self.web_url_or_bg(&ticket, j.revoked);
                let mut r = job_record(j, ticket);
                r.web_url = web_url;
                r
            })
            .collect())
    }

    pub async fn revoke(&self, id: String) -> Result<()> {
        self.node().await?.revoke(parse_id(&id)?)
    }

    pub async fn create_admin_ticket(&self, days: u32) -> Result<TicketInfo> {
        let node = self.node().await?;
        wait_for_relay(&node).await;
        // Not in core-api.md, but provided by toss-core (see CHANGES.md).
        let t = node.create_admin_ticket(days)?;
        Ok(self.info(&t))
    }

    // ---- tickets / transfers (client) -----------------------------------------------
    pub async fn parse_ticket(&self, text: &str) -> Result<TicketInfo> {
        let text = shortlink::expand(text.trim()).await?;
        let t = Ticket::from_str(&text).map_err(|e| anyhow!("không đọc được link ({e})"))?;
        Ok(self.info(&t))
    }

    pub async fn start_get(&self, ticket: String) -> Result<String> {
        let ticket = shortlink::expand(ticket.trim()).await?;
        let t =
            Ticket::from_str(ticket.trim()).map_err(|e| anyhow!("không đọc được link ({e})"))?;
        if t.body.admin {
            bail!("Đây là link admin, không dùng để nhận file");
        }
        if !t.body.get {
            bail!("Link này không cho phép nhận file");
        }
        if t.is_expired() {
            bail!("Link đã hết hạn");
        }
        let dest = self.inbox();
        std::fs::create_dir_all(&dest)
            .with_context(|| format!("không tạo được thư mục {}", dest.display()))?;
        let node = self.node().await?;
        let tr = node.get(t, dest).await?;
        Ok(tr.id.clone())
    }

    pub async fn start_put(&self, ticket: String, paths: Vec<String>) -> Result<String> {
        let ticket = shortlink::expand(ticket.trim()).await?;
        let t =
            Ticket::from_str(ticket.trim()).map_err(|e| anyhow!("không đọc được link ({e})"))?;
        if t.body.admin || !t.body.put {
            bail!("Link này không cho gửi file ngược lại");
        }
        if t.is_expired() {
            bail!("Link đã hết hạn");
        }
        if paths.is_empty() {
            bail!("Chưa chọn file hoặc folder nào");
        }
        let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        if let Some(p) = paths.iter().find(|p| !p.exists()) {
            bail!("Không tìm thấy {}", p.display());
        }
        let node = self.node().await?;
        let tr = node.put(t, paths).await?;
        Ok(tr.id.clone())
    }

    /// Core transfers ∪ v0.8 LocalSend transfers (same `TransferState` shape).
    pub async fn list_transfers(&self) -> Result<Vec<Value>> {
        let node = self.node().await?;
        let mut v: Vec<Value> = node
            .transfers()
            .iter()
            .filter_map(|t| serde_json::to_value(t).ok())
            .collect();
        v.extend(self.v08.transfer_values());
        Ok(v)
    }

    pub async fn pause(&self, id: String) -> Result<()> {
        if self.v08.is_transfer(&id) {
            bail!("Không tạm dừng được khi gửi qua LocalSend — huỷ rồi gửi lại");
        }
        self.node().await?.pause(&id).await
    }

    pub async fn resume(&self, id: String) -> Result<()> {
        if self.v08.is_transfer(&id) {
            bail!("Không tiếp tục được khi gửi qua LocalSend — bấm Gửi lại");
        }
        self.node().await?.resume(&id).await
    }

    pub async fn cancel(&self, id: String) -> Result<()> {
        if let Some(ev) = self.v08.mark_cancelled(&id) {
            if let Some(ls) = self.v08.ls() {
                ls.cancel(&id);
            }
            if let Some(app) = self.app_opt() {
                let _ = app.emit("transfer:event", ev);
            }
            return Ok(());
        }
        self.node().await?.cancel(&id).await
    }

    /// Not in core-api.md, but provided by toss-core (see CHANGES.md).
    pub async fn clear_finished(&self) -> Result<()> {
        self.node().await?.clear_finished();
        self.v08.clear_finished();
        Ok(())
    }

    // ---- remote host control (admin ticket) -------------------------------------------
    pub async fn set_admin_ticket(&self, ticket: Option<String>) -> Result<()> {
        let stored = match ticket.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            Some(text) => {
                let t = Ticket::from_str(text).map_err(|e| anyhow!("không đọc được link ({e})"))?;
                if !t.body.admin {
                    bail!("Đây không phải link admin");
                }
                if t.is_expired() {
                    bail!("Link admin đã hết hạn");
                }
                Some(t.to_string())
            }
            None => None,
        };
        *self.ctl.lock().await = None;
        self.remote_tickets
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.update_settings(|s| s.admin_ticket = stored)
    }

    pub fn has_admin_ticket(&self) -> bool {
        self.settings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .admin_ticket
            .is_some()
    }

    async fn ctl(&self) -> Result<Arc<CtlClient>> {
        let mut g = self.ctl.lock().await;
        if let Some(c) = g.clone() {
            return Ok(c);
        }
        let text = self
            .settings()
            .admin_ticket
            .ok_or_else(|| anyhow!(NO_ADMIN))?;
        let t = Ticket::from_str(&text).map_err(|e| anyhow!("link admin hỏng ({e})"))?;
        let node = self.node().await?;
        let c = tokio::time::timeout(Duration::from_secs(20), node.ctl(t))
            .await
            .map_err(|_| anyhow!("Máy kia không phản hồi (đang offline?)"))?
            .context("không kết nối được máy kia")?;
        let c = Arc::new(c);
        *g = Some(c.clone());
        Ok(c)
    }

    /// Run `f` on the control client; on failure reconnect once (cached connections go stale).
    async fn with_ctl<T, F, Fut>(&self, f: F) -> Result<T>
    where
        F: Fn(Arc<CtlClient>) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let c = self.ctl().await?;
        match f(c).await {
            Ok(v) => Ok(v),
            Err(first) => {
                *self.ctl.lock().await = None;
                match self.ctl().await {
                    Ok(c) => f(c).await,
                    Err(_) => Err(first),
                }
            }
        }
    }

    pub async fn remote_status(&self) -> Result<Value> {
        let s = self.with_ctl(|c| async move { c.status().await }).await?;
        Ok(serde_json::to_value(s)?)
    }

    /// v0.5.1: asks the remote host (admin ticket) to update its app now.
    /// Hosts before 0.5.1 do not know the request: refused up front.
    pub async fn remote_update(&self) -> Result<()> {
        let st = self.with_ctl(|c| async move { c.status().await }).await?;
        if !crate::updater::version_at_least(&st.version, (0, 5, 1)) {
            tracing::info!(version = %st.version, "remote update: host too old");
            bail!(REMOTE_TOO_OLD);
        }
        self.with_ctl(|c| async move { c.request_update().await })
            .await
            .map_err(|e| anyhow!("Không gửi được lệnh cập nhật tới máy đó: {e:#}"))?;
        tracing::info!(host = %st.device, version = %st.version, "remote update requested");
        Ok(())
    }

    pub async fn remote_list_dir(&self, root: String, path: String) -> Result<Value> {
        let e = self
            .with_ctl(|c| {
                let (root, path) = (root.clone(), path.clone());
                async move { c.list_dir(&root, &path).await }
            })
            .await?;
        Ok(serde_json::to_value(e)?)
    }

    pub async fn remote_create_ticket(&self, spec: TicketSpec) -> Result<TicketInfo> {
        let t = self
            .with_ctl(|c| {
                let spec = to_core_spec(spec.clone());
                async move { c.create_ticket(spec).await }
            })
            .await?;
        Ok(self.info_short(&t).await)
    }

    pub async fn remote_list_jobs(&self) -> Result<Vec<JobRecord>> {
        let jobs = self
            .with_ctl(|c| async move { c.list_jobs().await })
            .await?;
        // Fill `ticket` (for "copy link") via CtlRequest::JobTicket, for usable job
        // tickets only; cached per id. Old hosts don't know the request: stop at the
        // first failure and leave the rest empty (the UI hides "copy" then).
        let mut out = Vec::with_capacity(jobs.len());
        let mut fetch = true;
        let host = self
            .settings()
            .admin_ticket
            .and_then(|t| Ticket::from_str(&t).ok())
            .map(|t| t.body.host);
        for j in jobs {
            let usable = !j.revoked
                && (j.get || j.put)
                && (j.exp == 0 || j.exp > toss_core::util::now_unix());
            let cached = self
                .remote_tickets
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(&j.id)
                .cloned();
            let ticket = match cached {
                Some(t) => t,
                None if usable && fetch => match self.fetch_remote_ticket(j.id, host).await {
                    Ok(t) => {
                        self.remote_tickets
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .insert(j.id, t.clone());
                        t
                    }
                    Err(e) => {
                        tracing::info!("job tickets unavailable from this host: {e:#}");
                        fetch = false;
                        String::new()
                    }
                },
                None => String::new(),
            };
            out.push(job_record(j, ticket));
        }
        Ok(out)
    }

    async fn fetch_remote_ticket(&self, id: [u8; 16], host: Option<[u8; 32]>) -> Result<String> {
        let c = self.ctl().await?;
        let t = tokio::time::timeout(Duration::from_secs(10), c.job_ticket(id))
            .await
            .map_err(|_| anyhow!("timeout"))??;
        // Only accept what the host itself signed for that id.
        if t.body.id != id
            || Some(t.body.host) != host
            || t.body.admin
            || t.verify_signature().is_err()
        {
            bail!("host returned a ticket that does not match the job");
        }
        Ok(t.to_string())
    }

    pub async fn remote_revoke(&self, id: String) -> Result<()> {
        let id = parse_id(&id)?;
        self.with_ctl(|c| async move { c.revoke(id).await }).await
    }

    // ---- v0.4 diagnostics ---------------------------------------------------------------------
    pub fn log_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    /// Core snapshot + firewall (cached, checked off the runtime) + log folder. When host mode
    /// wanted a fixed UDP port the node could not bind, that goes first in `reasons`.
    pub async fn diagnostics(&self) -> Result<NetDiagUi> {
        let node = self.node().await?;
        let (diag, firewall) = tokio::join!(
            tokio::time::timeout(Duration::from_secs(10), node.diagnostics()),
            firewall::state()
        );
        let mut diag = diag.map_err(|_| anyhow!("Đo mạng quá lâu, thử lại sau"))?;
        let wanted = host_port(effective_host(&self.settings(), false).as_ref());
        if let Some(p) = wanted.filter(|p| diag.bind_port != Some(*p)) {
            diag.reasons.insert(
                0,
                format!("Cổng UDP {p} đang bị chương trình khác dùng — Toss phải dùng cổng ngẫu nhiên nên khó mở cổng trên router"),
            );
        }
        Ok(NetDiagUi {
            diag,
            firewall,
            log_dir: self.log_dir().to_string_lossy().into_owned(),
        })
    }

    /// Dials the host of `ticket` (no job started). An empty `ticket` means the stored admin
    /// ticket, which the UI never sees.
    pub async fn test_connect(&self, ticket: String) -> Result<ConnectReport> {
        let text = match ticket.trim() {
            "" => self
                .settings()
                .admin_ticket
                .ok_or_else(|| anyhow!("Dán một link Toss của máy kia để thử nối"))?,
            t => t.to_string(),
        };
        let t = Ticket::from_str(&text).map_err(|e| anyhow!("không đọc được link ({e})"))?;
        let node = self.node().await?;
        tokio::time::timeout(Duration::from_secs(30), node.test_connect(&t))
            .await
            .map_err(|_| anyhow!("Thử nối quá lâu, máy kia không phản hồi"))
    }

    /// Saves `net` and applies it by restarting the node (deferred, and retried until idle,
    /// while a transfer is running).
    pub async fn set_net(&self, net: NetConfig) -> Result<()> {
        let net = clean_net(net)?;
        let changed = self.update_settings(|s| {
            let changed = s.net != net;
            s.net = net;
            changed
        })?;
        if changed && !self.restart_node().await? {
            self.spawn_restart_when_idle();
        }
        Ok(())
    }

    /// `~/Desktop/toss-logs-<stamp>.zip`: logs, redacted settings and a diagnostics snapshot.
    pub async fn export_logs(&self) -> Result<String> {
        let mut extra: Vec<(&str, Vec<u8>)> = Vec::new();
        if let Ok(Ok(d)) = tokio::time::timeout(Duration::from_secs(4), self.diagnostics()).await {
            extra.push(("diagnostics.json", serde_json::to_vec_pretty(&d)?));
        }
        extra.push((
            "about.txt",
            format!(
                "Toss {}\n{} {}\n",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH
            )
            .into_bytes(),
        ));
        let dir = self.data_dir.clone();
        let dest = dirs::desktop_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let out =
            tokio::task::spawn_blocking(move || logs::export_zip(&dir, &extra, &dest)).await??;
        Ok(out.to_string_lossy().into_owned())
    }

    // ---- misc -------------------------------------------------------------------------------
    fn info(&self, t: &Ticket) -> TicketInfo {
        let mut info = ticket_info(t, &self.settings().page_host());
        if let Some(u) = self.short.get(&info.ticket) {
            info.web_url = u;
        }
        info
    }

    /// `info` for a ticket just minted: asks for a short link first (falls back to the
    /// long one after a few seconds). Admin tickets always stay long.
    async fn info_short(&self, t: &Ticket) -> TicketInfo {
        let mut info = self.info(t);
        if !t.body.admin {
            let host = self.settings().page_host();
            if let Some(u) = self.short.shorten(&host, &info.ticket).await {
                info.web_url = u;
            }
        }
        info
    }

    /// Short link of a stored job, or its long link while a short one is fetched in the
    /// background (once per session) for the next refresh.
    fn web_url_or_bg(&self, ticket: &str, revoked: bool) -> String {
        if ticket.is_empty() {
            return String::new();
        }
        let page_host = self.settings().page_host();
        if let Some(u) = self.short.get(ticket) {
            return u;
        }
        if !revoked && self.short.first_try(ticket) {
            let (short, host, text) = (self.short.clone(), page_host.clone(), ticket.to_string());
            tauri::async_runtime::spawn(async move { short.shorten(&host, &text).await });
        }
        match Ticket::from_str(ticket) {
            Ok(t) => t.web_url(&page_host),
            Err(_) => String::new(),
        }
    }

    /// The short-link cache (contacts.rs shortens invites with it).
    pub fn short_links(&self) -> Arc<ShortLinks> {
        self.short.clone()
    }
}

// ---- conversions --------------------------------------------------------------------------------

fn ticket_info(t: &Ticket, page_host: &str) -> TicketInfo {
    let b = &t.body;
    let host = t
        .host()
        .map(|h| h.to_string())
        .unwrap_or_else(|_| data_encoding::BASE32_NOPAD.encode(&b.host).to_lowercase());
    TicketInfo {
        ticket: t.to_string(),
        id: t.id_hex(),
        host,
        job: b.job.clone(),
        root: b.root.clone(),
        path: b.path.clone(),
        ret: b.ret.clone(),
        get: b.get,
        put: b.put,
        admin: b.admin,
        exp: b.exp,
        web_url: t.web_url(page_host),
        deep_link: t.deep_link(),
    }
}

fn to_core_spec(s: TicketSpec) -> CoreSpec {
    CoreSpec {
        job: s.job,
        root: s.root,
        path: s.path,
        ret: s.ret,
        get: s.get,
        put: s.put,
        days: s.days,
    }
}

fn job_record(j: toss_core::JobRecord, ticket: String) -> JobRecord {
    JobRecord {
        ticket,
        id: hex::encode(j.id),
        job: j.job,
        root: j.root,
        path: j.path,
        get: j.get,
        put: j.put,
        exp: j.exp,
        created: j.created,
        revoked: j.revoked,
        web_url: String::new(),
        last: j.last.map(|e| JobLast {
            at: e.at,
            device: e.device,
            op: match e.op {
                Op::Get => "get",
                Op::Put => "put",
            },
            files: e.files,
            bytes: e.bytes,
            ok: e.ok,
            msg: e.msg,
        }),
    }
}

fn parse_id(s: &str) -> Result<[u8; 16]> {
    let v = hex::decode(s.trim()).map_err(|_| anyhow!("id job không hợp lệ"))?;
    v.try_into().map_err(|_| anyhow!("id job không hợp lệ"))
}

fn clean_name(n: &str) -> Result<String> {
    let n: String = n
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(60)
        .collect();
    if n.is_empty() {
        bail!("Tên máy không được để trống");
    }
    Ok(n)
}

/// UDP port host mode runs on: the configured one, else the default 41820 (None = host mode off).
fn host_port(h: Option<&HostConfig>) -> Option<u16> {
    h.map(|h| h.bind_port.filter(|p| *p != 0).unwrap_or(DEFAULT_HOST_PORT))
}

/// Validates what the UI sent for `set_net`: relay URLs (trimmed, http(s), no duplicates)
/// and tuning bounds, so a typo cannot stop the node from starting.
fn clean_net(mut net: NetConfig) -> Result<NetConfig> {
    let mut urls: Vec<String> = Vec::new();
    for u in net.relay_urls.drain(..) {
        let u = u.trim().trim_end_matches('/').to_string();
        if u.is_empty() || urls.contains(&u) {
            continue;
        }
        let rest = u
            .strip_prefix("https://")
            .or_else(|| u.strip_prefix("http://"))
            .ok_or_else(|| {
                anyhow!("Địa chỉ relay phải bắt đầu bằng https:// (vd. https://relay.example.vn)")
            })?;
        if rest.is_empty()
            || rest.contains(char::is_whitespace)
            || (!rest.contains('.') && !rest.starts_with("localhost"))
        {
            bail!("Địa chỉ relay “{u}” không hợp lệ");
        }
        urls.push(u);
    }
    net.relay_urls = urls;
    let t = &mut net.tuning;
    t.streams = t.streams.clamp(1, 32);
    t.streams_high_rtt = t.streams_high_rtt.clamp(t.streams, 64);
    Ok(net)
}

fn valid_alias(a: &str) -> bool {
    !a.is_empty()
        && a.len() <= 32
        && a.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `strict` (user pressed save): roots must exist. Not strict (startup): tolerate an offline drive.
fn to_core_host(h: &HostConfig, strict: bool) -> Result<CoreHost> {
    let mut seen = HashSet::new();
    let mut roots = Vec::new();
    for Root { alias, path } in &h.roots {
        let path = path.trim();
        if !valid_alias(alias) {
            bail!("Tên gốc “{alias}” không hợp lệ (chỉ chữ, số, - và _, tối đa 32 ký tự)");
        }
        if !seen.insert(alias.clone()) {
            bail!("Tên gốc “{alias}” bị trùng");
        }
        let p = PathBuf::from(path);
        if strict && !p.is_dir() {
            bail!("Không tìm thấy thư mục {path}");
        }
        roots.push(CoreRoot {
            alias: alias.clone(),
            path: p,
        });
    }
    if h.return_root.trim().is_empty() {
        bail!("Chưa chọn thư mục nhận file gửi lại");
    }
    let ret = PathBuf::from(h.return_root.trim());
    if let Err(e) = std::fs::create_dir_all(&ret) {
        if strict {
            bail!(
                "Không tạo được thư mục nhận file gửi lại {}: {e}",
                h.return_root
            );
        }
    }
    let mut c = CoreHost::new(roots, ret);
    c.max_parallel_jobs = h.max_parallel_jobs.clamp(1, 16);
    c.max_parallel_streams = 4;
    c.bind_port = h.bind_port.filter(|p| *p != 0);
    Ok(c)
}

/// The saved host config restricted to the roots that exist right now, if that
/// is a non-empty strict subset.
fn available_roots(h: &HostConfig) -> Option<HostConfig> {
    let roots: Vec<Root> = h
        .roots
        .iter()
        .filter(|r| PathBuf::from(r.path.trim()).is_dir())
        .cloned()
        .collect();
    (!roots.is_empty() && roots.len() < h.roots.len()).then(|| HostConfig { roots, ..h.clone() })
}

/// `qs-` + first 8 hex chars of blake3(canonical path).
pub fn quick_share_alias(canonical: &str) -> String {
    let h = blake3::hash(canonical.as_bytes());
    format!("{QS_PREFIX}{}", &h.to_hex()[..8])
}

/// Job name for a shared folder: its (sanitized) name; a drive root uses the path.
fn share_job_name(p: &Path) -> String {
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| {
            p.to_string_lossy()
                .trim_end_matches(['/', '\\'])
                .to_string()
        });
    toss_core::paths::sanitize_component(&name, "Toss")
}

/// Host config as the UI sees it: quick-share roots stripped.
fn user_host(s: &Settings) -> Option<HostConfig> {
    s.host.clone().map(|mut h| {
        h.roots
            .retain(|r| !s.quick_shares.iter().any(|q| q.alias == r.alias));
        h
    })
}

/// `h` (user roots) + quick-share roots. `existing_only`: skip quick shares whose
/// folder is gone (interactive changes must not fail because of them).
fn merge_quick_shares(h: &HostConfig, shares: &[QuickShare], existing_only: bool) -> HostConfig {
    let mut out = h.clone();
    out.roots
        .retain(|r| !shares.iter().any(|q| q.alias == r.alias));
    for q in shares {
        if existing_only && !Path::new(&q.path).is_dir() {
            tracing::warn!("chia sẻ nhanh {}: không thấy {}", q.alias, q.path);
            continue;
        }
        out.roots.push(Root {
            alias: q.alias.clone(),
            path: q.path.clone(),
        });
    }
    out
}

/// The config host mode actually runs with (None = host mode off).
fn effective_host(s: &Settings, existing_only: bool) -> Option<HostConfig> {
    s.host
        .as_ref()
        .map(|h| merge_quick_shares(h, &s.quick_shares, existing_only))
}

fn is_active(v: &Value) -> bool {
    matches!(
        v["status"].as_str(),
        Some("connecting" | "running" | "paused")
    )
}

/// What the forwarder knows about each transfer id.
#[derive(Default)]
struct Seen {
    /// Started and not yet finished, to word notifications and history entries.
    open: HashMap<String, Open>,
    /// A final event (done / final error) was forwarded.
    finished: HashSet<String>,
}

struct Open {
    job: String,
    client: bool,
    put: bool,
    peer: String,
}

/// What a final event means for the user.
#[derive(Debug, PartialEq)]
enum Track {
    /// Plain notification (sent, failed).
    Notify(String),
    /// v0.8: files arrived (client get, or a push / link put received here).
    Received(Received),
}

impl Seen {
    /// Book-keeping for one event. Returns what to tell the user, if anything.
    fn track(&mut self, v: &Value) -> Option<Track> {
        let id = v["id"].as_str().unwrap_or("").to_string();
        match v["type"].as_str().unwrap_or("") {
            "started" => {
                self.open.insert(
                    id,
                    Open {
                        job: v["job"].as_str().unwrap_or("").to_string(),
                        client: v["side"] == "client",
                        put: v["op"] == "put",
                        peer: v["peer"].as_str().unwrap_or("").to_string(),
                    },
                );
                None
            }
            "done" => {
                self.finished.insert(id.clone());
                let o = self.open.remove(&id)?;
                let receive = o.client != o.put; // client get, or host side of a put / push
                match v["dest"].as_str() {
                    Some(dest) if receive => Some(Track::Received(Received {
                        id,
                        from_name: if o.peer.is_empty() {
                            "máy khác".into()
                        } else {
                            o.peer
                        },
                        job: o.job,
                        files: v["files"].as_u64().unwrap_or(0) as u32,
                        bytes: v["bytes"].as_u64().unwrap_or(0),
                        dest: dest.to_string(),
                        at: toss_core::util::now_unix(),
                    })),
                    _ if o.client => Some(Track::Notify(format!("Đã gửi xong: {}", o.job))),
                    _ => None,
                }
            }
            "error" if v["retry_in_s"].is_null() => {
                self.finished.insert(id.clone());
                let o = self.open.remove(&id)?;
                (o.client && v["code"] != "cancelled").then(|| {
                    Track::Notify(format!(
                        "Lỗi khi chuyển “{}”: {}",
                        o.job,
                        v["msg"].as_str().unwrap_or("")
                    ))
                })
            }
            _ => None,
        }
    }
}

/// Events (in the ipc.ts `TransferEvent` shapes) that bring the UI back in sync
/// after the forwarder lagged: `started` for transfers it never announced, a
/// `progress` snapshot for each, and the resting event (`paused`, or the final
/// `done` / `error` it may have missed).
fn resync_events(snaps: &[Value], seen: &Seen) -> Vec<Value> {
    use serde_json::json;
    let now = toss_core::util::now_unix();
    let mut out = Vec::new();
    for s in snaps {
        let Some(id) = s["id"].as_str() else { continue };
        if seen.finished.contains(id) {
            continue;
        }
        if !seen.open.contains_key(id) {
            out.push(json!({ "type": "started", "id": id, "job": s["job"], "op": s["op"], "side": s["side"], "peer": s["peer"] }));
        }
        out.push(json!({
            "type": "progress", "id": id, "bytes_done": s["bytes_done"], "bytes_total": s["bytes_total"],
            "files_done": s["files_done"], "files_total": s["files_total"], "speed_bps": s["speed_bps"],
            "eta_s": s["eta_s"], "path": s["path"],
        }));
        match s["status"].as_str().unwrap_or("") {
            "paused" => out.push(json!({ "type": "paused", "id": id })),
            "done" => out.push(json!({
                "type": "done", "id": id, "files": s["files_total"], "bytes": s["bytes_total"],
                "elapsed_s": now.saturating_sub(s["started_at"].as_u64().unwrap_or(now)), "dest": s["dest"],
            })),
            "error" => out.push(json!({
                "type": "error", "id": id, "code": "other", "msg": s["error"].as_str().unwrap_or(""), "retry_in_s": null,
            })),
            "cancelled" => out.push(json!({
                "type": "error", "id": id, "code": "cancelled", "msg": "cancelled", "retry_in_s": null,
            })),
            _ => {}
        }
    }
    out
}

/// Emit one core event to the UI (+ derived events, notifications, history).
fn forward(app: &AppHandle, seen: &mut Seen, v: Value) {
    // v0.5.1: an admin device asked this host to update the app (not a UI event).
    if v["type"] == "update_requested" {
        crate::updater::on_remote_request(app, v["device"].as_str().unwrap_or("máy admin"));
        return;
    }
    nearby::apply(app, core_effects(seen, v));
}

/// What one core event means for the app: always `transfer:event`, plus the event's own UI name
/// (v0.5 / v0.8), the unified `incoming` ask, notifications and received history.
fn core_effects(seen: &mut Seen, v: Value) -> Vec<Effect> {
    let mut fx = vec![Effect::Emit("transfer:event", v.clone())];
    match v["type"].as_str().unwrap_or("") {
        "nearby_changed" => {
            fx.push(Effect::Emit("nearby:changed", json!({})));
            return fx;
        }
        "text_incoming" => {
            let msg = TextMsg {
                from_name: v["from_name"].as_str().unwrap_or("Máy khác").to_string(),
                text: v["text"].as_str().unwrap_or("").to_string(),
                at: v["at"].as_u64().unwrap_or_else(toss_core::util::now_unix),
            };
            fx.push(Effect::Emit(
                "text",
                serde_json::to_value(&msg).unwrap_or_default(),
            ));
            fx.push(Effect::Notify {
                body: nearby::text_note(&msg.from_name, &msg.text),
                open: None,
            });
            return fx;
        }
        "host_status" => {
            let mut p = v.clone();
            if let Some(o) = p.as_object_mut() {
                o.remove("type");
            }
            fx.push(Effect::Emit("host:status", p));
            return fx;
        }
        _ => {}
    }
    // v0.5: contacts / presence / pushes / Local Network events also get their own UI event name.
    if let Some((name, payload)) = v05_event(&v) {
        let push = (name == "push:incoming")
            .then(|| serde_json::from_value::<IncomingPush>(payload.clone()).ok())
            .flatten();
        fx.push(Effect::Emit(name, payload));
        // v0.8: the same ask in the unified shape; window comes forward.
        if let Some(push) = push {
            let inc = nearby::toss_incoming(&push);
            fx.push(Effect::Emit(
                "incoming",
                serde_json::to_value(&inc).unwrap_or_default(),
            ));
            fx.push(Effect::Notify {
                body: nearby::ask_text(&inc.from_name, &inc.job),
                open: None,
            });
            fx.push(Effect::Show);
            fx.push(Effect::CloseLater {
                id: inc.id,
                secs: nearby::TOSS_ASK_SECS,
                toss: true,
            });
        }
        return fx;
    }
    match seen.track(&v) {
        Some(Track::Notify(body)) => fx.push(Effect::Notify { body, open: None }),
        Some(Track::Received(r)) => fx.push(Effect::Received(r)),
        None => {}
    }
    fx
}

/// Maps a v0.5 core event (serde tag `type`) to the UI event of contracts/contacts-v0.5.md §4.
/// None for every other event (transfer / host events keep their old route).
fn v05_event(v: &Value) -> Option<(&'static str, Value)> {
    use serde_json::json;
    let without_type = || {
        let mut p = v.clone();
        if let Some(o) = p.as_object_mut() {
            o.remove("type");
        }
        p
    };
    match v["type"].as_str()? {
        "contacts_changed" => Some(("contacts:changed", json!({}))),
        "contact_presence" => Some((
            "contact:presence",
            json!({ "id": v["id"], "online": v["online"], "rtt_ms": v["rtt_ms"] }),
        )),
        "push_incoming" => Some(("push:incoming", without_type())),
        "lan_blocked" => Some(("net:lan_blocked", json!({ "blocked": v["blocked"] }))),
        _ => None,
    }
}

pub(crate) fn notify(app: &AppHandle, body: &str) {
    notify_open(app, body, None)
}

/// Folders to open when a notification is clicked, by notification id (newest last).
static CLICKS: StdMutex<Vec<(i32, String)>> = StdMutex::new(Vec::new());
const MAX_CLICKS: usize = 50;

/// A notification; clicking it opens `open` (a received folder) when set (v0.8).
pub(crate) fn notify_open(app: &AppHandle, body: &str, open: Option<String>) {
    // Skip when the user is looking at the window anyway.
    if let Some(w) = app.get_webview_window(config::MAIN_WINDOW) {
        if w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) {
            return;
        }
    }
    let mut b = app.notification().builder().title("Toss").body(body);
    if let Some(path) = open {
        static NEXT: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        remember_click(id, &path);
        b = b.id(id).extra("open", path);
    }
    if let Err(e) = b.show() {
        tracing::debug!("notification failed: {e}");
    }
}

fn remember_click(id: i32, path: &str) {
    let mut c = CLICKS.lock().unwrap_or_else(|p| p.into_inner());
    c.push((id, path.to_string()));
    let excess = c.len().saturating_sub(MAX_CLICKS);
    c.drain(..excess);
}

/// Folder behind a clicked notification (`tap`): by notification id, else its `open` extra.
pub(crate) fn clicked_folder(a: &tauri_plugin_notification::ActionPerformed) -> Option<String> {
    if a.action_id() != "tap" {
        return None;
    }
    let n = a.notification()?;
    let by_id = CLICKS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .find(|(id, _)| *id == n.id())
        .map(|(_, p)| p.clone());
    by_id.or_else(|| n.extra().get("open")?.as_str().map(str::to_string))
}

/// QR code as SVG markup with a white quiet zone; scales to its container (no fixed width/height).
pub fn qr_svg(text: &str) -> Result<String> {
    use qrcode::{render::svg, EcLevel, QrCode};
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M)
        .map_err(|e| anyhow!("không tạo được mã QR: {e}"))?;
    let raw = code
        .render::<svg::Color>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build();
    let start = raw.find("<svg").ok_or_else(|| anyhow!("QR svg rỗng"))?;
    let rest = &raw[start..];
    let end = rest.find('>').ok_or_else(|| anyhow!("QR svg hỏng"))?;
    let tag: Vec<&str> = rest[..end]
        .split_whitespace()
        .filter(|tok| !tok.starts_with("width=") && !tok.starts_with("height="))
        .collect();
    Ok(format!(
        "{} width=\"100%\" height=\"100%\"{}",
        tag.join(" "),
        &rest[end..]
    ))
}

/// Tickets embed the host's relay URL (wire.md §1 v2). Right after the node
/// starts the relay may not be known yet: give it a moment (no wait once known,
/// at most 2 s so the UI never hangs on an offline machine).
async fn wait_for_relay(node: &Node) {
    if node.host_addr().relay.is_empty() {
        node.wait_online(Duration::from_secs(2)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;


    /// A long `#toss1…` link must parse offline; a short link needs the link service,
    /// which CI runners reach only sometimes — then the ticket string itself is checked.
    async fn parse_link_or_ticket(core: &Core, info: &TicketInfo) -> TicketInfo {
        match core.parse_ticket(&info.web_url).await {
            Ok(p) => p,
            Err(e) if !info.web_url.contains('#') => {
                eprintln!("short link not reachable here ({e:#}); checking the ticket");
                core.parse_ticket(&info.ticket).await.unwrap()
            }
            Err(e) => panic!("{}: {e:#}", info.web_url),
        }
    }

    #[test]
    fn qr_is_scalable_svg() {
        let svg = qr_svg("https://toss-link.vercel.app/#toss1abcdef").unwrap();
        assert!(svg.starts_with("<svg"), "{}", &svg[..svg.len().min(200)]);
        assert!(svg.contains("viewBox"));
        assert!(svg.contains("width=\"100%\""));
        let tag = &svg[..svg.find('>').unwrap()];
        assert_eq!(tag.matches("width=").count(), 1, "{tag}");
    }

    #[test]
    fn aliases() {
        assert!(valid_alias("RAW"));
        assert!(valid_alias("Tra-ve_1"));
        assert!(!valid_alias(""));
        assert!(!valid_alias("có dấu"));
        assert!(!valid_alias(&"a".repeat(33)));
    }

    #[test]
    fn host_port_defaults_to_41820() {
        assert_eq!(host_port(None), None, "host mode off: no fixed port");
        let mut h = HostConfig::default();
        assert_eq!(host_port(Some(&h)), Some(DEFAULT_HOST_PORT));
        h.bind_port = Some(0);
        assert_eq!(host_port(Some(&h)), Some(DEFAULT_HOST_PORT));
        h.bind_port = Some(50000);
        assert_eq!(host_port(Some(&h)), Some(50000));
    }

    #[test]
    fn set_net_validation() {
        let mut n = NetConfig {
            relay_urls: vec![
                " https://relay.example.vn/ ".into(),
                "https://relay.example.vn".into(),
                "".into(),
            ],
            ..NetConfig::default()
        };
        n.tuning.streams = 99;
        n.tuning.streams_high_rtt = 4;
        let c = clean_net(n).unwrap();
        assert_eq!(
            c.relay_urls,
            ["https://relay.example.vn"],
            "trimmed, de-duplicated"
        );
        assert_eq!((c.tuning.streams, c.tuning.streams_high_rtt), (32, 32));
        for bad in [
            "relay.example.vn",
            "ftp://x.vn",
            "https://",
            "https://no spaces.vn",
        ] {
            let n = NetConfig {
                relay_urls: vec![bad.into()],
                ..NetConfig::default()
            };
            assert!(clean_net(n).is_err(), "{bad}");
        }
    }

    #[test]
    fn host_config_validation() {
        let h = HostConfig {
            roots: vec![Root {
                alias: "RAW".into(),
                path: "/definitely/not/here".into(),
            }],
            return_root: std::env::temp_dir()
                .join("toss-ret-test")
                .to_string_lossy()
                .into_owned(),
            max_parallel_jobs: 99,
            bind_port: Some(41641),
        };
        assert!(to_core_host(&h, true).is_err()); // root missing
        let c = to_core_host(&h, false).unwrap(); // offline drive tolerated at startup
        assert_eq!(c.max_parallel_jobs, 16);
        assert_eq!(c.bind_port, Some(41641));
        assert_eq!(
            to_core_host(
                &HostConfig {
                    bind_port: Some(0),
                    ..h.clone()
                },
                false
            )
            .unwrap()
            .bind_port,
            None
        );
        let dup = HostConfig {
            roots: vec![
                Root {
                    alias: "A".into(),
                    path: ".".into(),
                },
                Root {
                    alias: "A".into(),
                    path: ".".into(),
                },
            ],
            ..h
        };
        assert!(to_core_host(&dup, false).is_err());
    }

    /// Whole adapter against the real toss-core, offline (no peers needed).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn adapter_end_to_end() {
        let base = std::env::temp_dir().join(format!("toss-desktop-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (data, raw, ret, dest) = (
            base.join("data"),
            base.join("raw"),
            base.join("ret"),
            base.join("dest"),
        );
        std::fs::create_dir_all(raw.join("2026/12A3")).unwrap();
        std::fs::write(raw.join("2026/12A3/a.cr3"), b"hello").unwrap();
        std::env::set_var(config::DATA_DIR_ENV, &data);

        let core = Core::new();
        let st = core.state().await.unwrap();
        assert!(st.first_run);
        assert!(!st.endpoint_id.is_empty());

        // wizard: v0.8 renames live (no node restart), identity kept
        let st2 = core
            .finish_setup("May test".into(), dest.to_string_lossy().into_owned())
            .await
            .unwrap();
        assert!(!st2.first_run);
        assert_eq!(st2.device_name, "May test");
        assert_eq!(st2.endpoint_id, st.endpoint_id);
        assert_eq!(core.settings().device_name, "May test");
        // v0.8 state fields
        assert_eq!(st2.receive_policy, crate::settings::ReceivePolicy::Contacts);
        assert!(st2.ls_enabled && st2.ls_trusted.is_empty());
        let v = serde_json::to_value(&st2).unwrap();
        assert_eq!(v["receive_policy"], "contacts");
        assert!(v.get("ls_port").is_some());

        // host mode
        let cfg = HostConfig {
            roots: vec![Root {
                alias: "RAW".into(),
                path: raw.to_string_lossy().into_owned(),
            }],
            return_root: ret.to_string_lossy().into_owned(),
            max_parallel_jobs: 2,
            bind_port: None,
        };
        core.set_host(Some(cfg.clone())).await.unwrap();
        assert_eq!(core.settings().host, Some(cfg));
        assert!(core.host_status().await.unwrap()["roots"][0] == "RAW");
        let dir = core.list_dir("RAW".into(), "2026".into()).await.unwrap();
        assert_eq!(dir[0]["name"], "12A3");

        // tickets: create -> parse round-trips; job list carries the re-signed ticket
        let spec = TicketSpec {
            job: "12A3 - RAW".into(),
            root: "RAW".into(),
            path: "2026/12A3".into(),
            ret: String::new(),
            get: true,
            put: true,
            days: 7,
        };
        let info = core.create_ticket(spec).await.unwrap();
        assert!(info.ticket.starts_with("toss1"));
        // Long link offline; with network the short-link service gives https://<host>/<code>.
        let host = config::DEFAULT_PAGE_HOST;
        assert!(
            info.web_url.starts_with(&format!("https://{host}/#toss1"))
                || (info.web_url.starts_with(&format!("https://{host}/"))
                    && !info.web_url.contains('#')),
            "{}",
            info.web_url
        );
        assert_eq!(info.deep_link, format!("toss://t/{}", info.ticket));
        let parsed = parse_link_or_ticket(&core, &info).await;
        assert_eq!(
            (parsed.job.as_str(), parsed.get, parsed.put, parsed.admin),
            ("12A3 - RAW", true, true, false)
        );
        assert_eq!(parsed.host, st.endpoint_id);
        let jobs = core.list_jobs().await.unwrap();
        assert_eq!(jobs.len(), 1);
        // Re-signed with the host's current addresses: same job, same id
        // (the string only differs if the relay/direct addresses changed).
        let relisted = core.parse_ticket(&jobs[0].ticket).await.unwrap();
        assert_eq!(
            (
                relisted.id.as_str(),
                relisted.job.as_str(),
                relisted.host.as_str()
            ),
            (info.id.as_str(), "12A3 - RAW", st.endpoint_id.as_str())
        );
        core.revoke(jobs[0].id.clone()).await.unwrap();
        assert!(core.list_jobs().await.unwrap()[0].revoked);
        assert!(core.parse_ticket("khong phai link").await.is_err());

        // admin ticket
        let admin = core.create_admin_ticket(30).await.unwrap();
        assert!(admin.admin && !admin.get && !admin.put);
        assert!(core
            .set_admin_ticket(Some(info.ticket.clone()))
            .await
            .is_err()); // not an admin ticket
        assert!(core
            .remote_status()
            .await
            .unwrap_err()
            .to_string()
            .contains("no_admin_ticket"));
        core.set_admin_ticket(Some(admin.ticket.clone()))
            .await
            .unwrap();
        assert!(core.settings().admin_ticket.is_some());
        core.set_admin_ticket(None).await.unwrap();

        // settings survive a restart
        let again = Core::new();
        assert_eq!(again.settings().device_name, "May test");
        assert!(again.settings().host.is_some());

        assert!(core.list_transfers().await.unwrap().is_empty());
        core.clear_finished().await.unwrap();
        core.shutdown().await;
        let _ = std::fs::remove_dir_all(&base);
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("toss-desktop-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn ipc_host_config_bind_port_is_optional() {
        let h: HostConfig =
            serde_json::from_str(r#"{"roots":[],"return_root":"x","max_parallel_jobs":2}"#)
                .unwrap();
        assert_eq!(h.bind_port, None);
        assert!(!serde_json::to_string(&h).unwrap().contains("bind_port"));
        let h: HostConfig = serde_json::from_str(
            r#"{"roots":[],"return_root":"x","max_parallel_jobs":2,"bind_port":41641}"#,
        )
        .unwrap();
        assert_eq!(h.bind_port, Some(41641));
        let h: HostConfig = serde_json::from_str(
            r#"{"roots":[],"return_root":"x","max_parallel_jobs":2,"bind_port":null}"#,
        )
        .unwrap();
        assert_eq!(h.bind_port, None);
    }

    /// Concurrent settings updates: every change must reach disk, none may be
    /// overwritten by an older snapshot, and no save may fail on the temp file.
    #[test]
    fn concurrent_settings_updates_are_not_lost() {
        let dir = tmpdir("settings-race");
        let core = Arc::new(Core::with_dir(dir.clone()));
        let threads: Vec<_> = (0..4)
            .map(|t| {
                let core = core.clone();
                std::thread::spawn(move || {
                    for i in 0..50 {
                        core.update_settings(|s| match t {
                            0 => s.device_name = format!("name {i}"),
                            1 => s.dest_dir = format!("dest {i}"),
                            2 => s.page_host = Some(format!("host{i}")),
                            _ => s.autostart = i % 2 == 0,
                        })
                        .expect("save");
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let disk = Settings::load(&dir);
        assert_eq!(disk.device_name, "name 49");
        assert_eq!(disk.dest_dir, "dest 49");
        assert_eq!(disk.page_host.as_deref(), Some("host49"));
        assert!(!disk.autostart);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Shutdown must close the endpoint even while a command still holds the node.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_closes_a_shared_node() {
        let dir = tmpdir("shutdown");
        let core = Core::with_dir(dir.clone());
        let held = core.node().await.unwrap(); // e.g. an in-flight command
        core.shutdown().await;
        assert!(
            held.endpoint().is_closed(),
            "node left running after shutdown"
        );
        // A later command starts a fresh node.
        let again = core.node().await.unwrap();
        assert!(!again.endpoint().is_closed());
        assert_eq!(again.endpoint_id(), held.endpoint_id());
        core.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A host whose root is not mounted at startup comes up with the roots it has.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_mode_survives_a_missing_root_at_startup() {
        let dir = tmpdir("host-start");
        let (a, missing, ret) = (dir.join("a"), dir.join("not-mounted"), dir.join("ret"));
        std::fs::create_dir_all(&a).unwrap();
        let h = HostConfig {
            roots: vec![
                Root {
                    alias: "A".into(),
                    path: a.to_string_lossy().into_owned(),
                },
                Root {
                    alias: "B".into(),
                    path: missing.to_string_lossy().into_owned(),
                },
            ],
            return_root: ret.to_string_lossy().into_owned(),
            max_parallel_jobs: 2,
            bind_port: None,
        };
        let core = Core::with_dir(dir.join("data"));
        core.update_settings(|s| s.host = Some(h.clone())).unwrap();
        let node = core.node().await.unwrap();
        let st = node
            .host_status()
            .expect("host mode must be on with the available root");
        assert_eq!(st.roots, vec!["A".to_string()]);
        // Once the drive is there, the retry applies the full config.
        std::fs::create_dir_all(&missing).unwrap();
        assert!(core.apply_host_at_start(&node, &h).await);
        assert_eq!(
            node.host_status().unwrap().roots,
            vec!["A".to_string(), "B".to_string()]
        );
        drop(node);
        core.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quick_share_alias_is_stable_and_valid() {
        let a = quick_share_alias("/Users/minh/Ảnh/12A3");
        assert_eq!(a, quick_share_alias("/Users/minh/Ảnh/12A3"));
        assert_ne!(a, quick_share_alias("/Users/minh/Ảnh/12A4"));
        assert!(
            a.starts_with("qs-") && a.len() == 11 && valid_alias(&a),
            "{a}"
        );
        let hex = &blake3::hash("/x".as_bytes()).to_hex()[..8];
        assert_eq!(quick_share_alias("/x"), format!("qs-{hex}"));
        assert_eq!(share_job_name(Path::new("/a/Lớp 12A3")), "Lớp 12A3");
        assert_eq!(share_job_name(Path::new("/a/x:y")), "x_y");
    }

    #[test]
    fn quick_share_roots_are_merged_and_hidden() {
        let qs = QuickShare {
            alias: "qs-00000000".into(),
            path: std::env::temp_dir().to_string_lossy().into_owned(),
            job_id: None,
        };
        let gone = QuickShare {
            alias: "qs-11111111".into(),
            path: "/definitely/not/here".into(),
            job_id: None,
        };
        let user = HostConfig {
            roots: vec![Root {
                alias: "RAW".into(),
                path: "/raw".into(),
            }],
            return_root: "/ret".into(),
            max_parallel_jobs: 2,
            bind_port: None,
        };
        let s = Settings {
            host: Some(user.clone()),
            quick_shares: vec![qs.clone(), gone.clone()],
            ..Settings::default()
        };
        let aliases = |h: &HostConfig| h.roots.iter().map(|r| r.alias.clone()).collect::<Vec<_>>();
        assert_eq!(
            aliases(&effective_host(&s, false).unwrap()),
            ["RAW", "qs-00000000", "qs-11111111"]
        );
        assert_eq!(
            aliases(&effective_host(&s, true).unwrap()),
            ["RAW", "qs-00000000"]
        );
        assert_eq!(user_host(&s), Some(user));
        let off = Settings { host: None, ..s };
        assert_eq!(effective_host(&off, false), None);
    }

    /// Roots the running host serves (empty = host mode off).
    async fn roots(core: &Core) -> Vec<String> {
        core.node()
            .await
            .unwrap()
            .host_status()
            .map(|h| h.roots)
            .unwrap_or_default()
    }

    /// Right-click share against the real core, offline.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn share_folder_end_to_end() {
        let dir = tmpdir("quick-share");
        let (data, dest, raw) = (dir.join("data"), dir.join("dest"), dir.join("raw"));
        let folder = dir.join("photos").join("Lớp 12A3");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::create_dir_all(&raw).unwrap();
        std::fs::write(folder.join("a.jpg"), b"x").unwrap();
        std::fs::write(dir.join("file.txt"), b"x").unwrap();
        let core = Core::with_dir(data.clone());
        core.update_settings(|s| s.dest_dir = dest.to_string_lossy().into_owned())
            .unwrap();
        let canon = toss_core::paths::canonicalize(&folder).unwrap();
        let alias = quick_share_alias(&canon.to_string_lossy());

        // 1. host mode off -> share turns it on with only the quick-share root.
        assert!(core.settings().host.is_none());
        let info = core
            .share_folder(folder.to_string_lossy().into_owned(), 7, false)
            .await
            .unwrap();
        assert_eq!(info.root, alias);
        assert_eq!((info.job.as_str(), info.path.as_str()), ("Lớp 12A3", ""));
        assert!(info.get && !info.put && !info.admin && info.exp > 0);
        let parsed = parse_link_or_ticket(&core, &info).await;
        assert_eq!(
            (parsed.id.as_str(), parsed.root.as_str()),
            (info.id.as_str(), alias.as_str())
        );
        let s = core.settings();
        assert!(s.host_auto);
        let h = s.host.clone().unwrap();
        assert!(
            h.roots.is_empty(),
            "quick-share roots never stored in host.roots"
        );
        assert_eq!(PathBuf::from(&h.return_root), dest.join("Tra-ve"));
        assert!(dest.join("Tra-ve").is_dir());
        assert_eq!(roots(&core).await, vec![alias.clone()]);
        assert!(core.state().await.unwrap().host.unwrap().roots.is_empty());
        assert_eq!(
            core.list_quick_shares(),
            vec![QuickShare {
                alias: alias.clone(),
                path: canon.to_string_lossy().into_owned(),
                job_id: Some(info.id.clone()),
            }]
        );
        let jobs = core.list_jobs().await.unwrap();
        assert!(jobs
            .iter()
            .any(|j| j.id == info.id && j.root == alias && j.get));
        let listed = core.list_dir(alias.clone(), String::new()).await.unwrap();
        assert_eq!(listed[0]["name"], "a.jpg");

        // 2. re-share: fresh ticket, still one root.
        let again = core
            .share_folder(format!("{}/", folder.to_string_lossy()), 3, true)
            .await
            .unwrap();
        assert_ne!(again.id, info.id);
        assert!(again.put);
        assert_eq!(core.list_quick_shares().len(), 1);
        assert_eq!(
            core.list_quick_shares()[0].job_id.as_deref(),
            Some(again.id.as_str())
        );
        assert_eq!(roots(&core).await, vec![alias.clone()]);

        // v0.8: a single file → its parent is the root, the file is the ticket path
        let f = core
            .share_folder(
                dir.join("file.txt").to_string_lossy().into_owned(),
                7,
                false,
            )
            .await
            .unwrap();
        assert_eq!((f.job.as_str(), f.path.as_str()), ("file", "file.txt"));
        let parent_alias = quick_share_alias(
            &toss_core::paths::canonicalize(&dir)
                .unwrap()
                .to_string_lossy(),
        );
        assert_eq!(f.root, parent_alias);
        core.remove_quick_share(parent_alias).await.unwrap();
        assert!(core
            .share_folder(dir.join("nope").to_string_lossy().into_owned(), 7, false)
            .await
            .is_err());

        // 3. remove: every ticket revoked, host mode back off.
        core.remove_quick_share(alias.clone()).await.unwrap();
        let jobs = core.list_jobs().await.unwrap();
        assert!(jobs.iter().filter(|j| j.root == alias).all(|j| j.revoked));
        assert_eq!(jobs.iter().filter(|j| j.root == alias).count(), 2);
        assert!(core.list_quick_shares().is_empty());
        let s = core.settings();
        assert!(s.host.is_none() && !s.host_auto);
        assert!(core.node().await.unwrap().host_status().is_none());
        assert!(core.remove_quick_share(alias.clone()).await.is_err());

        // 4. user host mode + quick share: the UI's set_host keeps the quick-share root.
        let user = HostConfig {
            roots: vec![Root {
                alias: "RAW".into(),
                path: raw.to_string_lossy().into_owned(),
            }],
            return_root: dir.join("ret").to_string_lossy().into_owned(),
            max_parallel_jobs: 2,
            bind_port: None,
        };
        core.set_host(Some(user.clone())).await.unwrap();
        core.share_folder(folder.to_string_lossy().into_owned(), 7, false)
            .await
            .unwrap();
        assert!(!core.settings().host_auto, "host mode belongs to the user");
        assert_eq!(roots(&core).await, vec!["RAW".to_string(), alias.clone()]);
        core.set_host(Some(user.clone())).await.unwrap(); // Settings UI save
        assert_eq!(roots(&core).await, vec!["RAW".to_string(), alias.clone()]);
        assert_eq!(core.settings().host, Some(user.clone()));
        assert_eq!(core.state().await.unwrap().host, Some(user.clone()));
        let bad = HostConfig {
            roots: vec![Root {
                alias: "qs-mine".into(),
                path: raw.to_string_lossy().into_owned(),
            }],
            ..user.clone()
        };
        assert!(core.set_host(Some(bad)).await.is_err());

        // 5. quick-share roots survive a restart.
        core.shutdown().await;
        drop(core);
        let core = Core::with_dir(data.clone());
        assert_eq!(roots(&core).await, vec!["RAW".to_string(), alias.clone()]);
        // removing it keeps the user's host mode on
        core.remove_quick_share(alias.clone()).await.unwrap();
        assert_eq!(roots(&core).await, vec!["RAW".to_string()]);
        assert_eq!(core.settings().host, Some(user));

        core.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn snap(id: &str, status: &str) -> Value {
        serde_json::json!({
            "id": id, "job": "J", "op": "get", "side": "client", "peer": "p", "status": status,
            "bytes_done": 5, "bytes_total": 10, "files_done": 1, "files_total": 2, "speed_bps": 0,
            "eta_s": null, "path": "direct", "dest": "/d", "error": "boom", "started_at": 0,
        })
    }

    #[test]
    fn resync_after_lag_restores_missed_events() {
        let mut seen = Seen::default();
        // Seen: "a" started; "b" started and finished; "c" never seen.
        seen.track(
            &serde_json::json!({"type": "started", "id": "a", "job": "J", "side": "client"}),
        );
        seen.track(
            &serde_json::json!({"type": "started", "id": "b", "job": "J", "side": "client"}),
        );
        assert!(seen
            .track(&serde_json::json!({"type": "done", "id": "b", "dest": "/d"}))
            .is_some());
        let evs = resync_events(
            &[snap("a", "done"), snap("b", "done"), snap("c", "paused")],
            &seen,
        );
        let kinds: Vec<(String, String)> = evs
            .iter()
            .map(|e| {
                (
                    e["id"].as_str().unwrap().to_string(),
                    e["type"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        let k = |id: &str, t: &str| (id.to_string(), t.to_string());
        assert_eq!(
            kinds,
            vec![
                k("a", "progress"),
                k("a", "done"),
                k("c", "started"),
                k("c", "progress"),
                k("c", "paused")
            ]
        );
        // Shapes match ipc.ts.
        assert_eq!(evs[1]["files"], 2);
        assert_eq!(evs[1]["bytes"], 10);
        assert!(evs[1]["elapsed_s"].is_u64());
        assert_eq!(evs[2]["op"], "get");
        // The synthesized final event notifies once, like the real one.
        assert!(seen.track(&evs[1]).is_some());
        assert!(seen.track(&evs[1]).is_none());
        let errs = resync_events(
            &[snap("d", "error"), snap("e", "cancelled")],
            &Seen::default(),
        );
        assert!(errs
            .iter()
            .any(|e| e["type"] == "error" && e["code"] == "other" && e["msg"] == "boom"));
        assert!(errs
            .iter()
            .any(|e| e["type"] == "error" && e["code"] == "cancelled"));
    }

    #[test]
    fn notifications_only_for_client_side_finals() {
        let mut seen = Seen::default();
        seen.track(&serde_json::json!({"type": "started", "id": "h", "job": "J", "side": "host"}));
        assert!(seen
            .track(&serde_json::json!({"type": "done", "id": "h", "dest": null}))
            .is_none());
        seen.track(
            &serde_json::json!({"type": "started", "id": "c", "job": "J", "side": "client"}),
        );
        assert!(seen
            .track(&serde_json::json!({"type": "error", "id": "c", "code": "busy", "retry_in_s": 30, "msg": ""}))
            .is_none());
        assert!(seen
            .track(&serde_json::json!({"type": "error", "id": "c", "code": "cancelled", "retry_in_s": null, "msg": ""}))
            .is_none());
        assert!(seen.finished.contains("c"));
    }

    #[test]
    fn v05_events_get_their_own_ui_event() {
        use serde_json::json;
        assert_eq!(
            v05_event(&json!({"type": "contacts_changed"})),
            Some(("contacts:changed", json!({})))
        );
        assert_eq!(
            v05_event(
                &json!({"type": "contact_presence", "id": "ab", "online": true, "rtt_ms": 12})
            ),
            Some((
                "contact:presence",
                json!({"id": "ab", "online": true, "rtt_ms": 12})
            ))
        );
        let (name, p) = v05_event(&json!({
            "type": "push_incoming", "push_id": "p", "from_id": "ab", "from_name": "Đạt",
            "job": "12A3", "files": 3, "bytes": 9, "at": 1
        }))
        .unwrap();
        assert_eq!(name, "push:incoming");
        assert!(p.get("type").is_none() && p["job"] == "12A3");
        assert_eq!(
            v05_event(&json!({"type": "lan_blocked", "blocked": true})),
            Some(("net:lan_blocked", json!({"blocked": true})))
        );
        assert_eq!(v05_event(&json!({"type": "progress", "id": "x"})), None);
    }

    #[test]
    fn missing_drive_falls_back_and_is_never_created() {
        let (p, fb) = effective_dest("/Volumes/Ổ không có 123/Toss");
        assert!(fb);
        assert!(p.ends_with("Toss"));
        assert!(!Path::new("/Volumes/Ổ không có 123").exists());
        let tmp = std::env::temp_dir().join(format!("toss-dest-{}", std::process::id()));
        let (p2, fb2) = effective_dest(tmp.to_str().unwrap());
        assert!(!fb2);
        assert_eq!(p2, tmp);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
