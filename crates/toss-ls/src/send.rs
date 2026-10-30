//! Sending files/folders/text to a LocalSend device.

use crate::files::{collect, OutFile};
use crate::{lock, Inner, LsEvent};
use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use localsend::http::client::{ClientError, LsHttpClientV2};
use localsend::http::dto_v2::PrepareUploadRequestDtoV2;
use localsend::model::discovery::ProtocolType;
use localsend::model::transfer::{FileContent, FileDto, FileMetadata};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// Parallel uploads per transfer.
const PARALLEL: usize = 2;
/// How long a receiver may take to accept.
const DECISION_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Largest text sent as a message (LocalSend CLI uses the same bound).
pub(crate) const MAX_TEXT_BYTES: usize = 64_000;

pub(crate) const MSG_PIN: &str = "Máy kia yêu cầu mã PIN — tắt PIN trong LocalSend của máy đó";
pub(crate) const MSG_DECLINED: &str = "Máy kia đã từ chối";
pub(crate) const MSG_BUSY: &str = "Máy kia đang bận";
pub(crate) const MSG_UNREACHABLE: &str =
    "Không kết nối được tới máy kia — kiểm tra hai máy cùng mạng";
pub(crate) const MSG_TIMEOUT: &str = "Máy kia không trả lời";
pub(crate) const MSG_CANCELLED: &str = "Đã huỷ";
pub(crate) const MSG_PEER_CANCELLED: &str = "Máy kia đã huỷ";

pub(crate) struct SendCtl {
    pub cancel: CancellationToken,
    pub by_peer: Arc<AtomicBool>,
    pub host: String,
    pub remote_sid: Arc<Mutex<Option<String>>>,
}

/// Bytes/s over the last ~2 s.
pub(crate) struct Speed {
    samples: VecDeque<(Instant, u64)>,
}

impl Speed {
    pub(crate) fn new() -> Self {
        Self {
            samples: VecDeque::new(),
        }
    }

    pub(crate) fn sample(&mut self, bytes: u64) -> u64 {
        let now = Instant::now();
        self.samples.push_back((now, bytes));
        while self.samples.len() > 2
            && now.duration_since(self.samples[0].0) > Duration::from_secs(2)
        {
            self.samples.pop_front();
        }
        let (t0, b0) = self.samples[0];
        let dt = now.duration_since(t0).as_secs_f64();
        if dt < 0.1 {
            return 0;
        }
        (bytes.saturating_sub(b0) as f64 / dt) as u64
    }
}

struct Target {
    host: String,
    port: u16,
    protocol: ProtocolType,
    fingerprint: String,
    alias: String,
}

fn target_of(inner: &Inner, fingerprint: &str) -> Result<Target> {
    let dev = inner
        .device(fingerprint)
        .ok_or_else(|| anyhow!("Không thấy máy này trong mạng — bấm làm mới rồi thử lại"))?;
    let http = dev
        .get_best_channel()
        .and_then(|c| c.http())
        .ok_or_else(|| anyhow!("Không có địa chỉ để kết nối tới máy này"))?;
    Ok(Target {
        host: http.host.clone(),
        port: http.port,
        protocol: http.protocol,
        fingerprint: dev.device.fingerprint.clone(),
        alias: crate::files::single_line(&dev.device.alias),
    })
}

fn client_for(inner: &Inner, t: &Target) -> Result<LsHttpClientV2, String> {
    let pin = (t.protocol == ProtocolType::Https).then(|| t.fingerprint.clone());
    LsHttpClientV2::try_new(&inner.ident.key_pem, &inner.ident.cert_pem, pin, None)
        .map_err(|e| format!("Lỗi khởi tạo kết nối: {e}"))
}

/// Quick TCP check so an unreachable peer fails in seconds, not minutes.
async fn reachable(host: &str, port: u16) -> bool {
    if host.contains('%') {
        return true; // scoped IPv6: let the HTTP client handle it
    }
    let addr = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    matches!(
        tokio::time::timeout(Duration::from_secs(4), tokio::net::TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}

pub(crate) fn vn_error(e: &ClientError) -> String {
    match e {
        ClientError::StatusCode(s) => match s.status {
            401 => MSG_PIN.into(),
            403 => MSG_DECLINED.into(),
            409 => MSG_BUSY.into(),
            429 => "Máy kia đang bận, thử lại sau ít phút".into(),
            422 => "File bị lỗi trên đường truyền (sai checksum)".into(),
            code => format!("Máy kia báo lỗi (mã {code})"),
        },
        ClientError::Reqwest(r) if r.is_timeout() => MSG_TIMEOUT.into(),
        ClientError::Reqwest(r) if r.is_connect() => MSG_UNREACHABLE.into(),
        ClientError::Reqwest(_) => "Mất kết nối với máy kia".into(),
        ClientError::Cancelled => MSG_CANCELLED.into(),
        ClientError::Io(e) => format!("Lỗi đọc file: {e}"),
        ClientError::Json(_) => "Máy kia trả lời không hợp lệ".into(),
        ClientError::Other(e) => format!("Lỗi kết nối: {e}"),
    }
}

fn dto(id: &str, f: &OutFile) -> FileDto {
    FileDto {
        id: id.into(),
        file_name: f.name.clone(),
        size: f.size,
        file_type: mime_of(&f.name).into(),
        sha256: None,
        preview: None,
        metadata: FileMetadata::from_path(&f.path),
    }
}

/// A small MIME guess so phones file photos/videos correctly.
pub(crate) fn mime_of(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "tif" | "tiff" => "image/tiff",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
}

pub(crate) async fn start_send(
    inner: &Arc<Inner>,
    fingerprint: &str,
    items: Vec<PathBuf>,
) -> Result<String> {
    if inner.stopped.is_cancelled() {
        return Err(anyhow!("LocalSend đã tắt"));
    }
    let t = target_of(inner, fingerprint)?;
    let files = tokio::task::spawn_blocking(move || collect(&items))
        .await
        .unwrap_or_default();
    if files.is_empty() {
        return Err(anyhow!(
            "Không có file nào để gửi (folder trống hoặc không đọc được)"
        ));
    }
    let comps: Vec<Vec<String>> = files
        .iter()
        .map(|f| f.name.split('/').map(String::from).collect())
        .collect();
    let (job, _) = crate::recv::job_name(comps.iter().map(|c| c.as_slice()));
    let id = uuid::Uuid::new_v4().to_string();
    let ctl = SendCtl {
        cancel: CancellationToken::new(),
        by_peer: Arc::new(AtomicBool::new(false)),
        host: t.host.clone(),
        remote_sid: Arc::new(Mutex::new(None)),
    };
    let (cancel, by_peer, remote_sid) = (
        ctl.cancel.clone(),
        ctl.by_peer.clone(),
        ctl.remote_sid.clone(),
    );
    lock(&inner.sends).insert(id.clone(), ctl);
    let bytes: u64 = files.iter().map(|f| f.size).sum();
    inner.emit(LsEvent::Started {
        id: id.clone(),
        outgoing: true,
        peer: t.alias.clone(),
        job,
        files: files.len() as u32,
        bytes,
    });
    let me = inner.clone();
    let tid = id.clone();
    inner.rt.spawn(async move {
        let res = run_send(&me, &tid, &t, files, &cancel, &by_peer, &remote_sid).await;
        lock(&me.sends).remove(&tid);
        match res {
            Ok((n, b)) => me.emit(LsEvent::Done {
                id: tid,
                dest: None,
                files: n,
                bytes: b,
            }),
            Err(msg) => me.emit(LsEvent::Error { id: tid, msg }),
        }
    });
    Ok(id)
}

async fn run_send(
    inner: &Arc<Inner>,
    id: &str,
    t: &Target,
    files: Vec<OutFile>,
    cancel: &CancellationToken,
    by_peer: &AtomicBool,
    remote_sid: &Mutex<Option<String>>,
) -> Result<(u32, u64), String> {
    let cancelled = || {
        if by_peer.load(Ordering::Relaxed) {
            MSG_PEER_CANCELLED.to_string()
        } else {
            MSG_CANCELLED.to_string()
        }
    };
    tokio::select! {
        ok = reachable(&t.host, t.port) => if !ok { return Err(MSG_UNREACHABLE.into()) },
        _ = cancel.cancelled() => return Err(cancelled()),
    }
    let client = client_for(inner, t)?;
    let mut by_id: HashMap<String, OutFile> = HashMap::new();
    let mut dtos = HashMap::new();
    for f in files {
        let fid = uuid::Uuid::new_v4().to_string();
        dtos.insert(fid.clone(), dto(&fid, &f));
        by_id.insert(fid, f);
    }
    let payload = PrepareUploadRequestDtoV2 {
        info: inner.register_dto(),
        files: dtos,
    };
    let prep = tokio::time::timeout(
        DECISION_TIMEOUT,
        client.prepare_upload(
            t.protocol,
            &t.host,
            t.port,
            None,
            payload,
            None,
            cancel.clone(),
        ),
    )
    .await;
    let prep = match prep {
        Err(_) => return Err(MSG_TIMEOUT.into()),
        Ok(Err(ClientError::Cancelled)) => return Err(cancelled()),
        Ok(Err(e)) => return Err(vn_error(&e)),
        Ok(Ok(p)) => p,
    };
    let Some(resp) = prep.response else {
        return Err("Máy kia không nhận file nào".into());
    };
    if resp.session_id.is_empty() || resp.files.keys().any(|k| !by_id.contains_key(k)) {
        return Err("Máy kia trả lời không hợp lệ".into());
    }
    *lock(remote_sid) = Some(resp.session_id.clone());

    let mut queue: Vec<(String, String, OutFile)> = resp
        .files
        .iter()
        .filter_map(|(fid, tok)| by_id.remove(fid).map(|f| (fid.clone(), tok.clone(), f)))
        .collect();
    queue.sort_by(|a, b| a.2.name.cmp(&b.2.name));
    let files_total = queue.len() as u32;
    let bytes_total: u64 = queue.iter().map(|q| q.2.size).sum();
    let queue = Mutex::new(VecDeque::from(queue));
    let sent = Arc::new(AtomicU64::new(0));
    let files_done = Arc::new(AtomicU32::new(0));
    // Not a child of `cancel`: on a user cancel the receiver is told first, then the
    // uploads are aborted, so it sees a cancel rather than failed files.
    let abort = CancellationToken::new();
    let first_err: Mutex<Option<String>> = Mutex::new(None);

    // Progress, at most 4 events per second.
    let fin = CancellationToken::new();
    {
        let (me, id, sent, files_done, fin) = (
            inner.clone(),
            id.to_string(),
            sent.clone(),
            files_done.clone(),
            fin.clone(),
        );
        inner.rt.spawn(async move {
            let mut speed = Speed::new();
            let mut last = u64::MAX;
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                    _ = fin.cancelled() => return,
                }
                let b = sent.load(Ordering::Relaxed).min(bytes_total);
                let bps = speed.sample(b);
                if b != last {
                    last = b;
                    me.emit(LsEvent::Progress {
                        id: id.clone(),
                        bytes_done: b,
                        bytes_total,
                        files_done: files_done.load(Ordering::Relaxed),
                        files_total,
                        speed_bps: bps,
                    });
                }
            }
        });
    }

    let ctx = UploadCtx {
        client: &client,
        t,
        session_id: &resp.session_id,
        queue: &queue,
        sent: &sent,
        files_done: &files_done,
        abort: &abort,
        first_err: &first_err,
    };
    let uploads = futures_util::future::join_all((0..PARALLEL).map(|_| upload_worker(&ctx)));
    tokio::pin!(uploads);
    let mut told_peer = by_peer.load(Ordering::Relaxed);
    let tell_peer = || async {
        let _ = tokio::time::timeout(
            Duration::from_secs(3),
            client.cancel(t.protocol, &t.host, t.port, &resp.session_id),
        )
        .await;
    };
    tokio::select! {
        _ = &mut uploads => {}
        _ = cancel.cancelled() => {
            if !by_peer.load(Ordering::Relaxed) {
                tell_peer().await;
                told_peer = true;
            }
            abort.cancel();
            uploads.await;
        }
    }
    fin.cancel();

    let err = lock(&first_err).take();
    if cancel.is_cancelled() {
        return Err(cancelled());
    }
    if let Some(err) = err {
        if !told_peer {
            tell_peer().await;
        }
        return Err(err);
    }
    inner.emit(LsEvent::Progress {
        id: id.into(),
        bytes_done: bytes_total,
        bytes_total,
        files_done: files_total,
        files_total,
        speed_bps: 0,
    });
    Ok((files_total, bytes_total))
}

struct UploadCtx<'a> {
    client: &'a LsHttpClientV2,
    t: &'a Target,
    session_id: &'a str,
    queue: &'a Mutex<VecDeque<(String, String, OutFile)>>,
    sent: &'a Arc<AtomicU64>,
    files_done: &'a AtomicU32,
    abort: &'a CancellationToken,
    first_err: &'a Mutex<Option<String>>,
}

async fn upload_worker(c: &UploadCtx<'_>) {
    loop {
        if c.abort.is_cancelled() {
            return;
        }
        let Some((fid, tok, f)) = lock(c.queue).pop_front() else {
            return;
        };
        let counter = c.sent.clone();
        let stream = FileContent::Path(f.path.clone())
            .into_stream()
            .map(move |r| {
                if let Ok(b) = &r {
                    counter.fetch_add(b.len() as u64, Ordering::Relaxed);
                }
                r
            });
        let body = localsend::reqwest::Body::wrap_stream(stream);
        let t = c.t;
        let r = c
            .client
            .upload(
                t.protocol,
                &t.host,
                t.port,
                None,
                c.session_id,
                &fid,
                &tok,
                body,
                c.abort.clone(),
            )
            .await;
        match r {
            Ok(()) => {
                c.files_done.fetch_add(1, Ordering::Relaxed);
            }
            Err(ClientError::Cancelled) => return,
            Err(e) => {
                {
                    let mut fe = lock(c.first_err);
                    if fe.is_none() {
                        *fe = Some(match &e {
                            ClientError::StatusCode(s) if s.status == 403 => {
                                "Máy kia đã dừng nhận".to_string()
                            }
                            e => vn_error(e),
                        });
                    }
                }
                c.abort.cancel();
                return;
            }
        }
    }
}

pub(crate) async fn send_text(inner: &Arc<Inner>, fingerprint: &str, text: &str) -> Result<()> {
    if inner.stopped.is_cancelled() {
        return Err(anyhow!("LocalSend đã tắt"));
    }
    if text.is_empty() {
        return Err(anyhow!("Tin nhắn trống"));
    }
    if text.len() > MAX_TEXT_BYTES {
        return Err(anyhow!(
            "Tin nhắn quá dài (tối đa 64 KB) — hãy gửi dạng file"
        ));
    }
    let t = target_of(inner, fingerprint)?;
    if !reachable(&t.host, t.port).await {
        return Err(anyhow!(MSG_UNREACHABLE));
    }
    let client = client_for(inner, &t).map_err(|e| anyhow!(e))?;
    let fid = uuid::Uuid::new_v4().to_string();
    let file = FileDto {
        id: fid.clone(),
        file_name: format!("{fid}.txt"),
        size: text.len() as u64,
        file_type: "text/plain".into(),
        sha256: None,
        preview: Some(text.to_string()),
        metadata: None,
    };
    let payload = PrepareUploadRequestDtoV2 {
        info: inner.register_dto(),
        files: HashMap::from([(fid.clone(), file)]),
    };
    let cancel = inner.stopped.child_token();
    let prep = tokio::time::timeout(
        Duration::from_secs(120),
        client.prepare_upload(
            t.protocol,
            &t.host,
            t.port,
            None,
            payload,
            None,
            cancel.clone(),
        ),
    )
    .await
    .map_err(|_| anyhow!(MSG_TIMEOUT))?
    .map_err(|e| anyhow!(vn_error(&e)))?;
    // 204: the receiver read the preview, nothing left to upload (the normal case).
    let Some(resp) = prep.response else {
        return Ok(());
    };
    if let Some(tok) = resp.files.get(&fid) {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let _ = tx.try_send(bytes::Bytes::from(text.to_string()));
        drop(tx);
        let body = localsend::reqwest::Body::wrap_stream(FileContent::Stream(rx).into_stream());
        client
            .upload(
                t.protocol,
                &t.host,
                t.port,
                None,
                &resp.session_id,
                &fid,
                tok,
                body,
                cancel,
            )
            .await
            .map_err(|e| anyhow!(vn_error(&e)))?;
    }
    Ok(())
}
