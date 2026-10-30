//! Host side of `toss/xfer/1`: ticket issuing and verification, the job queue
//! (`max_parallel_jobs`, `Busy`), serving gets and receiving puts.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use iroh::{endpoint::Connection, SecretKey};

use crate::{
    fsutil::{numbered_name, part_path, walk, HashCache, HASH_THREADS},
    jobs::{JobEvent, JobStore},
    node::{EntryKind, HostRuntime, Inner},
    paths::{check_inside, resolve_rel, sanitize_component, sanitize_rel, RelPath},
    ticket::{self, HostAddr, Ticket, TicketError, TicketSpec, MAX_JOB_LEN},
    transfer::{conn_path, ErrorCode, Side, Tracker},
    tuning::host_stream_cap,
    util::{clean_text, now_unix, random_id, random_suffix},
    wire::{
        read_msg, read_msg_max, write_msg, ClientHello, DenyReason, Done, FileEntry, HostHello,
        Manifest, Op, Want, WantItem, MAX_HELLO,
    },
    xfer::{self, net, timed, Pipe, RecvItem, SendItem, SessErr},
    ALPN_XFER2,
};

/// Starts hashing the source folder of a get ticket in the background
/// (`HashCache`, up to 4 files at once) so the manifest is ready when the
/// client connects. No-op for put-only tickets or unresolvable paths.
pub(crate) fn prehash_ticket(inner: &Inner, rt: &HostRuntime, t: &Ticket) {
    if !t.body.get || t.body.admin {
        return;
    }
    let Some(root) = rt.root(&t.body.root) else {
        return;
    };
    let Ok(path) = sanitize_rel(&t.body.path) else {
        return;
    };
    if let Ok(src) = resolve_rel(root, &path) {
        spawn_prehash(inner, src);
    }
}

/// Background prehash of `src` into the node's hash cache (see
/// [`crate::fsutil::spawn_prehash`]).
pub(crate) fn spawn_prehash(inner: &Inner, src: PathBuf) {
    let _ = crate::fsutil::spawn_prehash(inner.hashes.clone(), src);
}

/// [`issue_ticket`] with the node's key/jobs/addresses, then [`prehash_ticket`]
/// (for `Node::create_ticket` and ctl `CreateTicket`).
pub(crate) fn issue_ticket_and_prehash(
    inner: &Inner,
    rt: &HostRuntime,
    spec: TicketSpec,
) -> Result<Ticket> {
    let t = issue_ticket(&inner.secret, &inner.jobs, rt, spec, &inner.host_addr())?;
    prehash_ticket(inner, rt, &t);
    Ok(t)
}

/// Longest peer device name / app version kept (logs, jobs.json, UI).
pub(crate) const MAX_DEVICE: usize = 64;
/// Longest peer-supplied `Done.msg` kept in `jobs.json`.
const MAX_PEER_MSG: usize = 2000;

/// Signs a job ticket after validating the spec against the host config;
/// `addr` is the host's current relay/direct addresses (wire.md §1 v2).
pub(crate) fn issue_ticket(
    secret: &SecretKey,
    jobs: &JobStore,
    rt: &HostRuntime,
    mut spec: TicketSpec,
    addr: &HostAddr,
) -> Result<Ticket> {
    spec.job = spec.job.trim().chars().take(MAX_JOB_LEN).collect();
    if spec.job.is_empty() {
        return Err(crate::vi::ui("Tên việc không được để trống"));
    }
    if !spec.get && !spec.put {
        return Err(crate::vi::ui("Link phải cho phép tải về hoặc gửi lên"));
    }
    let root = rt
        .root(&spec.root)
        .ok_or_else(|| crate::vi::ui(format!("Không có thư mục gốc “{}”", spec.root)))?;
    let bad_path = |e: anyhow::Error| {
        tracing::info!("ticket path rejected: {e:#}");
        crate::vi::ui(format!("Đường dẫn không hợp lệ: {}", spec.path))
    };
    let path = sanitize_rel(&spec.path).map_err(bad_path)?;
    let src = resolve_rel(root, &path).map_err(bad_path)?;
    if spec.get && std::fs::symlink_metadata(&src).is_err() {
        return Err(crate::vi::ui(format!("Không tìm thấy {}", src.display())));
    }
    spec.path = path.as_str();
    spec.ret = sanitize_rel(&spec.ret)
        .map_err(|_| crate::vi::ui(format!("Thư mục nhận không hợp lệ: {}", spec.ret)))?
        .as_str();
    let mut body = ticket::body_from_spec(&secret.public(), &spec, random_id());
    body.set_host_addr(addr);
    let t = Ticket::sign(body, secret);
    jobs.add(&t.body)?;
    log_issued(&t);
    Ok(t)
}

/// Signs an admin ticket (with the host's current addresses, see [`issue_ticket`]).
pub(crate) fn issue_admin_ticket(
    secret: &SecretKey,
    jobs: &JobStore,
    days: u32,
    addr: &HostAddr,
) -> Result<Ticket> {
    let mut body = ticket::admin_body(&secret.public(), days, random_id());
    body.set_host_addr(addr);
    let t = Ticket::sign(body, secret);
    jobs.add(&t.body)?;
    log_issued(&t);
    Ok(t)
}

fn log_issued(t: &Ticket) {
    let addr = t.body.host_addr();
    if addr.is_empty() {
        tracing::warn!(
            id = %t.id_hex(),
            "ticket issued without relay/direct addresses (host not online yet?); \
             clients will depend on n0 DNS discovery"
        );
    } else {
        tracing::info!(id = %t.id_hex(), job = %t.body.job, "ticket issued ({})", addr.summary());
    }
}

/// wire.md §1 steps 1–3 (signature, expiry, revocation).
pub(crate) fn verify_common(inner: &Inner, t: &Ticket) -> Result<(), DenyReason> {
    match t.verify(&inner.endpoint.id()) {
        Ok(()) => {}
        Err(TicketError::Expired) => return Err(DenyReason::Expired),
        Err(_) => return Err(DenyReason::BadSignature),
    }
    match inner.jobs.revocation(&t.body.id) {
        Ok(false) => Ok(()),
        Ok(true) => Err(DenyReason::Revoked),
        // Fail closed: an unreadable revocation list must not un-revoke tickets.
        Err(e) => {
            tracing::error!("{e:#}");
            Err(DenyReason::Other(
                "Máy chia sẻ không đọc được danh sách link đã thu hồi".into(),
            ))
        }
    }
}

/// Where a verified xfer ticket points.
#[derive(Debug, Clone)]
pub(crate) struct Resolved {
    /// get: canonical root + the source path.
    pub src: PathBuf,
    pub root_c: PathBuf,
    /// put: return root (canonical) and target dir.
    pub ret_dir: PathBuf,
    pub return_root_c: PathBuf,
}

/// Full verification for an xfer request (wire.md §1 steps 1–4 + op check).
pub(crate) fn verify_xfer(
    inner: &Inner,
    rt: &HostRuntime,
    t: &Ticket,
    op: Op,
) -> Result<Resolved, DenyReason> {
    verify_common(inner, t)?;
    let b = &t.body;
    if b.admin {
        return Err(DenyReason::NotAllowed);
    }
    match op {
        Op::Get if !b.get => return Err(DenyReason::NotAllowed),
        Op::Put if !b.put => return Err(DenyReason::NotAllowed),
        _ => {}
    }
    let root_c = rt
        .root(&b.root)
        .ok_or(DenyReason::UnknownRoot)?
        .to_path_buf();
    let path = sanitize_rel(&b.path).map_err(|_| DenyReason::BadPath)?;
    let src = resolve_rel(&root_c, &path).map_err(|_| DenyReason::BadPath)?;
    if op == Op::Get && std::fs::symlink_metadata(&src).is_err() {
        return Err(DenyReason::BadPath);
    }
    let ret = if b.ret.is_empty() {
        let fallback = format!("job-{}", &hex::encode(b.id)[..8]);
        sanitize_rel(&sanitize_component(&b.job, &fallback)).map_err(|_| DenyReason::BadPath)?
    } else {
        sanitize_rel(&b.ret).map_err(|_| DenyReason::BadPath)?
    };
    let ret_dir = resolve_rel(&rt.return_root_c, &ret).map_err(|_| DenyReason::BadPath)?;
    Ok(Resolved {
        src,
        root_c,
        ret_dir,
        return_root_c: rt.return_root_c.clone(),
    })
}

/// Holds one of the `max_parallel_jobs` slots.
struct Slot(Arc<Inner>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
        self.0.emit_host_status();
    }
}

fn try_acquire(inner: &Arc<Inner>, max: u32) -> Option<Slot> {
    let ok = inner
        .active
        .try_update(Ordering::SeqCst, Ordering::SeqCst, |a| {
            (a < max).then_some(a + 1)
        })
        .is_ok();
    ok.then(|| Slot(inner.clone()))
}

async fn deny(conn: &Connection, send: &mut iroh::endpoint::SendStream, reason: DenyReason) {
    tracing::info!(
        remote = %conn.remote_id().fmt_short(),
        alpn = %String::from_utf8_lossy(conn.alpn()),
        "denied: {reason}"
    );
    let _ = write_msg(send, &HostHello::Denied { reason }).await;
    let _ = send.finish();
    // Give the client a moment to read the answer, then close.
    let _ = tokio::time::timeout(Duration::from_secs(3), conn.closed()).await;
    conn.close(1u32.into(), b"denied");
}

/// Handles one incoming `toss/xfer/1` connection.
pub(crate) async fn handle_xfer(inner: Arc<Inner>, conn: Connection) {
    let remote = conn.remote_id();
    let hello = tokio::time::timeout(xfer::pre_auth_timeout(), async {
        let (send, mut recv) = conn.accept_bi().await?;
        let hello: ClientHello = read_msg_max(&mut recv, MAX_HELLO).await?;
        anyhow::Ok((send, recv, hello))
    })
    .await;
    let (mut send, mut recv, hello) = match hello {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            // v0.4: test_connect / re-punch probes connect and close with "probe" without a hello.
            if matches!(conn.close_reason(), Some(iroh::endpoint::ConnectionError::ApplicationClosed(ref c)) if c.reason.as_ref() == b"probe")
            {
                tracing::debug!(remote = %remote.fmt_short(), "probe connection closed");
                return;
            }
            tracing::info!(remote = %remote.fmt_short(), "denied: bad hello: {e:#}");
            conn.close(1u32.into(), b"bad hello");
            return;
        }
        Err(_) => {
            tracing::info!(remote = %remote.fmt_short(), "denied: no hello within the pre-auth timeout");
            conn.close(1u32.into(), b"hello timeout");
            return;
        }
    };
    let ClientHello::v1 {
        ticket,
        device,
        op,
        app_version,
    } = hello;
    let device = clean_text(&device, MAX_DEVICE);
    let app_version = clean_text(&app_version, MAX_DEVICE);
    let Some(rt) = inner.host_runtime() else {
        deny(&conn, &mut send, DenyReason::NotAllowed).await;
        return;
    };
    let rt2 = rt.clone();
    let inner2 = inner.clone();
    let ticket2 = ticket.clone();
    let resolved =
        match tokio::task::spawn_blocking(move || verify_xfer(&inner2, &rt2, &ticket2, op)).await {
            Ok(Ok(r)) => r,
            Ok(Err(reason)) => {
                deny(&conn, &mut send, reason).await;
                return;
            }
            Err(e) => {
                deny(&conn, &mut send, DenyReason::Other(e.to_string())).await;
                return;
            }
        };
    let Some(slot) = try_acquire(&inner, rt.cfg.max_parallel_jobs) else {
        inner.busy_seen.lock().insert(remote, Instant::now());
        inner.emit_host_status();
        deny(&conn, &mut send, DenyReason::Busy).await;
        return;
    };
    inner.busy_seen.lock().remove(&remote);
    inner.emit_host_status();
    if write_msg(
        &mut send,
        &HostHello::Ok {
            job: ticket.body.job.clone(),
        },
    )
    .await
    .is_err()
    {
        return;
    }
    tracing::info!(
        job = %ticket.body.job,
        %device,
        ?op,
        %app_version,
        alpn = %String::from_utf8_lossy(conn.alpn()),
        "transfer accepted"
    );

    let id = format!("{}-{}", ticket.id_hex(), random_suffix());
    let dest = match op {
        Op::Put => Some(resolved.ret_dir.display().to_string()),
        Op::Get => None,
    };
    let tracker = Tracker::new(
        id,
        ticket.body.job.clone(),
        op,
        Side::Host,
        device.clone(),
        dest,
        inner.events.clone(),
    );
    inner.register(tracker.clone(), EntryKind::Host(conn.clone()));
    tracker.started();

    let ticker = tracker.spawn_ticker(conn.clone());
    let res = match op {
        Op::Get => {
            host_get(
                &inner, &rt, &conn, &mut send, &mut recv, &resolved, &tracker,
            )
            .await
        }
        Op::Put => {
            receive_put(
                &inner,
                &conn,
                &mut send,
                &mut recv,
                &resolved.return_root_c,
                &resolved.ret_dir,
                &tracker,
                conn.alpn() == ALPN_XFER2,
            )
            .await
        }
    };
    ticker.abort();
    let path = conn_path(&conn);
    let ev = match &res {
        Ok(done) => {
            if done.ok {
                tracker.finish_done(path);
            } else {
                tracker.tick(path);
                tracing::warn!(id = %tracker.id, "put incomplete: {}", done.msg);
                tracker.error(ErrorCode::HashMismatch, crate::vi::DONE_FAILED.into(), None);
            }
            JobEvent {
                at: now_unix(),
                device: device.clone(),
                op,
                files: done.files,
                bytes: done.bytes,
                ok: done.ok,
                msg: clean_text(&done.msg, MAX_PEER_MSG),
            }
        }
        Err(e) => {
            let code = match e {
                SessErr::Offline { .. } => ErrorCode::HostOffline,
                SessErr::Io(_) => ErrorCode::Io,
                SessErr::Denied(_) => ErrorCode::Denied,
                SessErr::Other(_) => ErrorCode::Other,
            };
            tracing::info!(id = %tracker.id, "host-side transfer failed: {e}");
            tracker.error(code, crate::vi::sess_msg(e), None);
            JobEvent {
                at: now_unix(),
                device: device.clone(),
                op,
                files: tracker.files_done.load(Ordering::Relaxed),
                bytes: tracker.moved.load(Ordering::Relaxed),
                ok: false,
                msg: e.to_string(),
            }
        }
    };
    if let Err(e) = inner.jobs.record_event(ticket.body.id, ev) {
        tracing::warn!("could not record job event: {e:#}");
    }
    drop(slot);
}

async fn host_get(
    inner: &Arc<Inner>,
    rt: &HostRuntime,
    conn: &Connection,
    send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream,
    resolved: &Resolved,
    tracker: &Arc<Tracker>,
) -> Result<Done, SessErr> {
    // Build the manifest (hashing may take a while for big jobs).
    let src = resolved.src.clone();
    let root_c = resolved.root_c.clone();
    let hashes = inner.hashes.clone();
    let (manifest, abs) =
        tokio::task::spawn_blocking(move || build_manifest(&src, &root_c, &hashes))
            .await
            .map_err(|e| SessErr::Other(e.into()))?
            .map_err(SessErr::Io)?;
    write_msg(send, &manifest).await.map_err(net(true))?;
    let want: Want = timed(
        "waiting for the file list",
        xfer::WANT_TIMEOUT,
        read_msg(recv),
    )
    .await?;

    let mut items = Vec::with_capacity(want.items.len());
    let mut seen = HashSet::new();
    let mut to_do = 0u64;
    for w in &want.items {
        let f = manifest
            .files
            .get(w.index as usize)
            .ok_or_else(|| SessErr::Other(anyhow!("want index {} out of range", w.index)))?;
        if w.offset > f.size || !seen.insert(w.index) {
            return Err(SessErr::Other(anyhow!("bad want item {w:?}")));
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
    if conn.alpn() == ALPN_XFER2 {
        let pipe = Pipe::new(
            &inner.cfg.net.tuning,
            host_stream_cap(rt.cfg.max_parallel_streams),
        );
        xfer::send_files_v2(conn, items, &pipe, tracker).await?;
    } else {
        xfer::send_files(conn, items, rt.cfg.max_parallel_streams as usize, tracker).await?;
    }
    let done: Done = timed("waiting for Done", xfer::stall_timeout(), read_msg(recv)).await?;
    tracing::info!(files = done.files, bytes = done.bytes, ok = done.ok, msg = %done.msg, "client done");
    let _ = send.finish();
    conn.close(0u32.into(), b"done");
    Ok(done)
}

/// Walks the ticket source and hashes every file (up to [`HASH_THREADS`] at
/// once; cached ones from a prehash are free). Returns the manifest and the
/// absolute path for each entry.
fn build_manifest(
    src: &std::path::Path,
    root_c: &std::path::Path,
    hashes: &HashCache,
) -> Result<(Manifest, Vec<PathBuf>)> {
    let meta = std::fs::symlink_metadata(src).with_context(|| format!("stat {}", src.display()))?;
    let prefix = if meta.is_file() {
        let name = src
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow!("bad file name"))?;
        sanitize_rel(name)?
    } else {
        RelPath::default()
    };
    // Defense in depth: never serve something that resolves outside the root.
    let files: Vec<_> = walk(src, &prefix)?
        .into_iter()
        .filter(|f| check_inside(root_c, &f.abs).is_ok())
        .collect();
    let paths: Vec<PathBuf> = files.iter().map(|f| f.abs.clone()).collect();
    let hashed = hashes.hash_many(&paths, HASH_THREADS);
    let mut manifest = Manifest::default();
    let mut abs = Vec::with_capacity(files.len());
    for (f, hash) in files.into_iter().zip(hashed) {
        let hash = hash?;
        let size = std::fs::metadata(&f.abs)?.len();
        manifest.total += size;
        manifest.files.push(FileEntry {
            rel: f.rel.as_str(),
            size,
            hash,
        });
        abs.push(f.abs);
    }
    Ok((manifest, abs))
}

/// Receiving side of a put (xfer put via ticket, or a v0.5 push from a
/// contact): reads the `Manifest`, answers `Want` (resume against `.part`s,
/// never overwriting a different complete file under `ret_dir`), receives the
/// data and answers `Done`. `v2` = segmented data streams (xfer/2; always for
/// pushes).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn receive_put(
    inner: &Arc<Inner>,
    conn: &Connection,
    send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream,
    return_root_c: &std::path::Path,
    ret_dir: &std::path::Path,
    tracker: &Arc<Tracker>,
    v2: bool,
) -> Result<Done, SessErr> {
    let manifest: Manifest = timed(
        "waiting for the file list",
        xfer::stall_timeout(),
        read_msg(recv),
    )
    .await?;
    xfer::check_manifest(&manifest).map_err(SessErr::Other)?;
    let ret_dir = ret_dir.to_path_buf();
    let return_root_c = return_root_c.to_path_buf();
    let hashes = inner.hashes.clone();
    let reserved = inner.put_reserved.clone();
    let m2 = manifest.clone();
    let plan = tokio::task::spawn_blocking(move || {
        plan_put(&return_root_c, &ret_dir, &m2, &hashes, &reserved)
    })
    .await
    .map_err(|e| SessErr::Other(e.into()))?
    .map_err(SessErr::Io)?;
    // Released when this session ends, however it ends.
    let _reservation = Reservation {
        set: inner.put_reserved.clone(),
        keys: plan.reserved.clone(),
    };
    write_msg(send, &plan.want).await.map_err(net(true))?;
    tracker.begin_session(
        manifest.files.len() as u32,
        manifest.total,
        plan.want.items.len() as u32,
        plan.to_do_bytes,
    );
    let summary = if v2 {
        let pipe = Pipe::new(&inner.cfg.net.tuning, None);
        xfer::receive_files_v2(
            conn,
            &plan.base_c,
            plan.items,
            &pipe,
            tracker,
            &inner.hashes,
        )
        .await?
    } else {
        xfer::receive_files(conn, &plan.base_c, plan.items, tracker, &inner.hashes).await?
    };

    let mut notes = Vec::new();
    if !plan.renames.is_empty() {
        notes.push(format!("renamed: {}", plan.renames.join("; ")));
    }
    if !plan.skipped.is_empty() {
        notes.push(format!(
            "skipped (invalid name): {}",
            plan.skipped.join("; ")
        ));
    }
    if !summary.mismatched.is_empty() {
        notes.push(format!("hash mismatch: {}", summary.mismatched.join("; ")));
    }
    let done = Done {
        files: summary.files,
        bytes: summary.bytes,
        ok: summary.mismatched.is_empty() && plan.skipped.is_empty(),
        msg: notes.join(" | "),
    };
    write_msg(send, &done).await.map_err(net(true))?;
    let _ = send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(10), conn.closed()).await;
    Ok(done)
}

struct PutPlan {
    base_c: PathBuf,
    want: Want,
    items: HashMap<u32, RecvItem>,
    to_do_bytes: u64,
    renames: Vec<String>,
    skipped: Vec<String>,
    /// Keys of the final paths reserved in `Inner::put_reserved`.
    reserved: Vec<String>,
}

/// Final paths that running puts are writing (`<final>` and `<final>.part`),
/// shared by all put sessions of a host so that two concurrent uploads of the
/// same name never share a `.part` file or replace each other's result.
pub(crate) type PutReservations = Arc<parking_lot::Mutex<HashSet<String>>>;

/// Case-folded key (the return root may be on a case-insensitive filesystem).
fn reservation_key(p: &std::path::Path) -> String {
    p.to_string_lossy().to_lowercase()
}

/// Releases a put session's reservations on drop.
struct Reservation {
    set: PutReservations,
    keys: Vec<String>,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut set = self.set.lock();
        for k in &self.keys {
            set.remove(k);
        }
    }
}

/// Decides, for every incoming file, its final name under `ret_dir` (never
/// overwriting a different complete file, never sharing a target with another
/// running put) and the resume offset. Chosen targets are added to `reserved`
/// and listed in `PutPlan::reserved`; the caller must release them.
fn plan_put(
    return_root_c: &std::path::Path,
    ret_dir: &std::path::Path,
    manifest: &Manifest,
    hashes: &HashCache,
    reserved: &PutReservations,
) -> Result<PutPlan> {
    let mut keys = Vec::new();
    let res = plan_put_inner(
        return_root_c,
        ret_dir,
        manifest,
        hashes,
        reserved,
        &mut keys,
    );
    match res {
        Ok(mut plan) => {
            plan.reserved = keys;
            Ok(plan)
        }
        Err(e) => {
            drop(Reservation {
                set: reserved.clone(),
                keys,
            });
            Err(e)
        }
    }
}

fn plan_put_inner(
    return_root_c: &std::path::Path,
    ret_dir: &std::path::Path,
    manifest: &Manifest,
    hashes: &HashCache,
    reserved: &PutReservations,
    keys: &mut Vec<String>,
) -> Result<PutPlan> {
    std::fs::create_dir_all(ret_dir).with_context(|| format!("create {}", ret_dir.display()))?;
    check_inside(return_root_c, ret_dir)?;
    let base_c = crate::paths::canonicalize(ret_dir)?;
    let mut plan = PutPlan {
        base_c: base_c.clone(),
        want: Want::default(),
        items: HashMap::new(),
        to_do_bytes: 0,
        renames: Vec::new(),
        skipped: Vec::new(),
        reserved: Vec::new(),
    };
    let mut taken: HashSet<String> = HashSet::new();
    for (i, e) in manifest.files.iter().enumerate() {
        let index = u32::try_from(i).context("too many files")?;
        let rel = match sanitize_rel(&e.rel) {
            Ok(r) if !r.is_empty() => r,
            _ => {
                plan.skipped.push(e.rel.clone());
                continue;
            }
        };
        let name = rel.file_name().unwrap_or_default().to_string();
        let mut chosen = None;
        for n in 1..=9999u32 {
            let cand = if n == 1 {
                rel.clone()
            } else {
                rel.with_file_name(&numbered_name(&name, n))
            };
            let key = cand.as_str().to_lowercase();
            if taken.contains(&key) {
                continue;
            }
            let path = base_c.join(cand.to_path());
            if check_inside(&base_c, &path).is_err() {
                break;
            }
            let rkey = reservation_key(&path);
            if reserved.lock().contains(&rkey) {
                // Another running put is writing this name right now.
                continue;
            }
            match std::fs::symlink_metadata(&path) {
                Ok(m) => {
                    if m.is_file() && m.len() == e.size && hashes.hash(&path)? == e.hash {
                        // Identical file already there: nothing to do.
                        taken.insert(key);
                        chosen = Some(None);
                        break;
                    }
                }
                Err(_) => {
                    // Reserve atomically (a concurrent plan may have just taken it).
                    if !reserved.lock().insert(rkey.clone()) {
                        continue;
                    }
                    keys.push(rkey);
                    taken.insert(key);
                    chosen = Some(Some((cand, path)));
                    break;
                }
            }
        }
        let Some(choice) = chosen else {
            plan.skipped.push(e.rel.clone());
            continue;
        };
        let Some((cand, final_path)) = choice else {
            continue;
        };
        let part = part_path(&final_path);
        if check_inside(&base_c, &part).is_err() {
            plan.skipped.push(e.rel.clone());
            continue;
        }
        let Some(offset) = xfer::resume_offset(&final_path, &part, e, hashes)? else {
            continue;
        };
        if cand != rel {
            plan.renames.push(format!("{} -> {}", rel, cand));
        }
        plan.to_do_bytes = plan.to_do_bytes.saturating_add(e.size - offset);
        plan.want.items.push(WantItem { index, offset });
        plan.items.insert(
            index,
            RecvItem {
                entry: e.clone(),
                final_path,
                part,
                offset,
                label: cand.as_str(),
                no_clobber: true,
            },
        );
    }
    Ok(plan)
}
