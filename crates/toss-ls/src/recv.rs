//! Receiving: the v2 server's events → `Incoming`/`Text`/progress, safe save paths.

use crate::files::{multi_line, safe_rel, sanitize_component, single_line, unique};
use crate::send::Speed;
use crate::{lock, Inner, LsEvent};
use localsend::discovery::{DeviceChannel, DiscoveredDevice, HttpChannel};
use localsend::http::client::LsHttpClientV2;
use localsend::http::dto_v2::RegisterDtoV2;
use localsend::http::server::common::save::FileUploadTarget;
use localsend::http::server::v2::{PrepareUploadDecisionV2, ServerEventV2, SessionEndReasonV2};
use localsend::http::server::PeerIp;
use localsend::model::discovery::ProtocolType;
use localsend::model::transfer::FileDto;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Longest text message kept (characters).
const MAX_TEXT_CHARS: usize = 100_000;

/// Where to reach the sender (receiver-side cancel).
#[derive(Clone)]
pub(crate) struct SenderAddr {
    host: String,
    port: u16,
    protocol: ProtocolType,
    fingerprint: String,
}

pub(crate) struct Pending {
    decision_tx: oneshot::Sender<PrepareUploadDecisionV2>,
    alias: String,
    sender: SenderAddr,
    /// Accepted-able files: id → (dto, sanitized components).
    files: HashMap<String, (FileDto, Vec<String>)>,
}

pub(crate) struct RecvSession {
    sender: SenderAddr,
    /// `<dest>/<sanitized alias>`
    root: PathBuf,
    /// Top folder name in the request → folder name used on disk (no clobber).
    top_map: HashMap<String, String>,
    dest: PathBuf,
    files: HashMap<String, (FileDto, Vec<String>)>,
    total_bytes: u64,
    ok_files: HashSet<String>,
    finalized: u64,
    inflight: HashMap<String, Arc<AtomicU64>>,
    reserved: HashSet<PathBuf>,
    ended: Option<SessionEndReasonV2>,
}

impl RecvSession {
    fn done_bytes(&self) -> u64 {
        self.finalized
            + self
                .inflight
                .values()
                .map(|p| p.load(Ordering::Relaxed))
                .sum::<u64>()
    }

    /// The path a file is written to: inside `root`, sanitized, no clobber.
    fn target_path(&mut self, comps: &[String]) -> Option<PathBuf> {
        let (last, dirs) = comps.split_last()?;
        let mut parent = self.root.clone();
        if let Some((top, rest)) = dirs.split_first() {
            parent.push(self.top_map.get(top)?);
            for c in rest {
                parent.push(c);
            }
        }
        std::fs::create_dir_all(&parent).ok()?;
        // A symlinked folder inside the inbox must not lead outside of it.
        let canon_parent = parent.canonicalize().ok()?;
        let canon_root = self.root.canonicalize().ok()?;
        if !canon_parent.starts_with(&canon_root) {
            return None;
        }
        let p = unique(parent.join(last), &self.reserved, false);
        self.reserved.insert(p.clone());
        Some(p)
    }
}

/// A "message request": one text file whose content is in `preview`.
fn message_of(files: &HashMap<String, FileDto>) -> Option<&str> {
    if files.len() != 1 {
        return None;
    }
    let f = files.values().next()?;
    if f.file_type != "text" && !f.file_type.starts_with("text/") {
        return None;
    }
    f.preview.as_deref()
}

/// Job name: the common top folder, the single file's name, or "N file".
pub(crate) fn job_name<'a>(names: impl Iterator<Item = &'a [String]>) -> (String, bool) {
    let all: Vec<&[String]> = names.collect();
    if let Some(first) = all.first() {
        let top = &first[0];
        let folder = all.iter().all(|c| c.len() > 1 && &c[0] == top);
        if folder {
            return (top.clone(), true);
        }
        if all.len() == 1 {
            return (first.last().cloned().unwrap_or_default(), false);
        }
    }
    (format!("{} file", all.len()), false)
}

fn alias_of(raw: &str) -> String {
    let a = single_line(raw);
    if a.is_empty() {
        "LocalSend".into()
    } else {
        a
    }
}

impl Inner {
    pub(crate) async fn server_loop(self: Arc<Self>, mut rx: mpsc::Receiver<ServerEventV2>) {
        loop {
            let ev = tokio::select! {
                ev = rx.recv() => ev,
                _ = self.stopped.cancelled() => None,
            };
            let Some(ev) = ev else {
                // Stopped: refuse what is still queued so no request hangs.
                while let Ok(ev) = rx.try_recv() {
                    if let ServerEventV2::PrepareUpload { decision_tx, .. } = ev {
                        let _ = decision_tx.send(PrepareUploadDecisionV2::Decline);
                    }
                }
                return;
            };
            self.on_server_event(ev);
        }
    }

    /// Puts a peer that contacted us into the discovery store.
    fn note_device(self: &Arc<Self>, ip: &PeerIp, info: &RegisterDtoV2, fingerprint: String) {
        if fingerprint.eq_ignore_ascii_case(&self.ident.fingerprint) {
            return;
        }
        let Some(d) = self.discovery() else { return };
        let dev = DiscoveredDevice {
            alias: info.alias.clone(),
            version: info.version.clone(),
            device_model: info.device_model.clone(),
            device_type: info.device_type.clone(),
            fingerprint,
            channel: DeviceChannel::Http(HttpChannel {
                host: ip.to_string(),
                port: info.port,
                protocol: info.protocol,
            }),
            download: info.download,
        };
        let me = self.clone();
        self.rt.spawn(async move {
            d.add_device(dev).await;
            me.devices_maybe_changed();
        });
    }

    fn on_server_event(self: &Arc<Self>, ev: ServerEventV2) {
        match ev {
            ServerEventV2::Register { ip, info } => {
                let fp = match info.protocol {
                    ProtocolType::Https => info.fingerprint.to_ascii_uppercase(),
                    ProtocolType::Http => info.fingerprint.clone(),
                };
                self.note_device(&ip, &info, fp);
            }
            ServerEventV2::PrepareUpload {
                session_id,
                ip,
                info,
                cert_fingerprint,
                files,
                decision_tx,
            } => self.on_prepare(session_id, ip, info, cert_fingerprint, files, decision_tx),
            ServerEventV2::FileUpload {
                session_id,
                file_id,
                file: _,
                target_tx,
            } => self.on_file_upload(session_id, file_id, target_tx),
            ServerEventV2::SessionEnd { session_id, reason } => {
                if let Some(s) = lock(&self.recv).get_mut(&session_id) {
                    s.ended = Some(reason);
                }
                self.check_finish(&session_id);
            }
            ServerEventV2::PrepareUploadAborted { session_id } => {
                if lock(&self.pending).remove(&session_id).is_some() {
                    self.emit(LsEvent::Error {
                        id: session_id,
                        msg: "Máy kia đã huỷ yêu cầu gửi".into(),
                    });
                }
                self.maybe_deferred_restart();
            }
            ServerEventV2::CancelReceived { ip, session_id } => {
                let sends = lock(&self.sends);
                for ctl in sends.values() {
                    let matches = lock(&ctl.remote_sid).as_deref() == Some(session_id.as_str());
                    let host = ctl.host.split('%').next().unwrap_or(&ctl.host);
                    if matches && host == ip.ip.to_string() {
                        ctl.by_peer.store(true, Ordering::Relaxed);
                        ctl.cancel.cancel();
                    }
                }
            }
            ServerEventV2::ListenerFailed { error } => {
                tracing::warn!("LocalSend listener failed, restarting: {error}");
                let me = self.clone();
                self.rt.spawn(async move {
                    for attempt in 0..10u32 {
                        if me.stopped.is_cancelled() {
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
                        match me.restart_server().await {
                            Ok(()) => return,
                            Err(e) => tracing::warn!("LocalSend server restart failed: {e:#}"),
                        }
                    }
                });
            }
        }
    }

    fn on_prepare(
        self: &Arc<Self>,
        session_id: String,
        ip: PeerIp,
        info: RegisterDtoV2,
        cert_fingerprint: Option<String>,
        files: HashMap<String, FileDto>,
        decision_tx: oneshot::Sender<PrepareUploadDecisionV2>,
    ) {
        let fp = cert_fingerprint.unwrap_or_else(|| info.fingerprint.clone());
        if self.stopped.is_cancelled() || fp.eq_ignore_ascii_case(&self.ident.fingerprint) {
            let _ = decision_tx.send(PrepareUploadDecisionV2::Decline);
            return;
        }
        self.note_device(&ip, &info, fp.clone());
        let alias = alias_of(&info.alias);

        if let Some(text) = message_of(&files) {
            // Nothing to transfer: accepting no file answers 204 and ends the request.
            let mut text = multi_line(text);
            if text.chars().count() > MAX_TEXT_CHARS {
                text = text.chars().take(MAX_TEXT_CHARS).collect();
            }
            let _ = decision_tx.send(PrepareUploadDecisionV2::Accept(HashSet::new()));
            self.emit(LsEvent::Text {
                fingerprint: fp,
                alias,
                text,
            });
            return;
        }

        let mut ok = HashMap::new();
        for (id, f) in files {
            match safe_rel(&f.file_name) {
                Some(c) => {
                    ok.insert(id, (f, c));
                }
                None => tracing::warn!(
                    "LocalSend: rejected unsafe file name {:?} from {alias}",
                    f.file_name
                ),
            }
        }
        if ok.is_empty() {
            let _ = decision_tx.send(PrepareUploadDecisionV2::Decline);
            return;
        }
        let bytes: u64 = ok.values().map(|(f, _)| f.size).sum();
        let mut names: Vec<String> = ok.values().map(|(_, c)| c.join("/")).collect();
        names.sort();
        names.truncate(5);
        let n = ok.len() as u32;
        let sender = SenderAddr {
            host: ip.to_string(),
            port: info.port,
            protocol: info.protocol,
            fingerprint: fp.clone(),
        };
        lock(&self.pending).insert(
            session_id.clone(),
            Pending {
                decision_tx,
                alias: alias.clone(),
                sender,
                files: ok,
            },
        );
        self.emit(LsEvent::Incoming {
            req_id: session_id.clone(),
            fingerprint: fp,
            alias,
            model: info.device_model.map(|m| single_line(&m)),
            ip: ip.ip.to_string(),
            files: n,
            bytes,
            names,
            text: None,
        });
        let me = self.clone();
        let t = self.opts.answer_timeout;
        self.rt.spawn(async move {
            tokio::time::sleep(t).await;
            let p = lock(&me.pending).remove(&session_id);
            if let Some(p) = p {
                let _ = p.decision_tx.send(PrepareUploadDecisionV2::Decline);
                me.emit(LsEvent::Error {
                    id: session_id,
                    msg: format!("Không trả lời trong {} giây — đã từ chối", t.as_secs()),
                });
                me.maybe_deferred_restart();
            }
        });
    }

    fn on_file_upload(
        self: &Arc<Self>,
        session_id: String,
        file_id: String,
        target_tx: oneshot::Sender<FileUploadTarget>,
    ) {
        let (path, progress) = {
            let mut map = lock(&self.recv);
            // Unknown session: dropping the responder fails the request (500).
            let Some(s) = map.get_mut(&session_id) else {
                return;
            };
            let Some((_, comps)) = s.files.get(&file_id).cloned() else {
                return;
            };
            let Some(path) = s.target_path(&comps) else {
                tracing::warn!("LocalSend: no safe path for {:?}", comps);
                return;
            };
            let p = Arc::new(AtomicU64::new(0));
            s.inflight.insert(file_id.clone(), p.clone());
            (path, p)
        };
        let (ptx, mut prx) = mpsc::channel::<u64>(16);
        self.rt.spawn(async move {
            while let Some(w) = prx.recv().await {
                progress.store(w, Ordering::Relaxed);
            }
        });
        let (rtx, rrx) = oneshot::channel();
        let sent = target_tx.send(FileUploadTarget::Path {
            path: path.clone(),
            result_tx: rtx,
            progress_tx: Some(ptx),
        });
        let me = self.clone();
        self.rt.spawn(async move {
            let res = match sent {
                Ok(()) => rrx.await.unwrap_or_else(|_| Err("aborted".into())),
                Err(_) => Err("aborted".into()),
            };
            me.file_result(&session_id, &file_id, &path, res);
        });
    }

    fn file_result(
        self: &Arc<Self>,
        session_id: &str,
        file_id: &str,
        path: &Path,
        res: Result<(), String>,
    ) {
        if let Err(e) = &res {
            tracing::warn!("LocalSend: receiving {} failed: {e}", path.display());
            let _ = std::fs::remove_file(path);
        }
        {
            let mut map = lock(&self.recv);
            let Some(s) = map.get_mut(session_id) else {
                return;
            };
            s.inflight.remove(file_id);
            match res {
                Ok(()) => {
                    if s.ok_files.insert(file_id.to_string()) {
                        s.finalized += s.files.get(file_id).map(|(f, _)| f.size).unwrap_or(0);
                    }
                }
                Err(_) => {
                    s.reserved.remove(path);
                }
            }
        }
        self.check_finish(session_id);
    }

    fn check_finish(self: &Arc<Self>, session_id: &str) {
        let s = {
            let mut map = lock(&self.recv);
            let done = map
                .get(session_id)
                .is_some_and(|s| s.ended.is_some() && s.inflight.is_empty());
            if !done {
                return;
            }
            map.remove(session_id)
        };
        let Some(s) = s else { return };
        let total = s.files.len() as u32;
        let ok = s.ok_files.len() as u32;
        let id = session_id.to_string();
        match s.ended {
            Some(SessionEndReasonV2::Finished) if ok == total => {
                self.emit(LsEvent::Progress {
                    id: id.clone(),
                    bytes_done: s.total_bytes,
                    bytes_total: s.total_bytes,
                    files_done: ok,
                    files_total: total,
                    speed_bps: 0,
                });
                self.emit(LsEvent::Done {
                    id,
                    dest: Some(s.dest),
                    files: ok,
                    bytes: s.finalized,
                });
            }
            Some(SessionEndReasonV2::Finished) => self.emit(LsEvent::Error {
                id,
                msg: format!("Nhận được {ok}/{total} file — {} file bị lỗi", total - ok),
            }),
            _ => self.emit(LsEvent::Error {
                id,
                msg: format!("Máy kia đã huỷ ({ok}/{total} file đã nhận)"),
            }),
        }
        self.maybe_deferred_restart();
    }

    async fn recv_ticker(self: Arc<Self>, id: String) {
        let mut speed = Speed::new();
        let mut last = u64::MAX;
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                _ = self.stopped.cancelled() => return,
            }
            let snap = {
                let map = lock(&self.recv);
                let Some(s) = map.get(&id) else { return };
                (
                    s.done_bytes(),
                    s.total_bytes,
                    s.ok_files.len() as u32,
                    s.files.len() as u32,
                )
            };
            let bps = speed.sample(snap.0);
            if snap.0 != last {
                last = snap.0;
                self.emit(LsEvent::Progress {
                    id: id.clone(),
                    bytes_done: snap.0.min(snap.1),
                    bytes_total: snap.1,
                    files_done: snap.2,
                    files_total: snap.3,
                    speed_bps: bps,
                });
            }
        }
    }
}

pub(crate) fn answer(inner: &Arc<Inner>, req_id: &str, accept: bool) {
    let Some(p) = lock(&inner.pending).remove(req_id) else {
        return;
    };
    if !accept || inner.stopped.is_cancelled() {
        let _ = p.decision_tx.send(PrepareUploadDecisionV2::Decline);
        inner.maybe_deferred_restart();
        return;
    }
    let dest = lock(&inner.dest).clone();
    let root = dest.join(sanitize_component(&p.alias, "LocalSend"));
    if let Err(e) = std::fs::create_dir_all(&root) {
        let _ = p.decision_tx.send(PrepareUploadDecisionV2::Decline);
        inner.emit(LsEvent::Error {
            id: req_id.into(),
            msg: format!("Không tạo được thư mục nhận: {e}"),
        });
        inner.maybe_deferred_restart();
        return;
    }
    let mut reserved = HashSet::new();
    let mut top_map = HashMap::new();
    let mut tops: Vec<&String> = p
        .files
        .values()
        .filter(|(_, c)| c.len() > 1)
        .map(|(_, c)| &c[0])
        .collect();
    tops.sort();
    tops.dedup();
    for t in tops {
        let dir = unique(root.join(t), &reserved, true);
        reserved.insert(dir.clone());
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| t.clone());
        top_map.insert(t.clone(), name);
    }
    let (job, is_folder) = job_name(p.files.values().map(|(_, c)| c.as_slice()));
    let dest_dir = match (is_folder, top_map.get(&job)) {
        (true, Some(mapped)) => root.join(mapped),
        _ => root.clone(),
    };
    let total_bytes: u64 = p.files.values().map(|(f, _)| f.size).sum();
    let n = p.files.len() as u32;
    let ids: HashSet<String> = p.files.keys().cloned().collect();
    lock(&inner.recv).insert(
        req_id.to_string(),
        RecvSession {
            sender: p.sender,
            root,
            top_map,
            dest: dest_dir,
            files: p.files,
            total_bytes,
            ok_files: HashSet::new(),
            finalized: 0,
            inflight: HashMap::new(),
            reserved,
            ended: None,
        },
    );
    if p.decision_tx
        .send(PrepareUploadDecisionV2::Accept(ids))
        .is_err()
    {
        lock(&inner.recv).remove(req_id);
        inner.emit(LsEvent::Error {
            id: req_id.into(),
            msg: "Máy kia đã huỷ yêu cầu gửi".into(),
        });
        inner.maybe_deferred_restart();
        return;
    }
    inner.emit(LsEvent::Started {
        id: req_id.into(),
        outgoing: false,
        peer: p.alias,
        job,
        files: n,
        bytes: total_bytes,
    });
    inner
        .rt
        .spawn(inner.clone().recv_ticker(req_id.to_string()));
}

/// Receiver-side cancel: refuse further uploads and tell the sender.
pub(crate) fn cancel_receive(inner: &Arc<Inner>, id: &str) {
    let Some(s) = lock(&inner.recv).remove(id) else {
        return;
    };
    let ok = s.ok_files.len();
    let total = s.files.len();
    inner.emit(LsEvent::Error {
        id: id.into(),
        msg: format!("Đã huỷ ({ok}/{total} file đã nhận)"),
    });
    let server = inner
        .server_handle
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let sender = s.sender;
    let sid = id.to_string();
    let (key, cert) = (inner.ident.key_pem.clone(), inner.ident.cert_pem.clone());
    inner.rt.spawn(async move {
        if let Some(server) = server {
            server.cancel_v2_session(&sid).await;
        }
        let pin = (sender.protocol == ProtocolType::Https).then(|| sender.fingerprint.clone());
        if let Ok(c) = LsHttpClientV2::try_new(&key, &cert, pin, Some(Duration::from_secs(5))) {
            let _ = c
                .cancel(sender.protocol, &sender.host, sender.port, &sid)
                .await;
        }
    });
    inner.maybe_deferred_restart();
}
