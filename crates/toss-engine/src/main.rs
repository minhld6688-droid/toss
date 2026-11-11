//! toss-engine: Toss without a window, for a host app (MFZ Pulse) to run as a child process.
//!
//! The host writes one JSON request per line on stdin and reads one JSON message per line on stdout:
//!
//!   -> {"id":1,"cmd":"send","key":"t:ab12…","paths":["/x"]}
//!   <- {"id":1,"ok":true,"result":"<transfer id>"}
//!   <- {"event":"transfer","data":{…}}          pushed at any time
//!
//! When stdin closes the host is gone, and the engine shuts down: it never outlives its app.
//! State lives in the normal Toss data folder (key, contacts, settings), so one machine keeps one
//! identity whichever front end runs. Unknown keys in settings.json are preserved.

#[path = "../../../apps/desktop/src-tauri/src/shortlink.rs"]
mod shortlink;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::{broadcast, mpsc},
};
use toss_core::{
    transfer::TransferEvent, HostConfig, NetConfig, Node, NodeConfig, ReceivePolicy, Root, Ticket, TicketSpec,
};
use toss_ls::{LsBridge, LsConfig, LsEvent};

const PAGE_HOST: &str = "toss-link.vercel.app";
const QS_PREFIX: &str = "qs-";

type Out = mpsc::UnboundedSender<String>;

fn say(out: &Out, v: Value) {
    let _ = out.send(v.to_string());
}

fn event(out: &Out, name: &str, data: Value) {
    say(out, json!({ "event": name, "data": data }));
}

// ---- settings: the desktop app's file, kept whole ------------------------------------------------

struct Settings {
    path: PathBuf,
    v: Mutex<Value>,
}

impl Settings {
    fn load(dir: &Path) -> Settings {
        let path = dir.join("settings.json");
        let v = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        Settings { path, v: Mutex::new(v) }
    }

    fn get(&self) -> Value {
        self.v.lock().unwrap().clone()
    }

    fn update(&self, f: impl FnOnce(&mut Value)) -> Result<()> {
        let mut g = self.v.lock().unwrap();
        f(&mut g);
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&*g)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    fn name(&self) -> String {
        match self.get()["device_name"].as_str() {
            Some(n) if !n.trim().is_empty() => n.to_string(),
            _ => gethostname::gethostname().to_string_lossy().trim_end_matches(".local").to_string(),
        }
    }

    fn dest(&self) -> PathBuf {
        let chosen = self.get()["dest_dir"].as_str().unwrap_or("").to_string();
        let p = PathBuf::from(&chosen);
        // A drive that isn't plugged in must not turn into an empty folder under /Volumes
        if !chosen.is_empty() && p.is_dir() {
            return p;
        }
        std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("Downloads")
    }

    fn policy(&self) -> ReceivePolicy {
        serde_json::from_value(self.get()["receive_policy"].clone()).unwrap_or_default()
    }

    fn trusted(&self, fp: &str) -> bool {
        self.get()["ls_trusted"]
            .as_array()
            .is_some_and(|a| a.iter().any(|t| t["fingerprint"].as_str() == Some(fp)))
    }

    fn net(&self) -> NetConfig {
        serde_json::from_value(self.get()["net"].clone()).unwrap_or_default()
    }

    fn page(&self) -> String {
        self.get()["page_host"].as_str().unwrap_or(PAGE_HOST).to_string()
    }

    /// The folders links can serve, with what each link needs from the host
    fn host(&self) -> Option<HostConfig> {
        let s = self.get();
        let mut roots: Vec<Root> = s["host"]["roots"]
            .as_array()
            .map(|a| a.iter().filter_map(|r| serde_json::from_value(r.clone()).ok()).collect())
            .unwrap_or_default();
        for q in s["quick_shares"].as_array().into_iter().flatten() {
            let (Some(alias), Some(path)) = (q["alias"].as_str(), q["path"].as_str()) else { continue };
            if Path::new(path).is_dir() && !roots.iter().any(|r| r.alias == alias) {
                roots.push(Root { alias: alias.to_string(), path: PathBuf::from(path) });
            }
        }
        if s["host"].is_null() && roots.is_empty() {
            return None;
        }
        let back = s["host"]["return_root"]
            .as_str()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.dest().join("Toss nhận lại"));
        let _ = std::fs::create_dir_all(&back);
        Some(HostConfig::new(roots, back))
    }
}

// ---- engine -----------------------------------------------------------------------------------

struct Engine {
    node: Arc<Node>,
    ls: Option<LsBridge>,
    data: PathBuf,
    settings: Settings,
    out: Out,
    short: shortlink::ShortLinks,
    /// LocalSend asks waiting for an answer
    asks: Mutex<HashMap<String, Value>>,
    /// LocalSend transfers, in the same shape as Toss transfers
    ls_transfers: Mutex<HashMap<String, Value>>,
    /// Who an outgoing LocalSend transfer went to, and what, for "send again"
    ls_sends: Mutex<HashMap<String, (String, Vec<PathBuf>)>>,
    hosting: tokio::sync::Mutex<()>,
}

impl Engine {
    fn now() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
    }

    // ---- people: one list, however each one is reachable

    fn peers(&self) -> Result<Value> {
        let near = self.node.nearby();
        let contacts = self.node.contacts();
        let mut out = Vec::new();
        let mut near_ips = Vec::new();
        for c in &contacts {
            let n = near.iter().find(|p| p.id == c.id);
            out.push(json!({
                "key": format!("t:{}", c.id), "name": c.name, "kind": "contact", "platform": c.platform,
                "contact": true, "online": c.online || n.is_some(), "nearby": n.is_some(),
                "trusted": c.auto_accept, "rtt_ms": c.rtt_ms, "last_seen": c.last_seen,
            }));
        }
        for p in &near {
            near_ips.extend(p.addrs.iter().filter_map(|a| a.rsplit_once(':').map(|x| x.0.to_string())));
            if contacts.iter().any(|c| c.id == p.id) {
                continue;
            }
            out.push(json!({
                "key": format!("t:{}", p.id), "name": p.name, "kind": "toss", "platform": p.platform,
                "contact": false, "online": true, "nearby": true, "trusted": false, "addrs": p.addrs,
            }));
        }
        if let Some(ls) = &self.ls {
            let mut seen = Vec::new();
            for d in ls.devices() {
                // The same machine, heard over both protocols
                if (d.is_toss && near_ips.contains(&d.ip)) || seen.contains(&d.fingerprint) {
                    continue;
                }
                seen.push(d.fingerprint.clone());
                out.push(json!({
                    "key": format!("l:{}", d.fingerprint), "name": d.alias,
                    "kind": "localsend", "platform": d.kind, "model": d.model,
                    "contact": false, "online": true, "nearby": true, "trusted": self.settings.trusted(&d.fingerprint),
                }));
            }
        }
        Ok(Value::Array(out))
    }

    async fn send(&self, key: &str, paths: Vec<PathBuf>) -> Result<String> {
        if paths.is_empty() {
            bail!("Không có file nào để gửi");
        }
        for p in &paths {
            if !p.exists() {
                bail!("Không thấy {}", p.display());
            }
        }
        if let Some(id) = key.strip_prefix("t:") {
            return Ok(self.node.send_to_peer(id, paths).await?.id);
        }
        let fp = key.strip_prefix("l:").ok_or_else(|| anyhow!("Không nhận ra người nhận"))?;
        let ls = self.ls.as_ref().ok_or_else(|| anyhow!("LocalSend đang tắt"))?;
        let id = ls.send(fp, paths.clone()).await?;
        self.ls_sends.lock().unwrap().insert(id.clone(), (fp.to_string(), paths));
        Ok(id)
    }

    // ---- receiving

    fn apply_dest(&self, dir: &Path) {
        self.node.set_inbox_dir(dir.to_path_buf());
        if let Some(ls) = &self.ls {
            ls.set_dest(dir.to_path_buf());
        }
    }

    fn answer(self: &Arc<Self>, id: &str, accept: bool, always: bool, folder: Option<PathBuf>) -> Result<()> {
        // Both streams read the folder when the first file starts: point them at the choice, and
        // back at the usual folder once that moment has passed
        if let Some(f) = folder.filter(|_| accept) {
            std::fs::create_dir_all(&f).with_context(|| format!("không ghi được vào {}", f.display()))?;
            self.apply_dest(&f);
            let me = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(20)).await;
                me.apply_dest(&me.settings.dest());
            });
        }
        let ask = self.asks.lock().unwrap().remove(id);
        if let Some(ask) = ask {
            if let Some(ls) = &self.ls {
                ls.answer(id, accept);
            }
            if accept && always {
                let fp = ask["from_key"].as_str().unwrap_or("").trim_start_matches("l:").to_string();
                let alias = ask["from_name"].clone();
                self.settings.update(|s| {
                    if !s["ls_trusted"].is_array() {
                        s["ls_trusted"] = json!([]);
                    }
                    s["ls_trusted"].as_array_mut().unwrap().push(json!({ "fingerprint": fp, "alias": alias }));
                })?;
            }
        } else {
            self.node.answer_push(id, accept, always)?;
        }
        event(&self.out, "incoming_closed", json!({ "id": id }));
        Ok(())
    }

    fn incoming(&self) -> Value {
        let mut v: Vec<Value> = self.node.incoming_pushes().iter().map(Self::push_ask).collect();
        v.extend(self.asks.lock().unwrap().values().cloned());
        Value::Array(v)
    }

    fn push_ask(p: &toss_core::IncomingPush) -> Value {
        json!({
            "id": p.push_id, "source": "toss", "from_key": format!("t:{}", p.from_id), "from_name": p.from_name,
            "stranger": p.stranger, "job": p.job, "files": p.files, "bytes": p.bytes, "at": p.at,
        })
    }

    // ---- transfers: Toss and LocalSend reach the host in one shape

    fn snapshot(t: &toss_core::TransferState) -> Value {
        let mut v = serde_json::to_value(t).unwrap_or_default();
        // A push is a put on both ends: only the side that started it is sending
        v["outgoing"] = json!(v["side"] == "client" && v["op"] != "get");
        v
    }

    fn transfers(&self) -> Value {
        let mut v: Vec<Value> = self.node.transfers().iter().map(Self::snapshot).collect();
        v.extend(self.ls_transfers.lock().unwrap().values().cloned());
        Value::Array(v)
    }

    fn on_core(&self, ev: TransferEvent) {
        let v = serde_json::to_value(&ev).unwrap_or_default();
        match v["type"].as_str().unwrap_or("") {
            "contacts_changed" | "contact_presence" | "nearby_changed" => event(&self.out, "peers", json!({})),
            "push_incoming" => {
                if let TransferEvent::PushIncoming(p) = &ev {
                    event(&self.out, "incoming", Self::push_ask(p));
                }
            }
            "text_incoming" => event(&self.out, "text", v),
            "lan_blocked" => event(&self.out, "lan_blocked", json!({ "blocked": v["blocked"] })),
            _ => {
                if let Some(id) = v["id"].as_str() {
                    if let Some(t) = self.node.transfer(&id.to_string()) {
                        event(&self.out, "transfer", Self::snapshot(&t));
                    }
                }
            }
        }
    }

    fn ls_update(&self, id: &str, f: impl FnOnce(&mut Value)) {
        let mut g = self.ls_transfers.lock().unwrap();
        let t = g.entry(id.to_string()).or_insert_with(|| {
            json!({ "id": id, "status": "connecting", "path": "direct", "bytes_done": 0, "bytes_total": 0,
                    "files_done": 0, "files_total": 0, "speed_bps": 0, "started_at": Self::now() })
        });
        f(t);
        event(&self.out, "transfer", t.clone());
    }

    fn on_ls(&self, ev: LsEvent) {
        match ev {
            LsEvent::DevicesChanged => event(&self.out, "peers", json!({})),
            LsEvent::Incoming { req_id, fingerprint, alias, files, bytes, names, .. } => {
                let trusted = self.settings.trusted(&fingerprint);
                let auto = matches!(self.settings.policy(), ReceivePolicy::Everyone)
                    || (matches!(self.settings.policy(), ReceivePolicy::Contacts) && trusted);
                if auto {
                    if let Some(ls) = &self.ls {
                        ls.answer(&req_id, true);
                    }
                    return;
                }
                let job = names.first().cloned().unwrap_or_else(|| "File".to_string());
                let ask = json!({
                    "id": req_id, "source": "localsend", "from_key": format!("l:{fingerprint}"), "from_name": alias,
                    "stranger": !trusted, "job": job, "files": files, "bytes": bytes, "at": Self::now(),
                });
                self.asks.lock().unwrap().insert(req_id, ask.clone());
                event(&self.out, "incoming", ask);
            }
            LsEvent::Started { id, outgoing, peer, job, files, bytes } => self.ls_update(&id, |t| {
                t["status"] = json!("running");
                t["outgoing"] = json!(outgoing);
                t["peer"] = json!(peer);
                t["job"] = json!(job);
                t["files_total"] = json!(files);
                t["bytes_total"] = json!(bytes);
            }),
            LsEvent::Progress { id, bytes_done, bytes_total, files_done, files_total, speed_bps } => {
                self.ls_update(&id, |t| {
                    t["status"] = json!("running");
                    t["bytes_done"] = json!(bytes_done);
                    t["bytes_total"] = json!(bytes_total);
                    t["files_done"] = json!(files_done);
                    t["files_total"] = json!(files_total);
                    t["speed_bps"] = json!(speed_bps);
                });
            }
            LsEvent::Done { id, dest, files, bytes } => self.ls_update(&id, |t| {
                t["status"] = json!("done");
                t["files_done"] = json!(files);
                t["bytes_done"] = json!(bytes);
                t["speed_bps"] = json!(0);
                t["dest"] = json!(dest.map(|d| d.to_string_lossy().into_owned()));
            }),
            LsEvent::Error { id, msg } => {
                if self.asks.lock().unwrap().remove(&id).is_some() {
                    event(&self.out, "incoming_closed", json!({ "id": id }));
                } else if self.ls_transfers.lock().unwrap().contains_key(&id) {
                    self.ls_update(&id, |t| {
                        t["status"] = json!("error");
                        t["error"] = json!(msg);
                        t["speed_bps"] = json!(0);
                    });
                }
            }
            LsEvent::Text { fingerprint, alias, text } => {
                event(&self.out, "text", json!({ "from_id": fingerprint, "from_name": alias, "text": text }));
            }
            LsEvent::Port { port, fallback } => event(&self.out, "ls_port", json!({ "port": port, "fallback": fallback })),
        }
    }

    // ---- links

    async fn link(&self, path: &str, days: u32, put: bool) -> Result<Value> {
        let _g = self.hosting.lock().await;
        let t = toss_core::paths::share_target(Path::new(path.trim()))?;
        let canon = t.root.to_string_lossy().into_owned();
        let alias = format!("{QS_PREFIX}{}", &blake3::hash(canon.as_bytes()).to_hex()[..8]);
        let job = toss_core::paths::sanitize_component(&t.job, "Toss");
        self.settings.update(|s| {
            if !s["quick_shares"].is_array() {
                s["quick_shares"] = json!([]);
            }
            let list = s["quick_shares"].as_array_mut().unwrap();
            if !list.iter().any(|q| q["alias"].as_str() == Some(alias.as_str())) {
                list.push(json!({ "alias": alias, "path": canon, "job_id": null }));
            }
        })?;
        self.node.set_host(self.settings.host()).await?;
        let ticket = self.node.create_ticket(TicketSpec {
            job, root: alias, path: t.path, ret: String::new(), get: true, put, days,
        })?;
        let text = ticket.to_string();
        let page = self.settings.page();
        let url = self.short.shorten(&page, &text).await.unwrap_or_else(|| ticket.web_url(&page));
        Ok(json!({ "url": url, "id": ticket.id_hex(), "expires_days": days }))
    }

    /// Anything pasted or clicked: a file link, or an invitation from another machine
    async fn open(&self, text: &str, dest: Option<PathBuf>) -> Result<Value> {
        let text = shortlink::expand(text.trim()).await?;
        let word = text.split(|c: char| c.is_whitespace() || c == '#' || c == '/').find(|w| w.starts_with("toss1") || w.starts_with("tossinv")).unwrap_or(&text);
        if word.starts_with(toss_core::contacts::INVITE_PREFIX) {
            let c = self.node.accept_invite(word).await?;
            return Ok(json!({ "kind": "contact", "name": c.name }));
        }
        let t = Ticket::from_str(word).map_err(|e| anyhow!("Không đọc được link ({e})"))?;
        if t.body.admin || !t.body.get {
            bail!("Link này không cho nhận file");
        }
        if t.is_expired() {
            bail!("Link đã hết hạn");
        }
        let dest = dest.unwrap_or_else(|| self.settings.dest());
        std::fs::create_dir_all(&dest)?;
        Ok(json!({ "kind": "transfer", "id": self.node.get(t, dest).await?.id }))
    }

    // ---- requests

    async fn handle(self: &Arc<Self>, r: &Value) -> Result<Value> {
        let s = |k: &str| r[k].as_str().unwrap_or("").to_string();
        let tid = s("tid");
        Ok(match r["cmd"].as_str().unwrap_or("") {
            "hello" => json!({
                "name": self.node.device_name(), "id": self.node.endpoint_id().to_string(), "version": toss_core::VERSION,
                "folder": self.settings.dest(), "policy": self.settings.policy(),
                "ls": self.ls.as_ref().map(|l| l.port()),
            }),
            "peers" => self.peers()?,
            "rescan" => {
                if let Some(ls) = &self.ls {
                    ls.rescan().await;
                }
                let n = self.node.clone();
                tokio::spawn(async move { n.refresh_presence().await });
                json!(true)
            }
            "send" => {
                let paths = r["paths"].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(PathBuf::from)).collect();
                json!(self.send(&s("key"), paths).await?)
            }
            "text" => {
                let (key, text) = (s("key"), s("text"));
                match key.split_once(':') {
                    Some(("t", id)) => self.node.send_text(id, &text).await?,
                    Some(("l", fp)) => self.ls.as_ref().ok_or_else(|| anyhow!("LocalSend đang tắt"))?.send_text(fp, &text).await?,
                    _ => bail!("Không nhận ra người nhận"),
                }
                json!(true)
            }
            "transfers" => self.transfers(),
            "cancel" => {
                if self.ls_transfers.lock().unwrap().contains_key(&tid) {
                    if let Some(ls) = &self.ls {
                        ls.cancel(&tid);
                    }
                } else {
                    self.node.cancel(&tid).await?;
                }
                json!(true)
            }
            "pause" => {
                self.node.pause(&tid).await?;
                json!(true)
            }
            "resume" => {
                self.node.resume(&tid).await?;
                json!(true)
            }
            "resend" => {
                let again = self.ls_sends.lock().unwrap().get(&tid).cloned();
                match again {
                    Some((fp, paths)) => json!(self.send(&format!("l:{fp}"), paths).await?),
                    None => json!(self.node.resend(&tid).await?.id),
                }
            }
            "clear" => {
                self.node.clear_finished();
                json!(true)
            }
            "incoming" => self.incoming(),
            "answer" => {
                let folder = r["folder"].as_str().map(PathBuf::from);
                self.answer(&s("ask"), r["accept"].as_bool() == Some(true), r["always"].as_bool() == Some(true), folder)?;
                json!(true)
            }
            "set_folder" => {
                let dir = s("folder");
                self.settings.update(|v| v["dest_dir"] = json!(dir))?;
                self.apply_dest(&self.settings.dest());
                json!(true)
            }
            "set_policy" => {
                let p: ReceivePolicy = serde_json::from_value(r["policy"].clone())?;
                self.settings.update(|v| v["receive_policy"] = r["policy"].clone())?;
                self.node.set_receive_policy(p);
                json!(true)
            }
            "set_relay" => {
                let urls = r["urls"].clone();
                self.settings.update(|v| {
                    if !v["net"].is_object() {
                        v["net"] = serde_json::to_value(NetConfig::default()).unwrap_or_default();
                    }
                    v["net"]["relay_urls"] = urls;
                })?;
                json!("restart")
            }
            "invite" => {
                let i = self.node.create_invite(r["days"].as_u64().unwrap_or(7) as u32, &self.settings.page())?;
                let url = self.short.shorten(&self.settings.page(), &i.invite).await.unwrap_or(i.web_url);
                json!({ "url": url, "name": i.name })
            }
            "open" => self.open(&s("text"), r["dest"].as_str().map(PathBuf::from)).await?,
            "rename" => {
                self.node.rename_contact(s("peer").trim_start_matches("t:"), &s("name"))?;
                json!(true)
            }
            "auto_accept" => {
                self.node.set_contact_auto_accept(s("peer").trim_start_matches("t:"), r["on"].as_bool() == Some(true))?;
                json!(true)
            }
            "forget" => {
                self.node.remove_contact(s("peer").trim_start_matches("t:"))?;
                json!(true)
            }
            "link" => self.link(&s("path"), r["days"].as_u64().unwrap_or(7) as u32, r["put"].as_bool() == Some(true)).await?,
            "links" => serde_json::to_value(self.node.list_jobs())?,
            "revoke" => {
                let id = s("job");
                let mut b = [0u8; 16];
                if id.len() != 32 {
                    bail!("Mã link không đúng");
                }
                for (i, x) in b.iter_mut().enumerate() {
                    *x = u8::from_str_radix(&id[i * 2..i * 2 + 2], 16)?;
                }
                self.node.revoke(b)?;
                json!(true)
            }
            "diag" => serde_json::to_value(self.node.diagnostics().await)?,
            other => bail!("Lệnh không có: {other}"),
        })
    }
}

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned()
}

async fn start() -> Result<(Arc<Engine>, mpsc::UnboundedReceiver<String>, mpsc::Receiver<LsEvent>)> {
    let data = arg("--data-dir").map(PathBuf::from).unwrap_or_else(toss_core::node::default_data_dir);
    std::fs::create_dir_all(&data).context("tạo thư mục dữ liệu")?;
    let settings = Settings::load(&data);
    let host = settings.host();
    let cfg = NodeConfig {
        data_dir: data.clone(),
        device_name: settings.name(),
        app_version: format!("{} mfz", toss_core::VERSION),
        net: settings.net(),
    };
    // A fixed UDP port makes hole punching easier for machines that serve links
    let port = host.as_ref().map(HostConfig::effective_bind_port);
    let node = match Node::start_with_port(cfg.clone(), port).await {
        Ok(n) => n,
        Err(e) if port.is_some() => {
            eprintln!("port {port:?} unavailable: {e:#}");
            Node::start_with_port(cfg, None).await?
        }
        Err(e) => return Err(e),
    };
    let node = Arc::new(node);
    node.set_inbox_dir(settings.dest());
    node.set_receive_policy(settings.policy());
    if host.is_some() {
        let _ = node.set_host(host).await;
    }
    // A host app with its own LocalSend stack passes --no-localsend: only one can hold the port
    let want_ls = settings.get()["ls_enabled"].as_bool().unwrap_or(true)
        && !std::env::args().any(|a| a == "--no-localsend");
    let mut ls = None;
    let (_keep, mut ls_rx) = mpsc::channel(1);
    if want_ls {
        match LsBridge::start(LsConfig {
            alias: settings.name(),
            data_dir: data.clone(),
            dest_dir: settings.dest(),
            port: toss_ls::DEFAULT_PORT,
        })
        .await
        {
            Ok((b, rx)) => {
                ls = Some(b);
                ls_rx = rx;
            }
            Err(e) => eprintln!("LocalSend bridge off: {e:#}"),
        }
    }
    let (tx, rx) = mpsc::unbounded_channel();
    let short = shortlink::ShortLinks::load(&data);
    let e = Arc::new(Engine {
        node, ls, data, settings, out: tx, short,
        asks: Mutex::default(), ls_transfers: Mutex::default(), ls_sends: Mutex::default(),
        hosting: tokio::sync::Mutex::new(()),
    });
    Ok((e, rx, ls_rx))
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let _ = tracing_subscriber::fmt().with_writer(std::io::stderr).with_env_filter(std::env::var("TOSS_LOG").unwrap_or_else(|_| "warn".into())).try_init();
    let (engine, mut lines, mut ls_rx) = match start().await {
        Ok(x) => x,
        Err(e) => {
            println!("{}", json!({ "event": "failed", "data": format!("{e:#}") }));
            return Err(e);
        }
    };
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let mut so = tokio::io::stdout();
        while let Some(l) = lines.recv().await {
            if so.write_all(format!("{l}\n").as_bytes()).await.is_err() || so.flush().await.is_err() {
                break;
            }
        }
    });
    {
        let e = engine.clone();
        let mut rx = e.node.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => e.on_core(ev),
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        event(&e.out, "resync", json!({}));
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
    {
        let e = engine.clone();
        tokio::spawn(async move {
            while let Some(ev) = ls_rx.recv().await {
                e.on_ls(ev);
            }
        });
    }
    event(&engine.out, "ready", json!({ "data_dir": engine.data }));
    let mut input = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = input.next_line().await {
        let Ok(r) = serde_json::from_str::<Value>(&line) else { continue };
        let e = engine.clone();
        // A request can wait on the network: it must never hold up the next one
        tokio::spawn(async move {
            let id = r["id"].clone();
            match e.handle(&r).await {
                Ok(v) => say(&e.out, json!({ "id": id, "ok": true, "result": v })),
                Err(err) => say(&e.out, json!({ "id": id, "ok": false, "error": format!("{err:#}") })),
            }
        });
    }
    // stdin closed: the app is gone
    if let Some(ls) = &engine.ls {
        ls.stop().await;
    }
    Ok(())
}
