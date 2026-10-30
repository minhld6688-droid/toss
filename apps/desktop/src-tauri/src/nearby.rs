//! v0.8 "Gần là thấy, thả là xong" (contracts/nearby-v0.8.md §3–§4): the unified nearby list
//! (Toss over mDNS ∪ LocalSend), unified incoming asks, LocalSend policy and transfers, received
//! history, text messages, web share.
//!
//! Event handling is split into pure "what should happen" functions returning [`Effect`]s
//! (unit-tested without a Tauri app) and [`apply`], which performs them.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;
use toss_core::util::now_unix;

use crate::{
    core::{notify_open, qr_svg, via_serde, Core},
    ipc::{Contact, CoreNearby, Incoming, IncomingPush, Nearby, Received, TextMsg, WebShare},
    ls::{Ls, LsDev, LsEv, LS_MULTICAST},
    settings::{LsTrusted, ReceivePolicy, Settings},
};

/// Received history kept in `<data_dir>/received.json`.
pub const RECEIVED_FILE: &str = "received.json";
const RECEIVED_MAX: usize = 30;
/// A LocalSend ask nobody answered is declined by the bridge after this.
const LS_ASK_SECS: u64 = 60;
/// The core declines an unanswered Toss push after 10 min.
pub const TOSS_ASK_SECS: u64 = 605;
/// Largest text message (core and LocalSend limit).
pub const MAX_TEXT: usize = 60 * 1024;

/// Something to do in the app after an event.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Emit a UI event.
    Emit(&'static str, Value),
    /// OS notification; clicking it opens `open` (a folder) when set.
    Notify { body: String, open: Option<String> },
    /// Show and focus the main window.
    Show,
    /// Answer a LocalSend prepare-upload (policy auto-accept).
    Answer { req_id: String, accept: bool },
    /// Record in the history, emit `received`, notify "Đã nhận …" (click opens the folder).
    Received(Received),
    /// Emit `incoming:closed` after `secs` if the ask is still unanswered.
    CloseLater { id: String, secs: u64, toss: bool },
}

// ---- pure helpers -----------------------------------------------------------------------------

/// Where a `send_to` / `send_text` key points.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    /// Endpoint id hex (contact or nearby Toss).
    Toss(String),
    /// LocalSend fingerprint.
    Ls(String),
}

/// `t:<hex>` | `l:<fingerprint>` | bare contact id.
pub fn parse_key(key: &str) -> Result<Target> {
    let k = key.trim();
    let t = if let Some(id) = k.strip_prefix("t:") {
        Target::Toss(id.trim().to_string())
    } else if let Some(fp) = k.strip_prefix("l:") {
        Target::Ls(fp.trim().to_string())
    } else {
        Target::Toss(k.to_string())
    };
    match &t {
        Target::Toss(s) | Target::Ls(s) if s.is_empty() => bail!("Chưa chọn máy nhận"),
        _ => Ok(t),
    }
}

/// `"192.168.1.5:51234"` / `"[fe80::1]:5"` → the IP part.
pub fn addr_ip(addr: &str) -> Option<String> {
    if let Ok(sa) = addr.parse::<std::net::SocketAddr>() {
        return Some(sa.ip().to_string());
    }
    if let Ok(ip) = addr.parse::<std::net::IpAddr>() {
        return Some(ip.to_string());
    }
    let (host, _) = addr.rsplit_once(':')?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.parse::<std::net::IpAddr>().ok().map(|ip| ip.to_string())
}

/// ipc.ts `Nearby.platform` for a LocalSend device.
pub fn ls_platform(kind: &str, model: Option<&str>) -> String {
    let m = model.unwrap_or("").to_lowercase();
    let p = if m.contains("iphone") || m.contains("ipad") || m.contains("ios") {
        "ios"
    } else if m.contains("windows") {
        "windows"
    } else if m.contains("mac") {
        "macos"
    } else if m.contains("linux") {
        "linux"
    } else if kind == "web" {
        "web"
    } else if kind == "mobile" || m.contains("android") {
        "android"
    } else {
        ""
    };
    p.to_string()
}

/// §3.3: core nearby (kind "toss") ∪ LocalSend devices. A LocalSend device that is a Toss whose IP
/// matches a core peer's address is the same machine and hidden; other Toss devices seen only over
/// LocalSend are "toss_lan".
pub fn merge_nearby(
    core: &[CoreNearby],
    contacts: &[Contact],
    ls: &[LsDev],
    trusted: &[LsTrusted],
) -> Vec<Nearby> {
    let mut toss = Vec::new();
    let mut ips = HashSet::new();
    for p in core {
        ips.extend(p.addrs.iter().filter_map(|a| addr_ip(a)));
        let c = contacts.iter().find(|c| c.id == p.id);
        toss.push(Nearby {
            key: format!("t:{}", p.id),
            kind: "toss",
            name: c.map_or_else(|| p.name.clone(), |c| c.name.clone()),
            model: Some("Toss".into()),
            platform: p.platform.clone(),
            contact_id: (p.contact || c.is_some()).then(|| p.id.clone()),
            trusted: c.is_some_and(|c| c.auto_accept),
        });
    }
    let mut others = Vec::new();
    let mut seen_fp = HashSet::new();
    for d in ls {
        if (d.is_toss && ips.contains(&d.ip)) || !seen_fp.insert(d.fingerprint.clone()) {
            continue;
        }
        others.push(Nearby {
            key: format!("l:{}", d.fingerprint),
            kind: if d.is_toss { "toss_lan" } else { "localsend" },
            name: d.alias.clone(),
            model: d.model.clone(),
            platform: ls_platform(&d.kind, d.model.as_deref()),
            contact_id: None,
            trusted: trusted.iter().any(|t| t.fingerprint == d.fingerprint),
        });
    }
    // Contacts / trusted devices first, then by name.
    let order = |a: &Nearby, b: &Nearby| {
        (a.contact_id.is_none(), !a.trusted, a.name.to_lowercase()).cmp(&(
            b.contact_id.is_none(),
            !b.trusted,
            b.name.to_lowercase(),
        ))
    };
    toss.sort_by(order);
    others.sort_by(order);
    toss.extend(others);
    toss
}

/// §3.2: LocalSend incoming accepted without asking?
pub fn ls_auto_accept(policy: ReceivePolicy, trusted: bool) -> bool {
    match policy {
        ReceivePolicy::Everyone => true,
        ReceivePolicy::Contacts => trusted,
        ReceivePolicy::Off => false,
    }
}

/// Job name shown for a LocalSend transfer: the common top folder, the single file, or "N file".
pub fn ls_job(names: &[String], files: u32) -> String {
    let top = |n: &String| {
        n.replace('\\', "/")
            .split('/')
            .find(|c| !c.is_empty())
            .unwrap_or("")
            .to_string()
    };
    let nested = names.iter().any(|n| n.replace('\\', "/").trim_matches('/').contains('/'));
    if let Some(first) = names.first() {
        let t = top(first);
        if nested && !t.is_empty() && names.iter().all(|n| top(n) == t) {
            return t;
        }
        if files <= 1 && !t.is_empty() {
            return first.replace('\\', "/").rsplit('/').next().unwrap_or(&t).to_string();
        }
    }
    format!("{} file", files.max(names.len() as u32))
}

/// Job name for items this machine sends (folder / file name, or "N file").
pub fn job_of(paths: &[PathBuf]) -> String {
    match paths {
        [one] => one
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| one.to_string_lossy().into_owned()),
        _ => format!("{} file", paths.len()),
    }
}

/// Toss push → unified ask.
pub fn toss_incoming(p: &IncomingPush) -> Incoming {
    Incoming {
        id: p.push_id.clone(),
        source: "toss".into(),
        from_key: if p.stranger {
            format!("t:{}", p.from_id)
        } else {
            p.from_id.clone()
        },
        from_name: p.from_name.clone(),
        stranger: p.stranger,
        job: p.job.clone(),
        files: p.files,
        bytes: p.bytes,
        names: Vec::new(),
        at: p.at,
    }
}

pub fn ask_text(name: &str, job: &str) -> String {
    format!("{name} muốn gửi {job}")
}

pub fn received_text(r: &Received) -> String {
    format!("Đã nhận {} từ {}", r.job, r.from_name)
}

/// "Tên: nội dung…" (first 120 characters).
pub fn text_note(name: &str, text: &str) -> String {
    let mut t: String = text.trim().chars().take(120).collect();
    if text.trim().chars().count() > 120 {
        t.push('…');
    }
    format!("{name}: {t}")
}

pub fn check_text(text: &str) -> Result<()> {
    if text.trim().is_empty() {
        bail!("Chưa có nội dung để gửi");
    }
    if text.len() > MAX_TEXT {
        bail!("Tin nhắn quá dài (tối đa 60 KB)");
    }
    Ok(())
}

pub fn check_paths(paths: Vec<String>) -> Result<Vec<PathBuf>> {
    if paths.is_empty() {
        bail!("Chưa chọn file hoặc folder nào");
    }
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    if let Some(p) = paths.iter().find(|p| !p.exists()) {
        bail!("Không tìm thấy {}", p.display());
    }
    Ok(paths)
}

// ---- received history -----------------------------------------------------------------------------

/// `<data_dir>/received.json`, newest first, at most 30.
pub struct ReceivedLog {
    path: PathBuf,
    list: StdMutex<Vec<Received>>,
}

impl ReceivedLog {
    pub fn load(dir: &Path) -> ReceivedLog {
        let path = dir.join(RECEIVED_FILE);
        let list = fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        ReceivedLog {
            path,
            list: StdMutex::new(list),
        }
    }

    pub fn list(&self) -> Vec<Received> {
        self.list.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn add(&self, r: Received) {
        let mut l = self.list.lock().unwrap_or_else(|p| p.into_inner());
        l.retain(|x| x.id != r.id);
        l.insert(0, r);
        l.truncate(RECEIVED_MAX);
        let save = || -> Result<()> {
            if let Some(d) = self.path.parent() {
                fs::create_dir_all(d)?;
            }
            let tmp = self.path.with_extension("json.tmp");
            fs::write(&tmp, serde_json::to_vec_pretty(&*l)?)?;
            fs::rename(&tmp, &self.path)?;
            Ok(())
        };
        if let Err(e) = save() {
            tracing::warn!("received.json: {e:#}");
        }
    }
}

// ---- LocalSend transfers ----------------------------------------------------------------------------

/// A LocalSend transfer in the ipc.ts `TransferState` shape (`contact: null`, `path: "direct"`).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct LsTransfer {
    pub id: String,
    pub job: String,
    pub op: &'static str,
    pub side: &'static str,
    pub peer: String,
    pub status: &'static str,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub files_done: u32,
    pub files_total: u32,
    pub speed_bps: u64,
    pub eta_s: Option<u64>,
    pub path: &'static str,
    pub rtt_ms: Option<u32>,
    pub streams: u32,
    pub dest: Option<String>,
    pub error: Option<String>,
    pub started_at: u64,
    pub contact: Option<String>,
}

impl LsTransfer {
    fn new(id: &str, job: &str, peer: &str, outgoing: bool) -> LsTransfer {
        LsTransfer {
            id: id.to_string(),
            job: job.to_string(),
            // A receive shows like a core push received: side "host", op "put".
            op: "put",
            side: if outgoing { "client" } else { "host" },
            peer: peer.to_string(),
            status: "connecting",
            bytes_done: 0,
            bytes_total: 0,
            files_done: 0,
            files_total: 0,
            speed_bps: 0,
            eta_s: None,
            path: "direct",
            rtt_ms: None,
            streams: 1,
            dest: None,
            error: None,
            started_at: now_unix(),
            contact: None,
        }
    }

    fn outgoing(&self) -> bool {
        self.side == "client"
    }

    fn is_final(&self) -> bool {
        matches!(self.status, "done" | "error" | "cancelled")
    }
}

/// Shell-side v0.8 state (owned by [`Core`]).
pub struct V08 {
    ls: StdMutex<Option<Ls>>,
    /// Serializes LocalSend start / stop.
    ls_gate: tokio::sync::Mutex<()>,
    ls_port: StdMutex<Option<u16>>,
    /// LocalSend asks waiting for the user.
    pending: StdMutex<Vec<Incoming>>,
    transfers: StdMutex<Vec<LsTransfer>>,
    /// Outgoing LocalSend sends: id → (fingerprint, items), for `resend`.
    sends: StdMutex<HashMap<String, (String, Vec<PathBuf>)>>,
    pub received: ReceivedLog,
}

fn lock<T>(m: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl V08 {
    pub fn new(data_dir: &Path) -> V08 {
        V08 {
            ls: StdMutex::new(None),
            ls_gate: tokio::sync::Mutex::new(()),
            ls_port: StdMutex::new(None),
            pending: StdMutex::new(Vec::new()),
            transfers: StdMutex::new(Vec::new()),
            sends: StdMutex::new(HashMap::new()),
            received: ReceivedLog::load(data_dir),
        }
    }

    pub fn ls(&self) -> Option<Ls> {
        lock(&self.ls).clone()
    }

    pub fn ls_port(&self) -> Option<u16> {
        *lock(&self.ls_port)
    }

    pub fn pending(&self) -> Vec<Incoming> {
        lock(&self.pending).clone()
    }

    pub fn take_pending(&self, id: &str) -> Option<Incoming> {
        let mut p = lock(&self.pending);
        let i = p.iter().position(|x| x.id == id)?;
        Some(p.remove(i))
    }

    pub fn transfer_values(&self) -> Vec<Value> {
        lock(&self.transfers)
            .iter()
            .filter_map(|t| serde_json::to_value(t).ok())
            .collect()
    }

    pub fn is_transfer(&self, id: &str) -> bool {
        lock(&self.transfers).iter().any(|t| t.id == id)
    }

    pub fn running(&self) -> usize {
        lock(&self.transfers)
            .iter()
            .filter(|t| t.status == "running")
            .count()
    }

    pub fn clear_finished(&self) {
        let mut t = lock(&self.transfers);
        t.retain(|t| !t.is_final());
        let ids: HashSet<String> = t.iter().map(|t| t.id.clone()).collect();
        drop(t);
        lock(&self.sends).retain(|id, _| ids.contains(id));
    }

    /// A send was started: remember it for `resend` and list it until the bridge reports `Started`.
    pub fn record_send(&self, id: &str, fp: &str, peer: &str, items: Vec<PathBuf>) {
        let job = job_of(&items);
        lock(&self.sends).insert(id.to_string(), (fp.to_string(), items));
        let mut t = lock(&self.transfers);
        if !t.iter().any(|t| t.id == id) {
            t.push(LsTransfer::new(id, &job, peer, true));
        }
    }

    pub fn send_of(&self, id: &str) -> Option<(String, Vec<PathBuf>)> {
        lock(&self.sends).get(id).cloned()
    }

    /// Marks a LocalSend transfer cancelled; the event for the UI, or None if `id` is not one.
    pub fn mark_cancelled(&self, id: &str) -> Option<Value> {
        let mut t = lock(&self.transfers);
        let tr = t.iter_mut().find(|t| t.id == id)?;
        if !tr.is_final() {
            tr.status = "cancelled";
            tr.speed_bps = 0;
            tr.eta_s = None;
        }
        Some(json!({ "type": "error", "id": id, "code": "cancelled", "msg": "cancelled", "retry_in_s": null }))
    }

    fn upsert(&self, id: &str, f: impl FnOnce(&mut LsTransfer)) -> LsTransfer {
        let mut t = lock(&self.transfers);
        let i = match t.iter().position(|t| t.id == id) {
            Some(i) => i,
            None => {
                t.push(LsTransfer::new(id, "LocalSend", "LocalSend", false));
                t.len() - 1
            }
        };
        f(&mut t[i]);
        t[i].clone()
    }

    /// What one bridge event means for the app (mutates the LocalSend state).
    pub fn on_ls(&self, s: &Settings, ev: LsEv) -> Vec<Effect> {
        let now = now_unix();
        match ev {
            LsEv::DevicesChanged => vec![Effect::Emit("nearby:changed", json!({}))],
            LsEv::Incoming {
                req_id,
                fingerprint,
                alias,
                files,
                bytes,
                names,
                text,
                ..
            } => {
                let trusted = s.ls_is_trusted(&fingerprint);
                if ls_auto_accept(s.receive_policy, trusted) {
                    return vec![Effect::Answer {
                        req_id,
                        accept: true,
                    }];
                }
                let job = match text {
                    Some(_) if files <= 1 => "tin nhắn".to_string(),
                    _ => ls_job(&names, files),
                };
                let inc = Incoming {
                    id: req_id.clone(),
                    source: "localsend".into(),
                    from_key: format!("l:{fingerprint}"),
                    from_name: alias.clone(),
                    stranger: !trusted,
                    job: job.clone(),
                    files,
                    bytes,
                    names: names.into_iter().take(5).collect(),
                    at: now,
                };
                {
                    let mut p = lock(&self.pending);
                    p.retain(|x| x.id != req_id);
                    p.push(inc.clone());
                }
                vec![
                    Effect::Emit("incoming", serde_json::to_value(&inc).unwrap_or_default()),
                    Effect::Notify {
                        body: ask_text(&alias, &job),
                        open: None,
                    },
                    Effect::Show,
                    Effect::CloseLater {
                        id: req_id,
                        secs: LS_ASK_SECS,
                        toss: false,
                    },
                ]
            }
            LsEv::Started {
                id,
                outgoing,
                peer,
                job,
                files,
                bytes,
            } => {
                let t = self.upsert(&id, |t| {
                    *t = LsTransfer {
                        status: "running",
                        bytes_total: bytes,
                        files_total: files,
                        started_at: if t.status == "connecting" && t.outgoing() == outgoing {
                            t.started_at
                        } else {
                            now
                        },
                        ..LsTransfer::new(&id, &job, &peer, outgoing)
                    };
                });
                vec![
                    Effect::Emit(
                        "transfer:event",
                        json!({ "type": "started", "id": id, "job": t.job, "op": t.op, "side": t.side, "peer": t.peer, "contact": null }),
                    ),
                    Effect::Emit(
                        "transfer:event",
                        json!({ "type": "manifest", "id": id, "files": files, "bytes": bytes, "to_do_files": files, "to_do_bytes": bytes }),
                    ),
                ]
            }
            LsEv::Progress {
                id,
                bytes_done,
                bytes_total,
                files_done,
                files_total,
                speed_bps,
            } => {
                let eta_s = (speed_bps > 0)
                    .then(|| bytes_total.saturating_sub(bytes_done) / speed_bps.max(1));
                let t = self.upsert(&id, |t| {
                    if !t.is_final() {
                        t.status = "running";
                    }
                    t.bytes_done = bytes_done;
                    t.bytes_total = bytes_total;
                    t.files_done = files_done;
                    t.files_total = files_total;
                    t.speed_bps = speed_bps;
                    t.eta_s = eta_s;
                });
                if t.is_final() {
                    return Vec::new();
                }
                vec![Effect::Emit(
                    "transfer:event",
                    json!({ "type": "progress", "id": id, "bytes_done": bytes_done, "bytes_total": bytes_total,
                            "files_done": files_done, "files_total": files_total, "speed_bps": speed_bps,
                            "eta_s": eta_s, "path": "direct", "rtt_ms": null, "streams": 1 }),
                )]
            }
            LsEv::Done {
                id,
                dest,
                files,
                bytes,
            } => {
                let dest = dest.map(|d| d.to_string_lossy().into_owned());
                let t = self.upsert(&id, |t| {
                    t.status = "done";
                    t.files_done = files;
                    t.files_total = files;
                    t.bytes_done = bytes;
                    t.bytes_total = bytes;
                    t.speed_bps = 0;
                    t.eta_s = None;
                    t.dest = dest.clone();
                });
                let mut fx = vec![Effect::Emit(
                    "transfer:event",
                    json!({ "type": "done", "id": id, "files": files, "bytes": bytes,
                            "elapsed_s": now.saturating_sub(t.started_at), "dest": t.dest }),
                )];
                if t.outgoing() {
                    fx.push(Effect::Notify {
                        body: format!("Đã gửi xong: {}", t.job),
                        open: None,
                    });
                } else {
                    let dest = t.dest.clone().unwrap_or_else(|| {
                        Path::new(&s.dest_dir)
                            .join(toss_core::paths::sanitize_component(&t.peer, "LocalSend"))
                            .to_string_lossy()
                            .into_owned()
                    });
                    fx.push(Effect::Received(Received {
                        id,
                        from_name: t.peer.clone(),
                        job: t.job.clone(),
                        files,
                        bytes,
                        dest,
                        at: now,
                    }));
                }
                fx
            }
            LsEv::Error { id, msg } => {
                // toss-ls: an ask that ended without an answer (timed out / sender gave up).
                if self.take_pending(&id).is_some() {
                    tracing::info!(%id, "LocalSend ask closed: {msg}");
                    return vec![Effect::Emit("incoming:closed", json!({ "id": id }))];
                }
                if !self.is_transfer(&id) {
                    tracing::warn!(%id, "LocalSend error: {msg}");
                    return Vec::new();
                }
                let mut was_cancelled = false;
                let t = self.upsert(&id, |t| {
                    was_cancelled = t.status == "cancelled";
                    if !was_cancelled {
                        t.status = "error";
                        t.error = Some(msg.clone());
                    }
                    t.speed_bps = 0;
                    t.eta_s = None;
                });
                let code = if was_cancelled { "cancelled" } else { "other" };
                let mut fx = vec![Effect::Emit(
                    "transfer:event",
                    json!({ "type": "error", "id": id, "code": code, "msg": msg, "retry_in_s": null }),
                )];
                if !was_cancelled {
                    let body = if t.outgoing() {
                        format!("Lỗi khi gửi “{}”: {msg}", t.job)
                    } else {
                        format!("Lỗi khi nhận “{}” từ {}: {msg}", t.job, t.peer)
                    };
                    fx.push(Effect::Notify { body, open: None });
                }
                fx
            }
            LsEv::Text { alias, text, .. } => {
                let msg = TextMsg {
                    from_name: alias.clone(),
                    text: text.clone(),
                    at: now,
                };
                vec![
                    Effect::Emit("text", serde_json::to_value(&msg).unwrap_or_default()),
                    Effect::Notify {
                        body: text_note(&alias, &text),
                        open: None,
                    },
                ]
            }
            LsEv::Port { port, fallback } => {
                *lock(&self.ls_port) = Some(port);
                if fallback {
                    tracing::warn!(port, "LocalSend port 53317 busy; using {port}");
                }
                vec![Effect::Emit(
                    "ls:port",
                    json!({ "port": port, "fallback": fallback }),
                )]
            }
        }
    }
}

/// Performs `effects` in the app.
pub fn apply(app: &AppHandle, effects: Vec<Effect>) {
    for e in effects {
        match e {
            Effect::Emit(name, payload) => {
                let _ = app.emit(name, payload);
            }
            Effect::Notify { body, open } => notify_open(app, &body, open),
            Effect::Show => crate::shell::show_main(app),
            Effect::Answer { req_id, accept } => {
                if let Some(ls) = app.state::<Arc<Core>>().v08.ls() {
                    ls.answer(&req_id, accept);
                }
            }
            Effect::Received(r) => {
                let core = app.state::<Arc<Core>>().inner().clone();
                core.v08.received.add(r.clone());
                let _ = app.emit("received", &r);
                notify_open(app, &received_text(&r), Some(r.dest.clone()));
            }
            Effect::CloseLater { id, secs, toss } => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(secs)).await;
                    let core = app.state::<Arc<Core>>().inner().clone();
                    let closed = if toss {
                        match core.node().await {
                            Ok(n) => !n.incoming_pushes().iter().any(|p| p.push_id == id),
                            Err(_) => true,
                        }
                    } else {
                        core.v08.take_pending(&id).is_some()
                    };
                    if closed {
                        let _ = app.emit("incoming:closed", json!({ "id": id }));
                    }
                });
            }
        }
    }
}

fn spawn_ls_pump(app: AppHandle, mut rx: mpsc::Receiver<LsEv>) {
    tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let core = app.state::<Arc<Core>>().inner().clone();
            let fx = core.v08.on_ls(&core.settings(), ev);
            apply(&app, fx);
        }
    });
}

/// macOS: one multicast datagram makes the OS ask for the Local Network permission now (first
/// run) instead of in the middle of the first transfer. Harmless elsewhere.
pub fn poke_local_network() -> Result<()> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").context("không mở được UDP")?;
    let _ = sock.set_multicast_ttl_v4(1);
    let target: std::net::SocketAddr = LS_MULTICAST.parse()?;
    if let Err(e) = sock.send_to(b"{}", target) {
        tracing::info!("local network poke: {e}");
    }
    Ok(())
}

// ---- Core methods -------------------------------------------------------------------------------

impl Core {
    fn emit(&self, name: &str, payload: Value) {
        if let Some(app) = self.app_opt() {
            let _ = app.emit(name, payload);
        }
    }

    /// Starts the LocalSend bridge when enabled and not running (startup, `set_ls_enabled`).
    pub async fn start_ls(&self) -> Result<()> {
        let _g = self.v08.ls_gate.lock().await;
        let s = self.settings();
        if !s.ls_enabled || self.v08.ls().is_some() {
            return Ok(());
        }
        let (ls, rx) = Ls::start(&s.device_name, self.data_dir(), Path::new(&s.dest_dir))
            .await
            .context("không bật được LocalSend")?;
        *self.v08.ls_port.lock().unwrap_or_else(|p| p.into_inner()) = Some(ls.port());
        *self.v08.ls.lock().unwrap_or_else(|p| p.into_inner()) = Some(ls);
        match self.app_opt() {
            Some(app) => spawn_ls_pump(app.clone(), rx),
            None => drop(rx),
        }
        tracing::info!("LocalSend bridge started");
        self.emit("nearby:changed", json!({}));
        Ok(())
    }

    /// Stops the bridge; its unanswered asks are closed.
    pub async fn stop_ls(&self) {
        let _g = self.v08.ls_gate.lock().await;
        let Some(ls) = self.v08.ls.lock().unwrap_or_else(|p| p.into_inner()).take() else {
            return;
        };
        ls.stop().await;
        *self.v08.ls_port.lock().unwrap_or_else(|p| p.into_inner()) = None;
        let asks: Vec<Incoming> =
            std::mem::take(&mut *self.v08.pending.lock().unwrap_or_else(|p| p.into_inner()));
        for a in asks {
            self.emit("incoming:closed", json!({ "id": a.id }));
        }
        self.emit("nearby:changed", json!({}));
        tracing::info!("LocalSend bridge stopped");
    }

    pub async fn list_nearby(&self) -> Result<Vec<Nearby>> {
        let node = self.node().await?;
        let core: Vec<CoreNearby> = via_serde(&node.nearby())?;
        let contacts: Vec<Contact> = via_serde(&node.contacts())?;
        let ls = self.v08.ls().map(|l| l.devices()).unwrap_or_default();
        Ok(merge_nearby(
            &core,
            &contacts,
            &ls,
            &self.settings().ls_trusted,
        ))
    }

    pub async fn rescan_nearby(&self) -> Result<()> {
        if let Some(ls) = self.v08.ls() {
            ls.rescan().await;
        }
        let node = self.node().await?;
        tauri::async_runtime::spawn(async move { node.refresh_presence().await });
        Ok(())
    }

    /// Returns the transfer id.
    pub async fn send_to(&self, key: String, paths: Vec<String>) -> Result<String> {
        let paths = check_paths(paths)?;
        match parse_key(&key)? {
            Target::Toss(id) => Ok(self.node().await?.send_to_peer(&id, paths).await?.id),
            Target::Ls(fp) => {
                let ls = self.ls_or_err()?;
                let peer = ls
                    .devices()
                    .into_iter()
                    .find(|d| d.fingerprint == fp)
                    .map_or_else(|| "LocalSend".to_string(), |d| d.alias);
                let id = ls.send(&fp, paths.clone()).await?;
                self.v08.record_send(&id, &fp, &peer, paths);
                Ok(id)
            }
        }
    }

    pub async fn send_text(&self, key: String, text: String) -> Result<()> {
        check_text(&text)?;
        match parse_key(&key)? {
            Target::Toss(id) => self.node().await?.send_text(&id, &text).await,
            Target::Ls(fp) => self.ls_or_err()?.send_text(&fp, &text).await,
        }
    }

    fn ls_or_err(&self) -> Result<Ls> {
        self.v08
            .ls()
            .ok_or_else(|| anyhow!("Đang tắt “Hiện với máy LocalSend” — bật lại trong Cài đặt"))
    }

    pub async fn list_incoming(&self) -> Result<Vec<Incoming>> {
        let node = self.node().await?;
        let pushes: Vec<IncomingPush> = via_serde(&node.incoming_pushes())?;
        let mut v: Vec<Incoming> = pushes.iter().map(toss_incoming).collect();
        v.extend(self.v08.pending());
        v.sort_by_key(|i| i.at);
        Ok(v)
    }

    /// `always`: Toss → the core adds the stranger as contact / turns on auto-accept;
    /// LocalSend → the device joins `ls_trusted`.
    pub async fn answer_incoming(&self, id: String, accept: bool, always: bool) -> Result<()> {
        let id = id.trim().to_string();
        if let Some(inc) = self.v08.take_pending(&id) {
            if let Some(ls) = self.v08.ls() {
                ls.answer(&id, accept);
            }
            if accept && always {
                let fp = inc.from_key.trim_start_matches("l:").to_string();
                self.update_settings(|s| {
                    if !s.ls_is_trusted(&fp) {
                        s.ls_trusted.push(LsTrusted {
                            fingerprint: fp.clone(),
                            alias: inc.from_name.clone(),
                        });
                    }
                })?;
            }
        } else {
            self.node()
                .await?
                .answer_push(&id, accept, always)?;
        }
        self.emit("incoming:closed", json!({ "id": id }));
        Ok(())
    }

    pub async fn set_receive_policy(&self, policy: ReceivePolicy) -> Result<()> {
        self.update_settings(|s| s.receive_policy = policy)?;
        self.node().await?.set_receive_policy(policy);
        Ok(())
    }

    pub async fn set_ls_enabled(&self, on: bool) -> Result<()> {
        self.update_settings(|s| s.ls_enabled = on)?;
        if on {
            self.start_ls().await
        } else {
            self.stop_ls().await;
            Ok(())
        }
    }

    pub fn forget_trusted(&self, fingerprint: String) -> Result<()> {
        let fp = fingerprint.trim().to_string();
        self.update_settings(|s| s.ls_trusted.retain(|t| t.fingerprint != fp))
    }

    pub fn list_received(&self) -> Vec<Received> {
        self.v08.received.list()
    }

    /// Same items to the same target; returns the new transfer id.
    pub async fn resend(&self, id: String) -> Result<String> {
        let id = id.trim().to_string();
        if let Some((fp, items)) = self.v08.send_of(&id) {
            if let Some(p) = items.iter().find(|p| !p.exists()) {
                bail!("Không tìm thấy {}", p.display());
            }
            let ls = self.ls_or_err()?;
            let peer = lock(&self.v08.transfers)
                .iter()
                .find(|t| t.id == id)
                .map_or_else(|| "LocalSend".to_string(), |t| t.peer.clone());
            let new_id = ls.send(&fp, items.clone()).await?;
            self.v08.record_send(&new_id, &fp, &peer, items);
            return Ok(new_id);
        }
        Ok(self.node().await?.resend(&id).await?.id)
    }

    pub async fn web_share_start(&self, paths: Vec<String>) -> Result<WebShare> {
        let paths = check_paths(paths)?;
        let ls = self
            .v08
            .ls()
            .ok_or_else(|| anyhow!("Bật “Hiện với máy LocalSend” trong Cài đặt để chia sẻ qua trình duyệt"))?;
        let urls = ls.web_share_start(paths).await?;
        let Some(first) = urls.first() else {
            ls.web_share_stop().await;
            bail!("Máy này chưa nối mạng LAN — nối Wi-Fi hoặc dây mạng rồi thử lại");
        };
        let qr_svg = qr_svg(first)?;
        Ok(WebShare { urls, qr_svg })
    }

    pub async fn web_share_stop(&self) {
        if let Some(ls) = self.v08.ls() {
            ls.web_share_stop().await;
        }
    }

    /// v0.8 migration, once: autostart on (with `--hidden`). Needs the app (autostart plugin).
    pub fn migrate_v08(&self) -> Result<bool> {
        if self.settings().migrated_v08 {
            return Ok(false);
        }
        use tauri_plugin_autostart::ManagerExt as _;
        let app = self
            .app_opt()
            .ok_or_else(|| anyhow!("app chưa sẵn sàng"))?;
        app.autolaunch()
            .enable()
            .map_err(|e| anyhow!("không bật được tự khởi động: {e}"))?;
        self.update_settings(|s| {
            s.autostart = true;
            s.migrated_v08 = true;
        })?;
        tracing::info!("0.8 migration: autostart on");
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(fp: &str, alias: &str, model: Option<&str>, kind: &str, ip: &str, toss: bool) -> LsDev {
        LsDev {
            fingerprint: fp.into(),
            alias: alias.into(),
            model: model.map(Into::into),
            kind: kind.into(),
            ip: ip.into(),
            is_toss: toss,
        }
    }

    fn peer(id: &str, name: &str, addrs: &[&str], contact: bool) -> CoreNearby {
        CoreNearby {
            id: id.into(),
            name: name.into(),
            platform: "macos".into(),
            version: "0.8.0".into(),
            addrs: addrs.iter().map(|a| a.to_string()).collect(),
            last_seen: 1,
            contact,
        }
    }

    fn contact(id: &str, name: &str, auto: bool) -> Contact {
        Contact {
            id: id.into(),
            name: name.into(),
            their_name: name.into(),
            platform: "windows".into(),
            added_at: 1,
            auto_accept: auto,
            online: true,
            last_seen: 1,
            rtt_ms: None,
        }
    }

    #[test]
    fn keys() {
        assert_eq!(parse_key("t:ab12").unwrap(), Target::Toss("ab12".into()));
        assert_eq!(parse_key(" l:FP ").unwrap(), Target::Ls("FP".into()));
        assert_eq!(parse_key("ab12").unwrap(), Target::Toss("ab12".into()));
        assert!(parse_key("l:").is_err() && parse_key("  ").is_err());
    }

    #[test]
    fn ips() {
        assert_eq!(addr_ip("192.168.1.5:51234").as_deref(), Some("192.168.1.5"));
        assert_eq!(addr_ip("[fe80::1]:5").as_deref(), Some("fe80::1"));
        assert_eq!(addr_ip("10.0.0.2").as_deref(), Some("10.0.0.2"));
        assert_eq!(addr_ip("relay"), None);
    }

    #[test]
    fn nearby_merge_dedups_by_ip() {
        let core = [
            peer("aa", "Mac Đạt", &["192.168.1.5:51234"], true),
            peer("bb", "Máy lạ", &["192.168.1.9:4000"], false),
        ];
        let contacts = [contact("aa", "Đạt (studio)", true)];
        let ls = [
            dev("f1", "Mac Đạt", Some("Toss"), "desktop", "192.168.1.5", true), // same machine
            dev("f2", "PC kho", Some("Toss"), "desktop", "192.168.1.20", true), // mDNS blocked there
            dev("f3", "iPhone của Minh", Some("iPhone"), "mobile", "192.168.1.30", false),
            dev("f4", "Galaxy", Some("Samsung"), "mobile", "192.168.1.31", false),
            dev("f3", "iPhone của Minh", Some("iPhone"), "mobile", "192.168.1.30", false),
        ];
        let trusted = [LsTrusted {
            fingerprint: "f3".into(),
            alias: "iPhone".into(),
        }];
        let n = merge_nearby(&core, &contacts, &ls, &trusted);
        let keys: Vec<(&str, &str)> = n.iter().map(|x| (x.key.as_str(), x.kind)).collect();
        assert_eq!(
            keys,
            [
                ("t:aa", "toss"),
                ("t:bb", "toss"),
                ("l:f3", "localsend"),
                ("l:f4", "localsend"),
                ("l:f2", "toss_lan"),
            ]
        );
        assert_eq!(n[0].name, "Đạt (studio)", "contact display name");
        assert_eq!(n[0].contact_id.as_deref(), Some("aa"));
        assert!(n[0].trusted && !n[1].trusted && n[1].contact_id.is_none());
        assert_eq!(n[2].platform, "ios");
        assert!(n[2].trusted);
        assert_eq!(n[3].platform, "android");
        let v = serde_json::to_value(&n[0]).unwrap();
        for k in ["key", "kind", "name", "model", "platform", "contact_id", "trusted"] {
            assert!(v.get(k).is_some(), "Nearby.{k}");
        }
    }

    #[test]
    fn platforms() {
        assert_eq!(ls_platform("desktop", Some("Windows")), "windows");
        assert_eq!(ls_platform("desktop", Some("macOS")), "macos");
        assert_eq!(ls_platform("desktop", Some("Linux")), "linux");
        assert_eq!(ls_platform("web", None), "web");
        assert_eq!(ls_platform("headless", None), "");
    }

    #[test]
    fn policy_matrix() {
        use ReceivePolicy::*;
        assert!(ls_auto_accept(Everyone, false) && ls_auto_accept(Everyone, true));
        assert!(ls_auto_accept(Contacts, true) && !ls_auto_accept(Contacts, false));
        assert!(!ls_auto_accept(Off, true) && !ls_auto_accept(Off, false));
    }

    #[test]
    fn job_names() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(ls_job(&s(&["12A3/a.jpg", "12A3/sub/b.jpg"]), 9), "12A3");
        assert_eq!(ls_job(&s(&["a.jpg"]), 1), "a.jpg");
        assert_eq!(ls_job(&s(&["a.jpg", "b.jpg"]), 7), "7 file");
        assert_eq!(ls_job(&s(&["x/a.jpg", "y/b.jpg"]), 2), "2 file");
        assert_eq!(ls_job(&[], 3), "3 file");
        assert_eq!(job_of(&[PathBuf::from("/a/Lớp 12A3")]), "Lớp 12A3");
        assert_eq!(job_of(&[PathBuf::from("/a"), PathBuf::from("/b")]), "2 file");
    }

    #[test]
    fn texts() {
        assert!(check_text("  ").is_err());
        assert!(check_text(&"x".repeat(MAX_TEXT + 1)).is_err());
        assert!(check_text("chào").is_ok());
        assert_eq!(text_note("Đạt", "  hi  "), "Đạt: hi");
        assert!(text_note("Đạt", &"a".repeat(200)).ends_with('…'));
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("toss-nearby-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn received_history_keeps_last_30() {
        let dir = tmp("recv");
        let log = ReceivedLog::load(&dir);
        for i in 0..35 {
            log.add(Received {
                id: format!("r{i}"),
                from_name: "Đạt".into(),
                job: format!("job {i}"),
                files: 1,
                bytes: 2,
                dest: "/d".into(),
                at: i,
            });
        }
        let l = ReceivedLog::load(&dir).list();
        assert_eq!(l.len(), 30);
        assert_eq!(l[0].id, "r34", "newest first");
        assert_eq!(l[29].id, "r5");
        let _ = fs::remove_dir_all(&dir);
    }

    fn incoming(fp: &str) -> LsEv {
        LsEv::Incoming {
            req_id: "q1".into(),
            fingerprint: fp.into(),
            alias: "iPhone của Minh".into(),
            model: Some("iPhone".into()),
            files: 3,
            bytes: 300,
            names: vec!["Album/a.jpg".into(), "Album/b.jpg".into()],
            text: None,
        }
    }

    #[test]
    fn ls_incoming_follows_the_policy() {
        let dir = tmp("ask");
        let st = V08::new(&dir);
        let mut s = Settings {
            dest_dir: "/inbox".into(),
            ..Settings::default()
        };
        // contacts (default) + stranger → ask: emit, notify, show, close timer
        let fx = st.on_ls(&s, incoming("fp1"));
        assert_eq!(fx.len(), 4);
        let Effect::Emit("incoming", v) = &fx[0] else {
            panic!("{fx:?}")
        };
        assert_eq!(
            (v["id"].as_str(), v["source"].as_str(), v["from_key"].as_str(), v["job"].as_str()),
            (Some("q1"), Some("localsend"), Some("l:fp1"), Some("Album"))
        );
        assert_eq!(v["stranger"], true);
        assert_eq!(
            fx[1],
            Effect::Notify {
                body: "iPhone của Minh muốn gửi Album".into(),
                open: None
            }
        );
        assert_eq!(fx[2], Effect::Show);
        assert!(matches!(&fx[3], Effect::CloseLater { secs: 60, toss: false, .. }));
        assert_eq!(st.pending().len(), 1);
        // the bridge gives up on the ask → closed, no transfer row
        assert_eq!(
            st.on_ls(&s, LsEv::Error { id: "q1".into(), msg: "hết giờ".into() }),
            vec![Effect::Emit("incoming:closed", json!({ "id": "q1" }))]
        );
        assert!(st.pending().is_empty() && st.transfer_values().is_empty());
        assert!(st.on_ls(&s, LsEv::Error { id: "zz".into(), msg: "x".into() }).is_empty());
        // trusted → auto accept
        s.ls_trusted.push(LsTrusted {
            fingerprint: "fp1".into(),
            alias: "x".into(),
        });
        assert_eq!(
            st.on_ls(&s, incoming("fp1")),
            vec![Effect::Answer {
                req_id: "q1".into(),
                accept: true
            }]
        );
        // off → even trusted asks; everyone → strangers accepted
        s.receive_policy = ReceivePolicy::Off;
        assert_eq!(st.on_ls(&s, incoming("fp1")).len(), 4);
        s.receive_policy = ReceivePolicy::Everyone;
        assert!(matches!(
            st.on_ls(&s, incoming("other"))[0],
            Effect::Answer { accept: true, .. }
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ls_transfers_track_and_finish() {
        let dir = tmp("xfer");
        let st = V08::new(&dir);
        let s = Settings {
            dest_dir: "/inbox".into(),
            ..Settings::default()
        };
        let fx = st.on_ls(
            &s,
            LsEv::Started {
                id: "x1".into(),
                outgoing: false,
                peer: "iPhone".into(),
                job: "Album".into(),
                files: 2,
                bytes: 100,
            },
        );
        let Effect::Emit("transfer:event", v) = &fx[0] else {
            panic!()
        };
        assert_eq!((v["type"].as_str(), v["side"].as_str(), v["op"].as_str()), (Some("started"), Some("host"), Some("put")));
        assert!(v["contact"].is_null());
        st.on_ls(
            &s,
            LsEv::Progress {
                id: "x1".into(),
                bytes_done: 50,
                bytes_total: 100,
                files_done: 1,
                files_total: 2,
                speed_bps: 25,
            },
        );
        let snap = &st.transfer_values()[0];
        assert_eq!(snap["status"], "running");
        assert_eq!(snap["eta_s"], 2);
        assert_eq!(snap["path"], "direct");
        assert!(snap["contact"].is_null());
        assert_eq!(st.running(), 1);
        let fx = st.on_ls(
            &s,
            LsEv::Done {
                id: "x1".into(),
                dest: Some(PathBuf::from("/inbox/iPhone/Album")),
                files: 2,
                bytes: 100,
            },
        );
        assert!(matches!(&fx[0], Effect::Emit("transfer:event", v) if v["type"] == "done"));
        let Effect::Received(r) = &fx[1] else {
            panic!("{fx:?}")
        };
        assert_eq!(
            (r.from_name.as_str(), r.job.as_str(), r.dest.as_str(), r.files),
            ("iPhone", "Album", "/inbox/iPhone/Album", 2)
        );
        // outgoing: resend info, error notifies, cancel wins over the error that follows
        st.record_send("o1", "fp", "Galaxy", vec![PathBuf::from("/x/Lớp 12A3")]);
        assert_eq!(st.transfer_values()[1]["job"], "Lớp 12A3");
        assert_eq!(st.send_of("o1").unwrap().0, "fp");
        assert!(st.mark_cancelled("o1").is_some());
        let fx = st.on_ls(
            &s,
            LsEv::Error {
                id: "o1".into(),
                msg: "huỷ".into(),
            },
        );
        assert_eq!(fx.len(), 1, "no error notification after a cancel");
        assert!(matches!(&fx[0], Effect::Emit(_, v) if v["code"] == "cancelled"));
        assert!(st.mark_cancelled("nope").is_none());
        st.clear_finished();
        assert!(st.transfer_values().is_empty() && st.send_of("o1").is_none());
        // a partly failed receive ends with Error (not Done): notified, status error
        st.on_ls(
            &s,
            LsEv::Started {
                id: "x2".into(),
                outgoing: false,
                peer: "iPhone".into(),
                job: "Album 2".into(),
                files: 2,
                bytes: 10,
            },
        );
        let fx = st.on_ls(
            &s,
            LsEv::Error {
                id: "x2".into(),
                msg: "thiếu 1 file".into(),
            },
        );
        assert!(matches!(&fx[1], Effect::Notify { body, .. } if body == "Lỗi khi nhận “Album 2” từ iPhone: thiếu 1 file"));
        assert_eq!(st.transfer_values()[0]["status"], "error");
        st.clear_finished();
        // text + port
        let fx = st.on_ls(
            &s,
            LsEv::Text {
                fingerprint: "fp".into(),
                alias: "Galaxy".into(),
                text: "địa chỉ studio".into(),
            },
        );
        assert!(matches!(&fx[0], Effect::Emit("text", v) if v["text"] == "địa chỉ studio" && v["from_name"] == "Galaxy"));
        st.on_ls(&s, LsEv::Port { port: 53318, fallback: true });
        assert_eq!(st.ls_port(), Some(53318));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn toss_push_to_incoming() {
        let mut p = IncomingPush {
            push_id: "p1".into(),
            from_id: "aa".into(),
            from_name: "Đạt".into(),
            job: "12A3".into(),
            files: 3,
            bytes: 9,
            at: 5,
            stranger: false,
        };
        let i = toss_incoming(&p);
        assert_eq!((i.source.as_str(), i.from_key.as_str(), i.stranger), ("toss", "aa", false));
        p.stranger = true;
        assert_eq!(toss_incoming(&p).from_key, "t:aa");
        // a 0.6 core event (no `stranger`) still reads
        let old: IncomingPush = serde_json::from_value(serde_json::json!({
            "push_id": "p", "from_id": "a", "from_name": "n", "job": "j", "files": 1, "bytes": 1, "at": 1
        }))
        .unwrap();
        assert!(!old.stranger);
    }

    #[test]
    fn local_network_poke_does_not_fail() {
        poke_local_network().unwrap();
    }
}
