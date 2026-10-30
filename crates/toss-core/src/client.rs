//! Client side of `toss/xfer/1`: get/put sessions, the retry policy
//! (Busy: every 30 s up to 1 h; HostOffline: every 30 s up to 24 h),
//! pause/resume/cancel.

use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use iroh::endpoint::Connection;
use tokio::sync::watch;

use crate::{
    fsutil::{part_path, walk, HashCache, HASH_THREADS},
    node::{ClientJob, Ctrl, EntryKind, Inner, Running, Target},
    paths::{canonicalize, check_inside, sanitize_component, sanitize_rel},
    resume::{Kind, TransferRecord},
    ticket::Ticket,
    transfer::{conn_path, ConnPath, ErrorCode, Side, Status, Tracker, Transfer},
    util::{clean_text, now_unix, random_suffix},
    vi::ui,
    wire::{
        read_msg, read_msg_max, write_msg, ClientHello, DenyReason, Done, FileEntry, HostHello,
        Manifest, Op, PeerReq, PeerResp, Want, WantItem, MAX_HELLO,
    },
    xfer::{self, net, timed, Pipe, RecvItem, SendItem, SessErr},
    ALPN_XFER, ALPN_XFER2,
};

/// Seconds between retries for Busy / HostOffline.
pub const RETRY_SECS: u64 = 30;
/// Give up on Busy after this long.
pub const BUSY_LIMIT: Duration = Duration::from_secs(3600);
/// Give up on HostOffline after this long.
pub const OFFLINE_LIMIT: Duration = Duration::from_secs(24 * 3600);
/// Streams of an xfer/1 put (old hosts).
const CLIENT_STREAMS: usize = 4;

/// Test hook: endpoints (by id) whose client sessions dial plain `toss/xfer/1`
/// like a v0.3 client.
static XFER1_ONLY: std::sync::LazyLock<parking_lot::Mutex<std::collections::HashSet<[u8; 32]>>> =
    std::sync::LazyLock::new(Default::default);

/// Makes the client sessions of the node with endpoint `id` speak only
/// `toss/xfer/1` (tests of the v0.3 compatibility path).
pub fn force_xfer1_client(id: iroh::EndpointId, on: bool) {
    let mut set = XFER1_ONLY.lock();
    if on {
        set.insert(*id.as_bytes());
    } else {
        set.remove(id.as_bytes());
    }
}

/// Creates and starts a client transfer.
pub(crate) async fn start(
    inner: &Arc<Inner>,
    ticket: Ticket,
    op: Op,
    dest: Option<PathBuf>,
    paths: Vec<PathBuf>,
) -> Result<Transfer> {
    if ticket.body.admin {
        return Err(ui(
            "Đây là link điều khiển máy chủ, không dùng để chuyển file",
        ));
    }
    match op {
        Op::Get if !ticket.body.get => return Err(ui("Link này không cho phép tải về")),
        Op::Put if !ticket.body.put => return Err(ui("Link này không cho phép gửi file lên")),
        _ => {}
    }
    ticket.verify_signature().map_err(|e| ui(e.to_string()))?;
    let host = ticket.host().map_err(|e| ui(e.to_string()))?;
    if host == inner.endpoint.id() {
        return Err(ui("Link này do chính máy này tạo"));
    }
    let job_dir = match (op, dest) {
        (Op::Get, Some(dest)) => {
            let fallback = format!("toss-{}", &ticket.id_hex()[..8]);
            let dir = dest.join(sanitize_component(&ticket.body.job, &fallback));
            tokio::fs::create_dir_all(&dir).await.map_err(|e| {
                ui(format!(
                    "Không tạo được thư mục {}: {}",
                    dir.display(),
                    crate::vi::io_msg(&e.into())
                ))
            })?;
            Some(dir)
        }
        (Op::Get, None) => return Err(ui("Chưa chọn thư mục để lưu")),
        (Op::Put, _) => {
            check_paths(&paths).await?;
            None
        }
    };
    let id = format!("{}-{}", ticket.id_hex(), random_suffix());
    let tracker = Tracker::new(
        id.clone(),
        ticket.body.job.clone(),
        op,
        Side::Client,
        host.fmt_short().to_string(),
        job_dir.as_ref().map(|d| d.display().to_string()),
        inner.events.clone(),
    );
    let rec = TransferRecord {
        id: id.clone(),
        kind: match op {
            Op::Get => Kind::Get,
            Op::Put => Kind::Put,
        },
        ticket: Some(ticket.to_string()),
        contact: None,
        paths: (op == Op::Put).then(|| paths.clone()),
        dest: job_dir
            .as_ref()
            .and_then(|d| d.parent().map(Path::to_path_buf)),
        job: ticket.body.job.clone(),
        status: Status::Connecting,
        bytes_done: 0,
        bytes_total: 0,
        created: now_unix(),
        push_id: None,
        peer: host.fmt_short().to_string(),
        files_total: 0,
        files_done: 0,
        error: None,
        stranger: false,
    };
    let job = Arc::new(ClientJob {
        target: Target::Ticket(ticket),
        op,
        job_dir,
        paths,
        part_files: Default::default(),
        running: tokio::sync::Mutex::new(None),
    });
    // Two gets of the same job into the same folder would write the same
    // `.part` files (and a cancel of one would delete the other's): a get
    // that is already in progress is returned instead of starting a second.
    let existing = {
        let mut map = inner.transfers.lock();
        let found = map.values().find_map(|e| match &e.kind {
            EntryKind::Client(j)
                if j.op == Op::Get
                    && j.ticket().map(|t| t.body.id) == job.ticket().map(|t| t.body.id)
                    && j.job_dir == job.job_dir
                    && !e.tracker.status().is_final() =>
            {
                Some((j.clone(), e.tracker.clone()))
            }
            _ => None,
        });
        if found.is_none() {
            let seq = inner.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            map.insert(
                id.clone(),
                crate::node::Entry {
                    seq,
                    tracker: tracker.clone(),
                    kind: EntryKind::Client(job.clone()),
                },
            );
        }
        found
    };
    if let Some((j, t)) = existing {
        if t.status() == Status::Paused {
            resume(inner, j, t.clone()).await?;
        }
        return Ok(Transfer {
            id: t.id.clone(),
            rx: t.subscribe_state(),
        });
    }
    crate::resume::track(&inner.store, rec, &tracker);
    tracker.started();
    let rx = tracker.subscribe_state();
    {
        let mut running = job.running.lock().await;
        spawn_locked(&mut running, inner, job.clone(), tracker);
    }
    Ok(Transfer { id, rx })
}

/// Every selected path must exist (else a Vietnamese error naming it).
async fn check_paths(paths: &[PathBuf]) -> Result<()> {
    if paths.is_empty() {
        return Err(ui("Chưa chọn file nào để gửi"));
    }
    for p in paths {
        if tokio::fs::symlink_metadata(p).await.is_err() {
            return Err(ui(format!(
                "Không đọc được {} (đã bị xoá hoặc đổi tên?)",
                p.display()
            )));
        }
    }
    Ok(())
}

/// v0.5: creates and starts a push to contact `contact_id` (hex).
pub(crate) async fn start_push(
    inner: &Arc<Inner>,
    contact_id: &str,
    paths: Vec<PathBuf>,
) -> Result<Transfer> {
    let contact_id = contact_id.trim().to_ascii_lowercase();
    let contact = inner
        .contacts
        .get(&contact_id)
        .ok_or_else(|| ui("Không có máy này trong danh bạ"))?
        .c;
    start_push_to(inner, &contact.id, &contact.name, false, paths).await
}

/// v0.8 `Node::send_to_peer`: a contact (as [`start_push`]) or a device in
/// the nearby store (a stranger push, `PushFrom`).
pub(crate) async fn start_push_peer(
    inner: &Arc<Inner>,
    peer_id: &str,
    paths: Vec<PathBuf>,
) -> Result<Transfer> {
    let hex = peer_id.trim().to_ascii_lowercase();
    if inner.contacts.contains(&hex) {
        return start_push(inner, &hex, paths).await;
    }
    let peer = inner.nearby.get(&hex).ok_or_else(|| {
        ui("Không thấy máy này trong mạng LAN nữa — kiểm tra máy kia còn mở Toss và cùng mạng")
    })?;
    let id = crate::peer::parse_endpoint_id(&hex)?;
    if id == inner.endpoint.id() {
        return Err(ui("Không thể gửi cho chính máy này"));
    }
    let name = crate::peer::nearby_name(&peer);
    start_push_to(inner, &hex, &name, true, paths).await
}

async fn start_push_to(
    inner: &Arc<Inner>,
    hex: &str,
    name: &str,
    stranger: bool,
    paths: Vec<PathBuf>,
) -> Result<Transfer> {
    let id = crate::peer::parse_endpoint_id(hex)?;
    check_paths(&paths).await?;
    let job_name = push_job_name(&paths);
    let push_id = crate::util::random_id();
    let rec = TransferRecord {
        id: format!("{}-{}", hex::encode(push_id), random_suffix()),
        kind: Kind::Push,
        ticket: None,
        contact: Some(hex.to_string()),
        paths: Some(paths.clone()),
        dest: None,
        job: job_name.clone(),
        status: Status::Connecting,
        bytes_done: 0,
        bytes_total: 0,
        created: now_unix(),
        push_id: Some(hex::encode(push_id)),
        peer: name.to_string(),
        files_total: 0,
        files_done: 0,
        error: None,
        stranger,
    };
    let job = Arc::new(ClientJob {
        target: Target::Contact {
            id,
            push_id,
            job: job_name,
            stranger,
        },
        op: Op::Put,
        job_dir: None,
        paths,
        part_files: Default::default(),
        running: tokio::sync::Mutex::new(None),
    });
    let tracker = launch(inner, job, rec, true);
    Ok(Transfer {
        id: tracker.id.clone(),
        rx: tracker.subscribe_state(),
    })
}

/// Job name of a push: the dropped folder's name (one folder), else "N file".
fn push_job_name(paths: &[PathBuf]) -> String {
    if let [p] = paths {
        if p.is_dir() {
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                return name.chars().take(crate::ticket::MAX_JOB_LEN).collect();
            }
        }
    }
    format!("{} file", paths.len())
}

/// Registers a client job (tracker with the record's id and state, persisted
/// via `transfers.json`) and starts its worker when `run`. `job` is new (its
/// `running` lock is free).
fn launch(inner: &Arc<Inner>, job: Arc<ClientJob>, rec: TransferRecord, run: bool) -> Arc<Tracker> {
    let tracker = Tracker::new(
        rec.id.clone(),
        rec.job.clone(),
        job.op,
        Side::Client,
        rec.peer.clone(),
        job.job_dir.as_ref().map(|d| d.display().to_string()),
        inner.events.clone(),
    );
    tracker.update(|s| {
        s.contact = rec.contact.clone();
        s.status = rec.status;
        s.bytes_done = rec.bytes_done;
        s.bytes_total = rec.bytes_total;
        s.files_done = rec.files_done;
        s.files_total = rec.files_total;
        s.error = rec.error.clone();
        s.started_at = rec.created;
    });
    tracker
        .bytes_done
        .store(rec.bytes_done, std::sync::atomic::Ordering::Relaxed);
    inner.register(tracker.clone(), EntryKind::Client(job.clone()));
    crate::resume::track(&inner.store, rec, &tracker);
    if run {
        tracker.started();
        match job.running.try_lock() {
            Ok(mut running) => spawn_locked(&mut running, inner, job.clone(), tracker.clone()),
            Err(_) => tracing::error!(id = %tracker.id, "new transfer already locked"),
        }
    }
    tracker
}

/// v0.5: restores the transfers of `transfers.json` (called by `Node::start`):
/// running/connecting ones resume, paused/failed ones wait for `Node::resume`,
/// finished ones stay in the list. Registered before `Node::start` returns,
/// so a new get of the same job finds (and reuses) the restored one.
pub(crate) fn restore_all(inner: &Arc<Inner>) {
    let records = inner.store.records();
    if records.is_empty() {
        return;
    }
    let mut resumed = 0;
    for mut rec in records {
        let id = rec.id.clone();
        match restore_job(inner, &rec) {
            Ok(job) => {
                let run = matches!(rec.status, Status::Running | Status::Connecting);
                if run {
                    rec.status = Status::Connecting;
                    resumed += 1;
                }
                launch(inner, job, rec, run);
            }
            Err(e) => {
                tracing::info!(%id, "dropping saved transfer: {e:#}");
                inner.store.remove(&[id]);
            }
        }
    }
    tracing::info!(resumed, "restored saved transfers");
}

fn restore_job(inner: &Inner, rec: &TransferRecord) -> Result<Arc<ClientJob>> {
    let (target, op, job_dir) = match rec.kind {
        Kind::Get | Kind::Put => {
            let t = Ticket::from_str(rec.ticket.as_deref().unwrap_or_default())
                .map_err(|e| anyhow!(e))?;
            if rec.kind == Kind::Get {
                let dest = rec.dest.clone().ok_or_else(|| anyhow!("no destination"))?;
                let fallback = format!("toss-{}", &t.id_hex()[..8]);
                let dir = dest.join(sanitize_component(&t.body.job, &fallback));
                (Target::Ticket(t), Op::Get, Some(dir))
            } else {
                (Target::Ticket(t), Op::Put, None)
            }
        }
        Kind::Push => {
            let cid = rec.contact.as_deref().unwrap_or_default();
            if !rec.stranger && !inner.contacts.contains(cid) {
                bail!("contact {cid} is gone");
            }
            let push_id: [u8; 16] = hex::decode(rec.push_id.as_deref().unwrap_or_default())
                .ok()
                .and_then(|v| v.try_into().ok())
                .ok_or_else(|| anyhow!("bad push id"))?;
            let target = Target::Contact {
                id: crate::peer::parse_endpoint_id(cid)?,
                push_id,
                job: rec.job.clone(),
                stranger: rec.stranger,
            };
            (target, Op::Put, None)
        }
    };
    Ok(Arc::new(ClientJob {
        target,
        op,
        job_dir,
        paths: rec.paths.clone().unwrap_or_default(),
        part_files: Default::default(),
        running: tokio::sync::Mutex::new(None),
    }))
}

/// Starts the worker task; the caller holds `job.running`'s lock, so checking
/// for a running task and starting one is atomic (no two workers per job).
fn spawn_locked(
    running: &mut Option<Running>,
    inner: &Arc<Inner>,
    job: Arc<ClientJob>,
    tracker: Arc<Tracker>,
) {
    let (ctrl_tx, ctrl_rx) = watch::channel(Ctrl::Run);
    tracker.set_status(Status::Connecting);
    let handle = tokio::spawn(run(inner.clone(), job, tracker, ctrl_rx));
    *running = Some(Running {
        ctrl: ctrl_tx,
        handle,
    });
}

/// Pauses a running client transfer.
pub(crate) async fn pause(job: Arc<ClientJob>) -> Result<()> {
    let mut running = job.running.lock().await;
    if let Some(r) = running.take() {
        let _ = r.ctrl.send(Ctrl::Pause);
        let _ = r.handle.await;
    }
    Ok(())
}

/// Resumes a paused (or failed) client transfer.
pub(crate) async fn resume(
    inner: &Arc<Inner>,
    job: Arc<ClientJob>,
    tracker: Arc<Tracker>,
) -> Result<()> {
    let mut running = job.running.lock().await;
    if running.as_ref().is_some_and(|r| !r.handle.is_finished()) {
        return Ok(());
    }
    match tracker.status() {
        Status::Done => return Err(ui("Lượt chuyển này đã xong")),
        Status::Cancelled => return Err(ui("Lượt chuyển này đã bị huỷ")),
        _ => {}
    }
    spawn_locked(&mut running, inner, job.clone(), tracker);
    Ok(())
}

/// Cancels a client transfer and deletes its `.part` files.
pub(crate) async fn cancel(job: Arc<ClientJob>, tracker: Arc<Tracker>) -> Result<()> {
    let mut running = job.running.lock().await;
    if tracker.status() == Status::Cancelled {
        return Ok(());
    }
    let was_running = running.as_ref().is_some_and(|r| !r.handle.is_finished());
    if let Some(r) = running.take() {
        let _ = r.ctrl.send(Ctrl::Cancel);
        let _ = r.handle.await;
    }
    if !was_running {
        if tracker.status() == Status::Done {
            return Err(ui("Lượt chuyển này đã xong"));
        }
        delete_parts(&job).await;
        tracker.error(ErrorCode::Cancelled, crate::vi::CANCELLED.into(), None);
    }
    Ok(())
}

async fn delete_parts(job: &ClientJob) {
    let parts: Vec<PathBuf> = job.part_files.lock().iter().cloned().collect();
    for p in parts {
        // The aborted receive tasks may still hold the file open for a moment;
        // Windows refuses to delete an open file, so retry briefly.
        for attempt in 0..20 {
            match tokio::fs::remove_file(&p).await {
                Ok(()) => break,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                Err(e) if attempt == 19 => {
                    tracing::warn!("could not delete {}: {e}", p.display());
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    }
}

/// Makes sure a worker that ends without reaching a resting state (e.g. a
/// panic) leaves the transfer in `Error`, so `Transfer::wait` never hangs.
struct EndGuard(Arc<Tracker>);

impl Drop for EndGuard {
    fn drop(&mut self) {
        let st = self.0.status();
        if !st.is_final() && st != Status::Paused {
            self.0.error(
                ErrorCode::Other,
                "Lượt chuyển dừng bất thường — bấm Tiếp tục để thử lại".into(),
                None,
            );
        }
    }
}

async fn run(
    inner: Arc<Inner>,
    job: Arc<ClientJob>,
    tracker: Arc<Tracker>,
    mut ctrl: watch::Receiver<Ctrl>,
) {
    let _guard = EndGuard(tracker.clone());
    let drive_fut = drive(&inner, &job, &tracker);
    tokio::select! {
        res = drive_fut => match res {
            Ok(path) => tracker.finish_done(path),
            Err((code, msg)) => {
                // Final failures (e.g. "No space left on device") must be
                // visible in the log, not only in the UI.
                tracing::warn!(id = %tracker.id, ?code, "transfer failed: {msg}");
                tracker.error(code, msg, None)
            }
        },
        c = async {
            loop {
                if ctrl.changed().await.is_err() { return Ctrl::Pause; }
                let c = *ctrl.borrow();
                if c != Ctrl::Run { return c; }
            }
        } => {
            match c {
                Ctrl::Cancel => {
                    delete_parts(&job).await;
                    tracker.error(ErrorCode::Cancelled, crate::vi::CANCELLED.into(), None);
                }
                _ => tracker.paused(),
            }
        }
    }
}

enum SessionEnd {
    Complete(ConnPath),
    Mismatch(String),
}

fn deny_code(r: &DenyReason) -> ErrorCode {
    match r {
        DenyReason::Expired => ErrorCode::Expired,
        DenyReason::Revoked => ErrorCode::Revoked,
        DenyReason::Busy => ErrorCode::Busy,
        _ => ErrorCode::Denied,
    }
}

/// Runs sessions until success or a final error, applying the retry policy.
async fn drive(
    inner: &Arc<Inner>,
    job: &Arc<ClientJob>,
    tracker: &Arc<Tracker>,
) -> Result<ConnPath, (ErrorCode, String)> {
    if let Target::Contact { id, stranger, .. } = &job.target {
        return drive_push(inner, job, tracker, *id, *stranger).await;
    }
    let mut since = Instant::now();
    let mut hash_retry_used = false;
    loop {
        let res = match job.op {
            Op::Get => get_session(inner, job, tracker).await,
            Op::Put => put_session(inner, job, tracker).await,
        };
        let (code, msg, limit) = match res {
            Ok(SessionEnd::Complete(p)) => return Ok(p),
            Ok(SessionEnd::Mismatch(msg)) => {
                if hash_retry_used {
                    tracing::warn!(id = %tracker.id, "{msg}");
                    return Err((ErrorCode::HashMismatch, crate::vi::DONE_FAILED.into()));
                }
                tracing::warn!("{msg}; retrying once on a fresh connection");
                hash_retry_used = true;
                continue;
            }
            Err(SessErr::Denied(DenyReason::Busy)) => (
                ErrorCode::Busy,
                format!("Máy chia sẻ đang bận — tự thử lại sau {RETRY_SECS} giây"),
                BUSY_LIMIT,
            ),
            Err(SessErr::Denied(r)) => return Err((deny_code(&r), r.to_string())),
            Err(e @ SessErr::Offline { .. }) => {
                let SessErr::Offline { connected, err } = &e else {
                    unreachable!()
                };
                if *connected {
                    since = Instant::now();
                }
                tracing::info!(id = %tracker.id, "session failed: {err:#}");
                let msg = format!(
                    "{} — tự thử lại sau {RETRY_SECS} giây",
                    crate::vi::sess_msg(&e)
                );
                (ErrorCode::HostOffline, msg, OFFLINE_LIMIT)
            }
            Err(e @ SessErr::Io(_)) => {
                tracing::warn!(id = %tracker.id, "session failed: {e}");
                return Err((ErrorCode::Io, crate::vi::sess_msg(&e)));
            }
            Err(e @ SessErr::Other(_)) => {
                tracing::warn!(id = %tracker.id, "session failed: {e}");
                return Err((ErrorCode::Other, crate::vi::sess_msg(&e)));
            }
        };
        if since.elapsed() >= limit {
            return Err((code, msg));
        }
        tracing::info!(id = %tracker.id, ?code, "{msg}; retrying in {RETRY_SECS}s");
        tracker.error(code, msg, Some(RETRY_SECS));
        tokio::time::sleep(Duration::from_secs(RETRY_SECS)).await;
    }
}

/// Dials the host and exchanges hellos.
async fn open(
    inner: &Arc<Inner>,
    job: &ClientJob,
    tracker: &Tracker,
) -> Result<
    (
        Connection,
        iroh::endpoint::SendStream,
        iroh::endpoint::RecvStream,
    ),
    SessErr,
> {
    tracker.set_status(Status::Connecting);
    let ticket = job
        .ticket()
        .ok_or_else(|| SessErr::Other(anyhow!("not a ticket transfer")))?;
    let conn = if XFER1_ONLY.lock().contains(inner.endpoint.id().as_bytes()) {
        inner.dial_host(ticket, ALPN_XFER).await
    } else {
        // v0.4: offer xfer/2, fall back to xfer/1 with a v0.3 host.
        inner
            .dial_host_alpns(ticket, ALPN_XFER2, vec![ALPN_XFER.to_vec()])
            .await
    }
    .map_err(net(false))?;
    let (mut send, mut recv) = conn.open_bi().await.map_err(|e| net(false)(e.into()))?;
    let hello = ClientHello::v1 {
        ticket: ticket.clone(),
        device: inner.device_name().to_string(),
        op: job.op,
        app_version: inner.cfg.app_version.clone(),
    };
    write_msg(&mut send, &hello).await.map_err(net(false))?;
    let answer: HostHello = read_msg(&mut recv).await.map_err(net(false))?;
    match answer {
        HostHello::Ok { .. } => Ok((conn, send, recv)),
        HostHello::Denied { reason } => {
            conn.close(0u32.into(), b"denied");
            Err(SessErr::Denied(reason))
        }
    }
}

/// Closes the ticker when dropped.
struct Abort(tokio::task::JoinHandle<()>);
impl Drop for Abort {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn get_session(
    inner: &Arc<Inner>,
    job: &Arc<ClientJob>,
    tracker: &Arc<Tracker>,
) -> Result<SessionEnd, SessErr> {
    let job_dir = job
        .job_dir
        .clone()
        .ok_or_else(|| SessErr::Other(anyhow!("no destination")))?;
    let (conn, mut send, mut recv) = open(inner, job, tracker).await?;
    let manifest: Manifest = read_msg(&mut recv).await.map_err(net(true))?;
    xfer::check_manifest(&manifest).map_err(SessErr::Other)?;

    let hashes = inner.hashes.clone();
    let m2 = manifest.clone();
    let plan = tokio::task::spawn_blocking(move || plan_get(&job_dir, &m2, &hashes))
        .await
        .map_err(|e| SessErr::Other(e.into()))?
        .map_err(SessErr::Io)?;
    job.part_files
        .lock()
        .extend(plan.items.values().map(|i| i.part.clone()));
    tracker.begin_session(
        manifest.files.len() as u32,
        manifest.total,
        plan.want.items.len() as u32,
        plan.to_do_bytes,
    );
    write_msg(&mut send, &plan.want).await.map_err(net(true))?;

    let _ticker = Abort(tracker.spawn_ticker(conn.clone()));
    let summary = if conn.alpn() == ALPN_XFER2 {
        let pipe = Pipe::new(&inner.cfg.net.tuning, None);
        xfer::receive_files_v2(
            &conn,
            &plan.base_c,
            plan.items,
            &pipe,
            tracker,
            &inner.hashes,
        )
        .await?
    } else {
        xfer::receive_files(&conn, &plan.base_c, plan.items, tracker, &inner.hashes).await?
    };

    let mut notes = Vec::new();
    if !plan.skipped.is_empty() {
        notes.push(format!(
            "skipped (invalid name): {}",
            plan.skipped.join("; ")
        ));
    }
    if !summary.mismatched.is_empty() {
        notes.push(format!("hash mismatch: {}", summary.mismatched.join("; ")));
    }
    let ok = notes.is_empty();
    let done = Done {
        files: summary.files,
        bytes: summary.bytes,
        ok,
        msg: notes.join(" | "),
    };
    let path = conn_path(&conn);
    let _ = write_msg(&mut send, &done).await;
    let _ = send.finish();
    // The host closes after reading Done; don't wait forever.
    let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
    conn.close(0u32.into(), b"done");

    if !summary.mismatched.is_empty() {
        return Ok(SessionEnd::Mismatch(done.msg));
    }
    if !plan.skipped.is_empty() {
        tracing::warn!("get: {}", done.msg);
        return Err(SessErr::Other(ui(format!(
            "Bỏ qua file có tên không dùng được trên máy này: {}",
            plan.skipped.join("; ")
        ))));
    }
    Ok(SessionEnd::Complete(path))
}

struct GetPlan {
    base_c: PathBuf,
    want: Want,
    items: HashMap<u32, RecvItem>,
    to_do_bytes: u64,
    skipped: Vec<String>,
}

fn plan_get(job_dir: &Path, manifest: &Manifest, hashes: &HashCache) -> Result<GetPlan> {
    std::fs::create_dir_all(job_dir).with_context(|| format!("create {}", job_dir.display()))?;
    let base_c = canonicalize(job_dir)?;
    let mut plan = GetPlan {
        base_c: base_c.clone(),
        want: Want::default(),
        items: HashMap::new(),
        to_do_bytes: 0,
        skipped: Vec::new(),
    };
    let mut seen = BTreeSet::new();
    for (i, e) in manifest.files.iter().enumerate() {
        let index = u32::try_from(i).context("too many files")?;
        let rel = match sanitize_rel(&e.rel) {
            Ok(r) if !r.is_empty() && seen.insert(r.as_str().to_lowercase()) => r,
            _ => {
                plan.skipped.push(e.rel.clone());
                continue;
            }
        };
        let final_path = base_c.join(rel.to_path());
        let part = part_path(&final_path);
        if check_inside(&base_c, &final_path).is_err() || check_inside(&base_c, &part).is_err() {
            plan.skipped.push(e.rel.clone());
            continue;
        }
        if let Ok(m) = std::fs::symlink_metadata(&final_path) {
            if m.is_file() && m.len() == e.size && hashes.hash(&final_path)? == e.hash {
                continue;
            }
        }
        let Some(offset) = xfer::resume_offset(&final_path, &part, e, hashes)? else {
            continue;
        };
        plan.to_do_bytes = plan.to_do_bytes.saturating_add(e.size - offset);
        plan.want.items.push(WantItem { index, offset });
        plan.items.insert(
            index,
            RecvItem {
                entry: e.clone(),
                final_path,
                part,
                offset,
                label: rel.as_str(),
                no_clobber: false,
            },
        );
    }
    Ok(plan)
}

async fn put_session(
    inner: &Arc<Inner>,
    job: &Arc<ClientJob>,
    tracker: &Arc<Tracker>,
) -> Result<SessionEnd, SessErr> {
    // Build (and hash) the manifest before dialing so the host slot is not held while hashing.
    let paths = job.paths.clone();
    let hashes = inner.hashes.clone();
    let (manifest, abs) =
        tokio::task::spawn_blocking(move || build_put_manifest(&paths, &hashes, false))
            .await
            .map_err(|e| SessErr::Other(e.into()))?
            .map_err(SessErr::Io)?;

    let (conn, mut send, mut recv) = open(inner, job, tracker).await?;
    let v2 = conn.alpn() == ALPN_XFER2;
    put_data_phase(
        inner, &conn, &mut send, &mut recv, &manifest, &abs, tracker, v2,
    )
    .await
}

/// Sender side of xfer put semantics after the hello (ticket put) or the
/// accepted `Push` (contacts): `Manifest` → `Want` → data → `Done`.
#[allow(clippy::too_many_arguments)]
async fn put_data_phase(
    inner: &Arc<Inner>,
    conn: &Connection,
    send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream,
    manifest: &Manifest,
    abs: &[PathBuf],
    tracker: &Arc<Tracker>,
    v2: bool,
) -> Result<SessionEnd, SessErr> {
    write_msg(send, manifest).await.map_err(net(true))?;
    let want: Want = read_msg(recv).await.map_err(net(true))?;
    let mut items = Vec::with_capacity(want.items.len());
    let mut to_do = 0u64;
    let mut seen = BTreeSet::new();
    for w in &want.items {
        let f: &FileEntry = manifest
            .files
            .get(w.index as usize)
            .ok_or_else(|| SessErr::Other(anyhow!("host wants unknown file #{}", w.index)))?;
        if !seen.insert(w.index) {
            return Err(SessErr::Other(anyhow!("host wants {} twice", f.rel)));
        }
        if w.offset > f.size {
            return Err(SessErr::Other(anyhow!(
                "host wants bad offset for {}",
                f.rel
            )));
        }
        to_do += f.size - w.offset;
        items.push(SendItem {
            index: w.index,
            offset: w.offset,
            abs: abs[w.index as usize].clone(),
            size: f.size,
        });
    }
    tracker.begin_session(
        manifest.files.len() as u32,
        manifest.total,
        want.items.len() as u32,
        to_do,
    );
    let _ticker = Abort(tracker.spawn_ticker(conn.clone()));
    if v2 {
        let pipe = Pipe::new(&inner.cfg.net.tuning, None);
        xfer::send_files_v2(conn, items, &pipe, tracker).await?;
    } else {
        xfer::send_files(conn, items, CLIENT_STREAMS, tracker).await?;
    }
    let done: Done = timed("waiting for Done", xfer::stall_timeout(), read_msg(recv)).await?;
    let path = conn_path(conn);
    let _ = send.finish();
    conn.close(0u32.into(), b"done");
    if !done.msg.is_empty() {
        tracing::info!(id = %tracker.id, "host: {}", done.msg);
    }
    if !done.ok {
        if done.msg.contains("hash mismatch") {
            return Ok(SessionEnd::Mismatch(done.msg));
        }
        tracing::warn!(id = %tracker.id, "receiver: {}", done.msg);
        return Err(SessErr::Other(ui(format!(
            "Máy nhận báo lỗi: {}",
            clean_text(&done.msg, 300)
        ))));
    }
    Ok(SessionEnd::Complete(path))
}

/// v0.5 push to a contact: waits until presence sees the contact online, then
/// runs push sessions. A contact that cannot be reached is waited for again
/// (no polling beyond the 60 s presence ping); a decline is final.
async fn drive_push(
    inner: &Arc<Inner>,
    job: &Arc<ClientJob>,
    tracker: &Arc<Tracker>,
    id: iroh::EndpointId,
    stranger: bool,
) -> Result<ConnPath, (ErrorCode, String)> {
    let mut hash_retry_used = false;
    loop {
        crate::peer::wait_reachable(inner, id, tracker, stranger)
            .await
            .map_err(|e| (ErrorCode::Other, e))?;
        let (code, msg) = match push_session(inner, job, tracker).await {
            Ok(SessionEnd::Complete(p)) => return Ok(p),
            Ok(SessionEnd::Mismatch(msg)) => {
                if hash_retry_used {
                    tracing::warn!(id = %tracker.id, "{msg}");
                    return Err((ErrorCode::HashMismatch, crate::vi::DONE_FAILED.into()));
                }
                tracing::warn!("{msg}; retrying once on a fresh connection");
                hash_retry_used = true;
                continue;
            }
            Err(SessErr::Denied(r)) => return Err((ErrorCode::Denied, r.to_string())),
            Err(e @ SessErr::Offline { .. }) => {
                let SessErr::Offline { connected, err } = &e else {
                    unreachable!()
                };
                tracing::info!(id = %tracker.id, "push: receiver unreachable: {err:#}");
                crate::peer::note_unreachable(inner, id);
                if !connected {
                    // Presence may still say online (another live connection):
                    // don't spin on a contact we cannot dial.
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                (ErrorCode::HostOffline, crate::vi::sess_msg(&e))
            }
            Err(e @ SessErr::Io(_)) => {
                tracing::warn!(id = %tracker.id, "push failed: {e}");
                return Err((ErrorCode::Io, crate::vi::sess_msg(&e)));
            }
            Err(e @ SessErr::Other(_)) => {
                tracing::warn!(id = %tracker.id, "push failed: {e}");
                return Err((ErrorCode::Other, crate::vi::sess_msg(&e)));
            }
        };
        tracker.error(code, msg, Some(crate::peer::PRESENCE_INTERVAL.as_secs()));
    }
}

/// One push: dial the contact, `Push`, wait for `PushAccepted` (the receiver
/// may first answer `PushQueued` and ask its user), then xfer/2 put semantics
/// on the same connection.
async fn push_session(
    inner: &Arc<Inner>,
    job: &Arc<ClientJob>,
    tracker: &Arc<Tracker>,
) -> Result<SessionEnd, SessErr> {
    let Target::Contact {
        id,
        push_id,
        job: job_name,
        ..
    } = &job.target
    else {
        return Err(SessErr::Other(anyhow!("not a push")));
    };
    let paths = job.paths.clone();
    let hashes = inner.hashes.clone();
    let (manifest, abs) =
        tokio::task::spawn_blocking(move || build_put_manifest(&paths, &hashes, true))
            .await
            .map_err(|e| SessErr::Other(e.into()))?
            .map_err(SessErr::Io)?;

    tracker.set_status(Status::Connecting);
    let hex = crate::peer::id_hex(*id);
    // A contact gets `Push` (0.6 contacts understand it); anyone else is a
    // nearby device and gets `PushFrom` with our name (v0.8).
    let contact = inner.contacts.contains(&hex);
    let conn = crate::peer::dial_peer(inner, *id)
        .await
        .map_err(net(false))?;
    let _live = contact.then(|| crate::peer::LiveGuard::new(inner, *id));
    let (mut send, mut recv) = conn.open_bi().await.map_err(|e| net(false)(e.into()))?;
    let (files, bytes) = (manifest.files.len() as u32, manifest.total);
    let req = if contact {
        PeerReq::Push {
            push_id: *push_id,
            job: job_name.clone(),
            files,
            bytes,
        }
    } else {
        PeerReq::PushFrom {
            push_id: *push_id,
            job: job_name.clone(),
            files,
            bytes,
            name: inner.device_name().to_string(),
            platform: crate::contacts::platform(),
            version: inner.cfg.app_version.clone(),
        }
    };
    write_msg(&mut send, &req).await.map_err(net(false))?;
    let first = timed(
        "waiting for the push answer",
        Duration::from_secs(60),
        read_msg_max(&mut recv, MAX_HELLO),
    )
    .await;
    let mut resp: PeerResp = match first {
        Ok(r) => r,
        Err(e) if !contact && crate::peer::refused_as_old(&conn).await => {
            tracing::info!(id = %tracker.id, "push: receiver is a Toss < 0.8 ({e})");
            return Err(SessErr::Denied(DenyReason::Other(
                crate::vi::OLD_PEER.into(),
            )));
        }
        Err(e) => return Err(e),
    };
    if resp == PeerResp::PushQueued {
        let name = tracker.snapshot().peer;
        tracing::info!(id = %tracker.id, "push queued: waiting for {name} to accept");
        tracker.update(|s| s.error = Some(format!("Chờ {name} đồng ý nhận")));
        resp = timed(
            "waiting for the receiver to accept",
            crate::peer::PUSH_ANSWER_TIMEOUT + Duration::from_secs(60),
            read_msg_max(&mut recv, MAX_HELLO),
        )
        .await?;
    }
    match resp {
        PeerResp::PushAccepted => {}
        PeerResp::PushAcceptedPaired { name, platform, .. } if !contact => {
            // "Luôn nhận từ máy này" on the receiver: pair back.
            crate::peer::pair_back(inner, &conn, &name, &platform).await;
        }
        PeerResp::PushDeclined { reason } | PeerResp::Denied { reason } => {
            conn.close(0u32.into(), b"declined");
            return Err(SessErr::Denied(DenyReason::Other(clean_text(&reason, 300))));
        }
        other => {
            tracing::info!(id = %tracker.id, "unexpected answer to a push: {other:?}");
            conn.close(1u32.into(), b"protocol");
            return Err(SessErr::Other(ui(crate::vi::PROTOCOL)));
        }
    }
    tracker.update(|s| s.error = None);
    put_data_phase(
        inner, &conn, &mut send, &mut recv, &manifest, &abs, tracker, true,
    )
    .await
}

/// Manifest for a put: each selected file is `name`, each selected folder keeps
/// its name as first segment.
///
/// `push`: a single selected folder is sent as its contents (the receiver puts
/// them in `<contact>/<folder name>/`, the push's job folder).
fn build_put_manifest(
    paths: &[PathBuf],
    hashes: &HashCache,
    push: bool,
) -> Result<(Manifest, Vec<PathBuf>)> {
    let mut walked = Vec::new();
    let mut seen = BTreeSet::new();
    let single_folder = push && paths.len() == 1 && paths[0].is_dir();
    for p in paths {
        let name = p.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
            ui(format!(
                "Không gửi được {}: tên file không dùng được",
                p.display()
            ))
        })?;
        let prefix = if single_folder {
            crate::paths::RelPath::default()
        } else {
            sanitize_rel(name).map_err(|_| {
                ui(format!(
                    "Không gửi được {}: tên file không dùng được",
                    p.display()
                ))
            })?
        };
        for f in walk(p, &prefix)? {
            let rel = f.rel.as_str();
            if !seen.insert(rel.to_lowercase()) {
                tracing::warn!("skipping duplicate {rel}");
                continue;
            }
            walked.push(f);
        }
    }
    let abs: Vec<PathBuf> = walked.iter().map(|f| f.abs.clone()).collect();
    let hashed = hashes.hash_many(&abs, HASH_THREADS);
    let mut manifest = Manifest::default();
    for (f, hash) in walked.into_iter().zip(hashed) {
        manifest.total += f.size;
        manifest.files.push(FileEntry {
            rel: f.rel.as_str(),
            size: f.size,
            hash: hash?,
        });
    }
    Ok((manifest, abs))
}

/// v0.8 `Node::resend`: the same paths to the same target (push or ticket
/// put), or the same download again (ticket get).
pub(crate) async fn resend(inner: &Arc<Inner>, id: &str) -> Result<Transfer> {
    let job = {
        let map = inner.transfers.lock();
        let e = map
            .get(id)
            .ok_or_else(|| ui("Không tìm thấy lượt chuyển này"))?;
        match &e.kind {
            EntryKind::Client(j) => j.clone(),
            EntryKind::Host(_) => {
                return Err(ui("Chỉ gửi lại được lượt chuyển do máy này gửi đi"));
            }
        }
    };
    match &job.target {
        Target::Ticket(t) => {
            if t.is_expired() {
                return Err(ui("Link đã hết hạn nên không gửi lại được — xin link mới"));
            }
            match job.op {
                Op::Put => start(inner, t.clone(), Op::Put, None, job.paths.clone()).await,
                Op::Get => {
                    let dest = job
                        .job_dir
                        .as_ref()
                        .and_then(|d| d.parent())
                        .map(Path::to_path_buf)
                        .ok_or_else(|| ui("Không biết thư mục lưu của lượt tải này"))?;
                    start(inner, t.clone(), Op::Get, Some(dest), Vec::new()).await
                }
            }
        }
        Target::Contact { id, .. } => {
            start_push_peer(inner, &crate::peer::id_hex(*id), job.paths.clone()).await
        }
    }
}
