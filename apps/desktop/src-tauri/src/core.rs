//! The ONLY file that touches `toss-core`. Everything above it (commands.rs, shell.rs)
//! speaks the IPC types from `ipc.rs` and plain `serde_json::Value`s.
//!
//! Written against contracts/core-api.md. Where the contract has no call for something the
//! UI contract needs, the gap is called out with `GAP:` and listed in contracts/CHANGES.md.

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex, OnceLock},
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_notification::NotificationExt as _;
use tokio::sync::{broadcast, Mutex, RwLock};
use toss_core::{
    wire::Op, CtlClient, HostConfig as CoreHost, Node, NodeConfig, Root as CoreRoot, Ticket,
    TicketSpec as CoreSpec, TransferEvent,
};

use crate::{
    config,
    ipc::{AppState, HostConfig, JobLast, JobRecord, Root, TicketInfo, TicketSpec},
    settings::{data_dir, Settings},
};

/// Prefix of the error text the UI uses to detect "no admin ticket stored".
pub const NO_ADMIN: &str = "no_admin_ticket: Chưa có máy tổng từ xa";

pub struct Core {
    data_dir: PathBuf,
    app: OnceLock<AppHandle>,
    settings: StdMutex<Settings>,
    node: RwLock<Option<Arc<Node>>>,
    start_lock: Mutex<()>,
    ctl: Mutex<Option<Arc<CtlClient>>>,
}

impl Core {
    pub fn new() -> Core {
        let dir = data_dir();
        let settings = Settings::load(&dir);
        Core {
            data_dir: dir,
            app: OnceLock::new(),
            settings: StdMutex::new(settings),
            node: RwLock::new(None),
            start_lock: Mutex::new(()),
            ctl: Mutex::new(None),
        }
    }

    pub fn attach(&self, app: AppHandle) {
        let _ = self.app.set(app);
    }

    fn app(&self) -> Result<&AppHandle> {
        self.app.get().ok_or_else(|| anyhow!("app chưa sẵn sàng"))
    }

    // ---- settings helpers ------------------------------------------------
    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    fn update_settings<R>(&self, f: impl FnOnce(&mut Settings) -> R) -> Result<R> {
        let (r, snapshot) = {
            let mut s = self.settings.lock().unwrap();
            let r = f(&mut s);
            (r, s.clone())
        };
        snapshot.save(&self.data_dir)?;
        Ok(r)
    }

    pub fn first_run(&self) -> bool {
        self.settings.lock().unwrap().first_run
    }

    pub fn host_mode(&self) -> bool {
        self.settings.lock().unwrap().host.is_some()
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
        let (name, host) = {
            let s = self.settings.lock().unwrap();
            (s.device_name.clone(), s.host.clone())
        };
        std::fs::create_dir_all(&self.data_dir).context("tạo thư mục dữ liệu")?;
        let cfg = NodeConfig {
            data_dir: self.data_dir.clone(),
            device_name: name,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
        };
        let node = tokio::time::timeout(Duration::from_secs(20), Node::start(cfg))
            .await
            .map_err(|_| anyhow!("khởi động mạng quá lâu"))?
            .context("không khởi động được mạng P2P")?;
        let node = Arc::new(node);
        if let Some(h) = host {
            match to_core_host(&h, false) {
                Ok(c) => {
                    if let Err(e) = node.set_host(Some(c)).await {
                        tracing::error!("bật chế độ máy tổng thất bại: {e:#}");
                    }
                }
                Err(e) => tracing::error!("cấu hình máy tổng không hợp lệ: {e:#}"),
            }
        }
        self.spawn_forwarder(node.subscribe());
        Ok(node)
    }

    /// Restart the node so a new device name takes effect. Skipped while transfers are active
    /// (the name is then applied at next launch).
    async fn restart_for_rename(&self) -> Result<()> {
        let _g = self.start_lock.lock().await;
        let Some(cur) = self.node.read().await.clone() else { return Ok(()) };
        if cur.transfers().iter().any(|t| is_active(&serde_json::to_value(t).unwrap_or(Value::Null))) {
            tracing::info!("device name saved; node restart deferred (transfers active)");
            return Ok(());
        }
        drop(cur);
        let old = self.node.write().await.take();
        *self.ctl.lock().await = None;
        if let Some(n) = old {
            match Arc::try_unwrap(n) {
                Ok(node) => node.shutdown().await,
                Err(_) => tracing::warn!("node still referenced during restart; dropping"),
            }
        }
        let n = self.start_node().await?;
        *self.node.write().await = Some(n);
        Ok(())
    }

    pub async fn shutdown(&self) {
        let old = self.node.write().await.take();
        if let Some(n) = old {
            if let Ok(node) = Arc::try_unwrap(n) {
                node.shutdown().await;
            }
        }
    }

    // ---- event forwarding -----------------------------------------------------
    fn spawn_forwarder(&self, mut rx: broadcast::Receiver<TransferEvent>) {
        let Some(app) = self.app.get().cloned() else { return };
        tauri::async_runtime::spawn(async move {
            // id -> (job, is_client_side), to word notifications
            let mut jobs: HashMap<String, (String, bool)> = HashMap::new();
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        if let Ok(v) = serde_json::to_value(&ev) {
                            forward(&app, &mut jobs, v);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => tracing::warn!("event stream lagged by {n}"),
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
        let autostart = self.app().ok().and_then(|a| a.autolaunch().is_enabled().ok()).unwrap_or(s.autostart);
        Ok(AppState {
            version: env!("CARGO_PKG_VERSION").to_string(),
            endpoint_id,
            device_name: s.device_name.clone(),
            dest_dir: s.dest_dir.clone(),
            host: s.host.clone(),
            page_host: s.page_host(),
            autostart,
            first_run: s.first_run,
        })
    }

    pub async fn finish_setup(&self, device_name: String, dest_dir: String) -> Result<AppState> {
        let name = clean_name(&device_name)?;
        std::fs::create_dir_all(&dest_dir).with_context(|| format!("không tạo được thư mục {dest_dir}"))?;
        self.update_settings(|s| {
            s.device_name = name;
            s.dest_dir = dest_dir;
            s.first_run = false;
        })?;
        self.restart_for_rename().await?;
        self.state().await
    }

    pub async fn set_device_name(&self, name: String) -> Result<()> {
        let name = clean_name(&name)?;
        self.update_settings(|s| s.device_name = name)?;
        self.restart_for_rename().await
    }

    pub fn set_dest_dir(&self, path: String) -> Result<()> {
        std::fs::create_dir_all(&path).with_context(|| format!("không tạo được thư mục {path}"))?;
        self.update_settings(|s| s.dest_dir = path)
    }

    pub fn set_autostart(&self, on: bool) -> Result<()> {
        let launch = self.app()?.autolaunch();
        if on {
            launch.enable().map_err(|e| anyhow!("không bật được tự khởi động: {e}"))?;
        } else {
            launch.disable().map_err(|e| anyhow!("không tắt được tự khởi động: {e}"))?;
        }
        self.update_settings(|s| s.autostart = on)
    }

    // ---- host mode ----------------------------------------------------------------
    pub async fn set_host(&self, config: Option<HostConfig>) -> Result<()> {
        let node = self.node().await?;
        match &config {
            Some(h) => node.set_host(Some(to_core_host(h, true)?)).await?,
            None => node.set_host(None).await?,
        }
        self.update_settings(|s| s.host = config)
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
        let t = node.create_ticket(to_core_spec(spec))?;
        Ok(self.info(&t))
    }

    pub async fn list_jobs(&self) -> Result<Vec<JobRecord>> {
        let node = self.node().await?;
        Ok(node
            .list_jobs()
            .into_iter()
            .map(|j| {
                // Re-signed ticket string so the UI can offer "copy link" for old jobs too.
                let ticket = node.job_ticket(j.id).map(|t| t.to_string()).unwrap_or_default();
                job_record(j, ticket)
            })
            .collect())
    }

    pub async fn revoke(&self, id: String) -> Result<()> {
        self.node().await?.revoke(parse_id(&id)?)
    }

    pub async fn create_admin_ticket(&self, days: u32) -> Result<TicketInfo> {
        let node = self.node().await?;
        // Not in core-api.md, but provided by toss-core (see CHANGES.md).
        let t = node.create_admin_ticket(days)?;
        Ok(self.info(&t))
    }

    // ---- tickets / transfers (client) -----------------------------------------------
    pub fn parse_ticket(&self, text: &str) -> Result<TicketInfo> {
        let t = Ticket::from_str(text.trim()).map_err(|e| anyhow!("không đọc được link ({e})"))?;
        Ok(self.info(&t))
    }

    pub async fn start_get(&self, ticket: String) -> Result<String> {
        let t = Ticket::from_str(ticket.trim()).map_err(|e| anyhow!("không đọc được link ({e})"))?;
        if t.body.admin {
            bail!("Đây là link admin, không dùng để nhận file");
        }
        if !t.body.get {
            bail!("Link này không cho phép nhận file");
        }
        if t.is_expired() {
            bail!("Link đã hết hạn");
        }
        let dest = PathBuf::from(self.settings().dest_dir);
        std::fs::create_dir_all(&dest).with_context(|| format!("không tạo được thư mục {}", dest.display()))?;
        let node = self.node().await?;
        let tr = node.get(t, dest).await?;
        Ok(tr.id.clone())
    }

    pub async fn start_put(&self, ticket: String, paths: Vec<String>) -> Result<String> {
        let t = Ticket::from_str(ticket.trim()).map_err(|e| anyhow!("không đọc được link ({e})"))?;
        if t.body.admin || !t.body.put {
            bail!("Link này không cho phép trả file");
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

    pub async fn list_transfers(&self) -> Result<Vec<Value>> {
        let node = self.node().await?;
        Ok(node.transfers().iter().filter_map(|t| serde_json::to_value(t).ok()).collect())
    }

    pub async fn pause(&self, id: String) -> Result<()> {
        self.node().await?.pause(&id).await
    }

    pub async fn resume(&self, id: String) -> Result<()> {
        self.node().await?.resume(&id).await
    }

    pub async fn cancel(&self, id: String) -> Result<()> {
        self.node().await?.cancel(&id).await
    }

    /// Not in core-api.md, but provided by toss-core (see CHANGES.md).
    pub async fn clear_finished(&self) -> Result<()> {
        self.node().await?.clear_finished();
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
        self.update_settings(|s| s.admin_ticket = stored)
    }

    pub fn has_admin_ticket(&self) -> bool {
        self.settings.lock().unwrap().admin_ticket.is_some()
    }

    async fn ctl(&self) -> Result<Arc<CtlClient>> {
        let mut g = self.ctl.lock().await;
        if let Some(c) = g.clone() {
            return Ok(c);
        }
        let text = self.settings().admin_ticket.ok_or_else(|| anyhow!(NO_ADMIN))?;
        let t = Ticket::from_str(&text).map_err(|e| anyhow!("link admin hỏng ({e})"))?;
        let node = self.node().await?;
        let c = tokio::time::timeout(Duration::from_secs(20), node.ctl(t))
            .await
            .map_err(|_| anyhow!("Máy tổng không phản hồi (đang offline?)"))?
            .context("không kết nối được máy tổng")?;
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
        Ok(self.info(&t))
    }

    pub async fn remote_list_jobs(&self) -> Result<Vec<JobRecord>> {
        let jobs = self.with_ctl(|c| async move { c.list_jobs().await }).await?;
        // Tickets of remote jobs can't be re-signed here; the UI hides "copy" when empty.
        Ok(jobs.into_iter().map(|j| job_record(j, String::new())).collect())
    }

    pub async fn remote_revoke(&self, id: String) -> Result<()> {
        let id = parse_id(&id)?;
        self.with_ctl(|c| async move { c.revoke(id).await }).await
    }

    // ---- misc -------------------------------------------------------------------------------
    fn info(&self, t: &Ticket) -> TicketInfo {
        ticket_info(t, &self.settings().page_host())
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
    CoreSpec { job: s.job, root: s.root, path: s.path, ret: s.ret, get: s.get, put: s.put, days: s.days }
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
    let n: String = n.trim().chars().filter(|c| !c.is_control()).take(60).collect();
    if n.is_empty() {
        bail!("Tên máy không được để trống");
    }
    Ok(n)
}

fn valid_alias(a: &str) -> bool {
    !a.is_empty() && a.len() <= 32 && a.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `strict` (user pressed save): roots must exist. Not strict (startup): tolerate an offline drive.
fn to_core_host(h: &HostConfig, strict: bool) -> Result<CoreHost> {
    let mut seen = HashSet::new();
    let mut roots = Vec::new();
    for Root { alias, path } in &h.roots {
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
        roots.push(CoreRoot { alias: alias.clone(), path: p });
    }
    if h.return_root.trim().is_empty() {
        bail!("Chưa chọn thư mục trả về");
    }
    let ret = PathBuf::from(&h.return_root);
    if let Err(e) = std::fs::create_dir_all(&ret) {
        if strict {
            bail!("Không tạo được thư mục trả về {}: {e}", h.return_root);
        }
    }
    Ok(CoreHost { roots, return_root: ret, max_parallel_jobs: h.max_parallel_jobs.clamp(1, 16), max_parallel_streams: 4 })
}

fn is_active(v: &Value) -> bool {
    matches!(v["status"].as_str(), Some("connecting" | "running" | "paused"))
}

/// Emit one core event to the UI (+ derived `host:status`, + native notifications).
fn forward(app: &AppHandle, jobs: &mut HashMap<String, (String, bool)>, v: Value) {
    let _ = app.emit("transfer:event", &v);
    let ty = v["type"].as_str().unwrap_or("");
    if ty == "host_status" {
        let mut p = v.clone();
        if let Some(o) = p.as_object_mut() {
            o.remove("type");
        }
        let _ = app.emit("host:status", p);
        return;
    }
    let id = v["id"].as_str().unwrap_or("").to_string();
    match ty {
        "started" => {
            let job = v["job"].as_str().unwrap_or("").to_string();
            jobs.insert(id, (job, v["side"] == "client"));
        }
        "done" => {
            if let Some((job, client)) = jobs.remove(&id) {
                if client {
                    let verb = if v["dest"].is_null() { "Đã trả xong" } else { "Đã nhận xong" };
                    notify(app, &format!("{verb}: {job}"));
                }
            }
        }
        "error" if v["retry_in_s"].is_null() && v["code"] != "cancelled" => {
            if let Some((job, client)) = jobs.remove(&id) {
                if client {
                    notify(app, &format!("Lỗi khi chuyển “{job}”: {}", v["msg"].as_str().unwrap_or("")));
                }
            }
        }
        _ => {}
    }
}

fn notify(app: &AppHandle, body: &str) {
    // Skip when the user is looking at the window anyway.
    if let Some(w) = app.get_webview_window(config::MAIN_WINDOW) {
        if w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) {
            return;
        }
    }
    if let Err(e) = app.notification().builder().title("Toss").body(body).show() {
        tracing::debug!("notification failed: {e}");
    }
}

/// QR code as SVG markup with a white quiet zone; scales to its container (no fixed width/height).
pub fn qr_svg(text: &str) -> Result<String> {
    use qrcode::{render::svg, EcLevel, QrCode};
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M).map_err(|e| anyhow!("không tạo được mã QR: {e}"))?;
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
    Ok(format!("{} width=\"100%\" height=\"100%\"{}", tag.join(" "), &rest[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn host_config_validation() {
        let h = HostConfig {
            roots: vec![Root { alias: "RAW".into(), path: "/definitely/not/here".into() }],
            return_root: std::env::temp_dir().join("toss-ret-test").to_string_lossy().into_owned(),
            max_parallel_jobs: 99,
        };
        assert!(to_core_host(&h, true).is_err()); // root missing
        let c = to_core_host(&h, false).unwrap(); // offline drive tolerated at startup
        assert_eq!(c.max_parallel_jobs, 16);
        let dup = HostConfig { roots: vec![Root { alias: "A".into(), path: ".".into() }, Root { alias: "A".into(), path: ".".into() }], ..h };
        assert!(to_core_host(&dup, false).is_err());
    }

    /// Whole adapter against the real toss-core, offline (no peers needed).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn adapter_end_to_end() {
        let base = std::env::temp_dir().join(format!("toss-desktop-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (data, raw, ret, dest) = (base.join("data"), base.join("raw"), base.join("ret"), base.join("dest"));
        std::fs::create_dir_all(raw.join("2026/12A3")).unwrap();
        std::fs::write(raw.join("2026/12A3/a.cr3"), b"hello").unwrap();
        std::env::set_var(config::DATA_DIR_ENV, &data);

        let core = Core::new();
        let st = core.state().await.unwrap();
        assert!(st.first_run);
        assert!(!st.endpoint_id.is_empty());

        // wizard: rename restarts the node but keeps the identity
        let st2 = core.finish_setup("May test".into(), dest.to_string_lossy().into_owned()).await.unwrap();
        assert!(!st2.first_run);
        assert_eq!(st2.device_name, "May test");
        assert_eq!(st2.endpoint_id, st.endpoint_id);
        assert!(core.node().await.unwrap().device_name() == "May test");

        // host mode
        let cfg = HostConfig {
            roots: vec![Root { alias: "RAW".into(), path: raw.to_string_lossy().into_owned() }],
            return_root: ret.to_string_lossy().into_owned(),
            max_parallel_jobs: 2,
        };
        core.set_host(Some(cfg.clone())).await.unwrap();
        assert_eq!(core.settings().host, Some(cfg));
        assert!(core.host_status().await.unwrap()["roots"][0] == "RAW");
        let dir = core.list_dir("RAW".into(), "2026".into()).await.unwrap();
        assert_eq!(dir[0]["name"], "12A3");

        // tickets: create -> parse round-trips; job list carries the re-signed ticket
        let spec = TicketSpec { job: "12A3 - RAW".into(), root: "RAW".into(), path: "2026/12A3".into(), ret: String::new(), get: true, put: true, days: 7 };
        let info = core.create_ticket(spec).await.unwrap();
        assert!(info.ticket.starts_with("toss1"));
        assert!(info.web_url.starts_with("https://toss-link.vercel.app/#toss1"));
        assert_eq!(info.deep_link, format!("toss://t/{}", info.ticket));
        let parsed = core.parse_ticket(&info.web_url).unwrap();
        assert_eq!((parsed.job.as_str(), parsed.get, parsed.put, parsed.admin), ("12A3 - RAW", true, true, false));
        assert_eq!(parsed.host, st.endpoint_id);
        let jobs = core.list_jobs().await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].ticket, info.ticket);
        core.revoke(jobs[0].id.clone()).await.unwrap();
        assert!(core.list_jobs().await.unwrap()[0].revoked);
        assert!(core.parse_ticket("khong phai link").is_err());

        // admin ticket
        let admin = core.create_admin_ticket(30).await.unwrap();
        assert!(admin.admin && !admin.get && !admin.put);
        assert!(core.set_admin_ticket(Some(info.ticket.clone())).await.is_err()); // not an admin ticket
        assert!(core.remote_status().await.unwrap_err().to_string().contains("no_admin_ticket"));
        core.set_admin_ticket(Some(admin.ticket.clone())).await.unwrap();
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
}
