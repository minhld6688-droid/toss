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
    fsutil::{part_path, walk, HashCache},
    node::{ClientJob, Ctrl, EntryKind, Inner, Running},
    paths::{canonicalize, check_inside, sanitize_component, sanitize_rel},
    ticket::Ticket,
    transfer::{conn_path, ConnPath, ErrorCode, Side, Status, Tracker, Transfer},
    util::random_suffix,
    wire::{
        read_msg, write_msg, ClientHello, DenyReason, Done, FileEntry, HostHello, Manifest, Op,
        Want, WantItem,
    },
    xfer::{self, net, timed, RecvItem, SendItem, SessErr},
    ALPN_XFER,
};

/// Seconds between retries for Busy / HostOffline.
pub const RETRY_SECS: u64 = 30;
/// Give up on Busy after this long.
pub const BUSY_LIMIT: Duration = Duration::from_secs(3600);
/// Give up on HostOffline after this long.
pub const OFFLINE_LIMIT: Duration = Duration::from_secs(24 * 3600);
const CLIENT_STREAMS: usize = 4;

/// Creates and starts a client transfer.
pub(crate) async fn start(
    inner: &Arc<Inner>,
    ticket: Ticket,
    op: Op,
    dest: Option<PathBuf>,
    paths: Vec<PathBuf>,
) -> Result<Transfer> {
    if ticket.body.admin {
        bail!("this is an admin ticket; use it for remote control, not transfers");
    }
    match op {
        Op::Get if !ticket.body.get => bail!("ticket does not allow downloading"),
        Op::Put if !ticket.body.put => bail!("ticket does not allow uploading"),
        _ => {}
    }
    ticket.verify_signature().map_err(|e| anyhow!(e))?;
    let host = ticket.host().map_err(|e| anyhow!(e))?;
    if host == inner.endpoint.id() {
        bail!("this ticket was issued by this device");
    }
    let job_dir = match (op, dest) {
        (Op::Get, Some(dest)) => {
            let fallback = format!("toss-{}", &ticket.id_hex()[..8]);
            let dir = dest.join(sanitize_component(&ticket.body.job, &fallback));
            tokio::fs::create_dir_all(&dir)
                .await
                .with_context(|| format!("create {}", dir.display()))?;
            Some(dir)
        }
        (Op::Get, None) => bail!("get needs a destination"),
        (Op::Put, _) => {
            if paths.is_empty() {
                bail!("nothing to send");
            }
            for p in &paths {
                tokio::fs::symlink_metadata(p)
                    .await
                    .with_context(|| format!("cannot read {}", p.display()))?;
            }
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
    let job = Arc::new(ClientJob {
        ticket,
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
                    && j.ticket.body.id == job.ticket.body.id
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
    tracker.started();
    let rx = tracker.subscribe_state();
    {
        let mut running = job.running.lock().await;
        spawn_locked(&mut running, inner, job.clone(), tracker);
    }
    Ok(Transfer { id, rx })
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
        Status::Done => bail!("transfer already finished"),
        Status::Cancelled => bail!("transfer was cancelled"),
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
            bail!("transfer already finished");
        }
        delete_parts(&job).await;
        tracker.error(ErrorCode::Cancelled, "cancelled".into(), None);
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
                "transfer task ended unexpectedly".into(),
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
            Err((code, msg)) => tracker.error(code, msg, None),
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
                    tracker.error(ErrorCode::Cancelled, "cancelled".into(), None);
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
                    return Err((ErrorCode::HashMismatch, msg));
                }
                tracing::warn!("{msg}; retrying once on a fresh connection");
                hash_retry_used = true;
                continue;
            }
            Err(SessErr::Denied(DenyReason::Busy)) => {
                (ErrorCode::Busy, "host is busy".to_string(), BUSY_LIMIT)
            }
            Err(SessErr::Denied(r)) => return Err((deny_code(&r), r.to_string())),
            Err(SessErr::Offline { connected, err }) => {
                if connected {
                    since = Instant::now();
                }
                (ErrorCode::HostOffline, format!("{err:#}"), OFFLINE_LIMIT)
            }
            Err(SessErr::Io(e)) => return Err((ErrorCode::Io, format!("{e:#}"))),
            Err(SessErr::Other(e)) => return Err((ErrorCode::Other, format!("{e:#}"))),
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
    let conn = inner
        .dial_host(&job.ticket, ALPN_XFER)
        .await
        .map_err(net(false))?;
    let (mut send, mut recv) = conn.open_bi().await.map_err(|e| net(false)(e.into()))?;
    let hello = ClientHello::v1 {
        ticket: job.ticket.clone(),
        device: inner.cfg.device_name.clone(),
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
    let summary =
        xfer::receive_files(&conn, &plan.base_c, plan.items, tracker, &inner.hashes).await?;

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
        return Err(SessErr::Other(anyhow!("{}", done.msg)));
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
    let (manifest, abs) = tokio::task::spawn_blocking(move || build_put_manifest(&paths, &hashes))
        .await
        .map_err(|e| SessErr::Other(e.into()))?
        .map_err(SessErr::Io)?;

    let (conn, mut send, mut recv) = open(inner, job, tracker).await?;
    write_msg(&mut send, &manifest).await.map_err(net(true))?;
    let want: Want = read_msg(&mut recv).await.map_err(net(true))?;
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
    xfer::send_files(&conn, items, CLIENT_STREAMS, tracker).await?;
    let done: Done = timed(
        "waiting for Done",
        xfer::stall_timeout(),
        read_msg(&mut recv),
    )
    .await?;
    let path = conn_path(&conn);
    let _ = send.finish();
    conn.close(0u32.into(), b"done");
    if !done.msg.is_empty() {
        tracing::info!(id = %tracker.id, "host: {}", done.msg);
    }
    if !done.ok {
        if done.msg.contains("hash mismatch") {
            return Ok(SessionEnd::Mismatch(done.msg));
        }
        return Err(SessErr::Other(anyhow!("host: {}", done.msg)));
    }
    Ok(SessionEnd::Complete(path))
}

/// Manifest for a put: each selected file is `name`, each selected folder keeps
/// its name as first segment.
fn build_put_manifest(paths: &[PathBuf], hashes: &HashCache) -> Result<(Manifest, Vec<PathBuf>)> {
    let mut manifest = Manifest::default();
    let mut abs = Vec::new();
    let mut seen = BTreeSet::new();
    for p in paths {
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow!("cannot send {}: no usable name", p.display()))?;
        let prefix = sanitize_rel(name).with_context(|| format!("cannot send {}", p.display()))?;
        for f in walk(p, &prefix)? {
            let rel = f.rel.as_str();
            if !seen.insert(rel.to_lowercase()) {
                tracing::warn!("skipping duplicate {rel}");
                continue;
            }
            let hash = hashes.hash(&f.abs)?;
            manifest.total += f.size;
            manifest.files.push(FileEntry {
                rel,
                size: f.size,
                hash,
            });
            abs.push(f.abs);
        }
    }
    Ok((manifest, abs))
}
