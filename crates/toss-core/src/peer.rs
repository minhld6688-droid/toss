//! ALPN `toss/peer/1` (contracts/contacts-v0.5.md §2.2): pairing with an
//! invite, presence pings between contacts and pushes from contacts.
//!
//! Every node accepts it. The first bi stream carries one [`PeerReq`] and
//! its [`PeerResp`] (postcard, u32-LE length prefix, ≤ 64 KiB). A contact is
//! authenticated by `conn.remote_id()` (QUIC/TLS) against `contacts.json`;
//! anything but `Pair` from a stranger is denied. After `PushAccepted` the
//! same stream and connection carry xfer/2 put semantics.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use iroh::{endpoint::Connection, EndpointAddr, EndpointId, PublicKey, TransportAddr, Watcher};
use parking_lot::{Mutex, RwLock};
use tokio::{
    sync::{oneshot, watch, Notify},
    task::JoinSet,
};

use crate::{
    contacts::{addr_from_hint, platform, Contact, IncomingPush, Invite, MAX_NAME},
    host,
    node::{EntryKind, Inner},
    paths::{canonicalize, sanitize_component},
    ticket::{HostAddr, MAX_JOB_LEN},
    transfer::{conn_path, conn_rtt_ms, ErrorCode, Side, Status, Tracker, TransferEvent},
    util::{clean_text, now_unix, random_suffix},
    wire::{read_msg_max, write_msg, Op, PeerReq, PeerResp, MAX_HELLO},
    xfer::{self, SessErr},
    ALPN_PEER,
};

/// Presence ping period while the node runs.
pub const PRESENCE_INTERVAL: Duration = Duration::from_secs(60);
/// How long a queued push waits for the user.
pub const PUSH_ANSWER_TIMEOUT: Duration = Duration::from_secs(600);
/// A presence ping: dial at most this long…
const PING_DIAL_TIMEOUT: Duration = Duration::from_secs(10);
/// …then the `Pong` must arrive within this.
const PONG_TIMEOUT: Duration = Duration::from_secs(5);
/// Answers to `Pair` / `Push` must arrive within this.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

/// hex(endpoint id), the contact id.
pub(crate) fn id_hex(id: EndpointId) -> String {
    hex::encode(id.as_bytes())
}

/// Parses a contact id (64 hex chars).
pub(crate) fn parse_endpoint_id(s: &str) -> Result<EndpointId> {
    let bytes: [u8; 32] = hex::decode(s.trim())
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| anyhow!("mã máy không hợp lệ: {s}"))?;
    PublicKey::from_bytes(&bytes).map_err(|_| anyhow!("mã máy không hợp lệ: {s}"))
}

/// A push from a contact waiting for the user.
#[derive(Debug)]
struct Pending {
    info: IncomingPush,
    token: u64,
    answer: oneshot::Sender<bool>,
}

/// Runtime state of `toss/peer/1` on a node.
#[derive(Debug)]
pub(crate) struct PeerState {
    /// Where pushes land: `<inbox_dir>/<contact name>/<job>/`.
    pub inbox_dir: RwLock<PathBuf>,
    /// Pushes waiting for `answer_push`, by hex(push id).
    pending: Mutex<HashMap<String, Pending>>,
    next_token: AtomicU64,
    /// Push ids the user accepted (a resumed push is not asked again).
    accepted: Mutex<HashSet<[u8; 16]>>,
    /// Bumped whenever a contact comes online or is removed (outbox wake-up).
    online_gen: watch::Sender<u64>,
    /// Asks the presence loop for an immediate round.
    kick: Arc<Notify>,
    /// Live peer connections per contact (a contact with one is online).
    live: Mutex<HashMap<EndpointId, usize>>,
    /// Runtime for work started from sync API calls.
    rt: tokio::runtime::Handle,
}

impl PeerState {
    pub fn new(data_dir: &Path) -> PeerState {
        let inbox = dirs::home_dir()
            .map(|h| h.join("Toss"))
            .unwrap_or_else(|| data_dir.join("inbox"));
        PeerState {
            inbox_dir: RwLock::new(inbox),
            pending: Mutex::new(HashMap::new()),
            next_token: AtomicU64::new(1),
            accepted: Mutex::new(HashSet::new()),
            online_gen: watch::channel(0).0,
            kick: Arc::new(Notify::new()),
            live: Mutex::new(HashMap::new()),
            rt: tokio::runtime::Handle::current(),
        }
    }

    /// `Node::incoming_pushes`.
    pub fn incoming(&self) -> Vec<IncomingPush> {
        let mut v: Vec<IncomingPush> = self
            .pending
            .lock()
            .values()
            .map(|p| p.info.clone())
            .collect();
        v.sort_by(|a, b| (a.at, &a.push_id).cmp(&(b.at, &b.push_id)));
        v
    }

    /// `Node::answer_push`.
    pub fn answer(&self, push_id: &str, accept: bool) -> Result<()> {
        let p = self
            .pending
            .lock()
            .remove(&push_id.trim().to_ascii_lowercase())
            .ok_or_else(|| {
                anyhow!("không còn yêu cầu gửi file này (đã hết hạn hoặc đã trả lời)")
            })?;
        let _ = p.answer.send(accept);
        Ok(())
    }

    fn bump(&self) {
        self.online_gen.send_modify(|g| *g += 1);
    }

    fn live_count(&self, id: &EndpointId) -> usize {
        self.live.lock().get(id).copied().unwrap_or(0)
    }
}

/// Marks a contact online while a peer connection with it is alive.
pub(crate) struct LiveGuard {
    inner: Arc<Inner>,
    id: EndpointId,
}

impl LiveGuard {
    pub fn new(inner: &Arc<Inner>, id: EndpointId) -> LiveGuard {
        *inner.peer.live.lock().entry(id).or_insert(0) += 1;
        set_presence(inner, &id_hex(id), true, None);
        LiveGuard {
            inner: inner.clone(),
            id,
        }
    }
}

impl Drop for LiveGuard {
    fn drop(&mut self) {
        let mut live = self.inner.peer.live.lock();
        if let Some(n) = live.get_mut(&self.id) {
            *n -= 1;
            if *n == 0 {
                live.remove(&self.id);
            }
        }
    }
}

/// Records a presence result and emits `ContactPresence` when it changed.
/// "Offline" is ignored while a connection with the contact is alive.
pub(crate) fn set_presence(inner: &Inner, id: &str, online: bool, rtt_ms: Option<u32>) {
    if !online {
        if let Ok(eid) = parse_endpoint_id(id) {
            if inner.peer.live_count(&eid) > 0 {
                return;
            }
        }
    }
    if let Some(c) = inner.contacts.set_presence(id, online, rtt_ms) {
        inner.emit(TransferEvent::ContactPresence {
            id: c.id.clone(),
            online: c.online,
            rtt_ms: c.rtt_ms,
        });
        if c.online {
            inner.peer.bump();
        }
    }
}

/// A push could not reach the contact: mark it offline (unless connected).
pub(crate) fn note_unreachable(inner: &Inner, id: EndpointId) {
    set_presence(inner, &id_hex(id), false, None);
}

/// Waits until contact `id` is online (shows "Chờ <name> online" meanwhile).
/// Err when the contact was removed.
pub(crate) async fn wait_online(
    inner: &Arc<Inner>,
    id: EndpointId,
    tracker: &Tracker,
) -> Result<(), String> {
    let hex = id_hex(id);
    let mut gen = inner.peer.online_gen.subscribe();
    let mut announced = false;
    loop {
        let Some(c) = inner.contacts.get(&hex) else {
            return Err("máy nhận đã bị xoá khỏi danh bạ".into());
        };
        if c.c.online {
            return Ok(());
        }
        if !announced {
            announced = true;
            // Maybe it just came up: check now instead of at the next round.
            inner.peer.kick.notify_one();
            tracker.error(
                ErrorCode::HostOffline,
                format!("Chờ {} online", c.c.name),
                Some(PRESENCE_INTERVAL.as_secs()),
            );
        }
        if gen.changed().await.is_err() {
            return Err("node stopped".into());
        }
    }
}

/// The last addresses iroh used for `id` (relay + direct), as a hint.
async fn remote_hint(inner: &Inner, id: EndpointId) -> HostAddr {
    match inner.endpoint.remote_info(id).await {
        Some(info) => {
            let addrs: Vec<TransportAddr> = info.addrs().map(|a| a.addr().clone()).collect();
            HostAddr::from_endpoint_addr(&EndpointAddr::from_parts(id, addrs))
        }
        None => HostAddr::default(),
    }
}

/// Dials a contact (its last known addresses; n0/mDNS lookup as fallback).
pub(crate) async fn dial_contact(inner: &Inner, id: EndpointId) -> Result<Connection> {
    let hint = inner
        .contacts
        .get(&id_hex(id))
        .map(|s| s.addr)
        .unwrap_or_default();
    inner
        .connect_alpns(addr_from_hint(id, &hint), ALPN_PEER, Vec::new())
        .await
}

/// `Node::accept_invite`.
pub(crate) async fn pair(inner: &Arc<Inner>, text: &str) -> Result<Contact> {
    let inv: Invite = text.parse()?;
    inv.verify()?;
    let inviter = inv.inviter()?;
    if inviter == inner.endpoint.id() {
        bail!("đây là lời mời của chính máy này");
    }
    tracing::info!(
        inviter = %inviter.fmt_short(),
        name = %inv.body.name,
        "pairing: dialing the inviting device ({})",
        inv.host_addr().summary()
    );
    let conn = inner
        .connect_alpns(inv.endpoint_addr()?, ALPN_PEER, Vec::new())
        .await
        .with_context(|| format!("không nối được tới {}", inv.body.name))?;
    let res = async {
        let (mut send, mut recv) = conn.open_bi().await?;
        let req = PeerReq::Pair {
            secret: inv.body.secret,
            name: inner.cfg.device_name.clone(),
            platform: platform(),
            version: inner.cfg.app_version.clone(),
        };
        write_msg(&mut send, &req).await?;
        let resp: PeerResp =
            tokio::time::timeout(ANSWER_TIMEOUT, read_msg_max(&mut recv, MAX_HELLO))
                .await
                .map_err(|_| anyhow!("máy kia không trả lời"))??;
        anyhow::Ok(resp)
    }
    .await;
    let rtt = conn_rtt_ms(&conn);
    conn.close(0u32.into(), b"paired");
    match res? {
        PeerResp::Paired { name, platform, .. } => {
            let rid = id_hex(inviter);
            let name = if name.trim().is_empty() {
                inv.body.name.clone()
            } else {
                name
            };
            inner
                .contacts
                .upsert(&rid, &name, &platform, inv.host_addr());
            tracing::info!(contact = %inviter.fmt_short(), %name, "paired");
            inner.emit(TransferEvent::ContactsChanged {});
            set_presence(inner, &rid, true, rtt);
            inner
                .contacts
                .get(&rid)
                .map(|s| s.c)
                .ok_or_else(|| anyhow!("contact vanished"))
        }
        PeerResp::Denied { reason } => bail!("{} từ chối kết nối: {reason}", inv.body.name),
        other => bail!("unexpected answer to Pair: {other:?}"),
    }
}

/// One presence ping: `Hello` → `Pong`. Returns (RTT, the name it reports).
async fn ping(inner: &Inner, id: EndpointId) -> Result<(Option<u32>, String)> {
    let conn = tokio::time::timeout(PING_DIAL_TIMEOUT, dial_contact(inner, id))
        .await
        .map_err(|_| anyhow!("no answer within {}s", PING_DIAL_TIMEOUT.as_secs()))??;
    let res = async {
        let (mut send, mut recv) = conn.open_bi().await?;
        let hello = PeerReq::Hello {
            name: inner.cfg.device_name.clone(),
            version: inner.cfg.app_version.clone(),
        };
        write_msg(&mut send, &hello).await?;
        let resp: PeerResp = tokio::time::timeout(PONG_TIMEOUT, read_msg_max(&mut recv, MAX_HELLO))
            .await
            .map_err(|_| anyhow!("no pong within {}s", PONG_TIMEOUT.as_secs()))??;
        anyhow::Ok(resp)
    }
    .await;
    let rtt = conn_rtt_ms(&conn);
    conn.close(0u32.into(), b"hello");
    match res? {
        PeerResp::Pong { name, .. } => Ok((rtt, clean_text(&name, MAX_NAME))),
        PeerResp::Denied { reason } => bail!("denied: {reason}"),
        other => bail!("unexpected answer to Hello: {other:?}"),
    }
}

/// Pings every contact concurrently and records the results.
pub(crate) async fn ping_all(inner: &Arc<Inner>) {
    let contacts = inner.contacts.list();
    let mut set = JoinSet::new();
    for c in contacts {
        let Ok(id) = parse_endpoint_id(&c.id) else {
            continue;
        };
        let inner = inner.clone();
        set.spawn(async move {
            let res = ping(&inner, id).await;
            (c, res)
        });
    }
    while let Some(joined) = set.join_next().await {
        let Ok((c, res)) = joined else { continue };
        match res {
            Ok((rtt, name)) => {
                if !name.is_empty() && name != c.their_name {
                    let _ = inner.contacts.update(&c.id, |c| c.their_name = name);
                    inner.emit(TransferEvent::ContactsChanged {});
                }
                set_presence(inner, &c.id, true, rtt);
            }
            Err(e) => {
                // Offline contacts are normal: never above debug.
                tracing::debug!(contact = %c.name, "presence: no answer: {e:#}");
                set_presence(inner, &c.id, false, None);
            }
        }
    }
}

/// Presence loop: pings all contacts on start, every 60 s, on a network
/// change (debounced) and when asked (`kick`).
pub(crate) fn spawn_presence(inner: &Arc<Inner>) {
    let weak = Arc::downgrade(inner);
    let kick = inner.peer.kick.clone();
    let mut addrs = inner.endpoint.watch_addr();
    tokio::spawn(async move {
        loop {
            {
                let Some(inner) = weak.upgrade() else { return };
                if inner.endpoint.is_closed() {
                    return;
                }
                ping_all(&inner).await;
            }
            tokio::select! {
                _ = tokio::time::sleep(PRESENCE_INTERVAL) => {}
                _ = kick.notified() => {}
                r = addrs.updated() => {
                    if r.is_err() {
                        return;
                    }
                    tracing::debug!("network changed: presence round");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    });
}

/// `Node::remove_contact`.
pub(crate) fn remove_contact(inner: &Arc<Inner>, id: &str) -> Result<()> {
    let id = id.trim().to_ascii_lowercase();
    inner.contacts.remove(&id)?;
    let eid = parse_endpoint_id(&id).ok();
    // Forget pushes to it (cancel the unfinished ones).
    let victims: Vec<_> = {
        let map = inner.transfers.lock();
        map.iter()
            .filter_map(|(tid, e)| match &e.kind {
                EntryKind::Client(j) if eid.is_some() && j.contact() == eid => {
                    Some((tid.clone(), j.clone(), e.tracker.clone()))
                }
                _ => None,
            })
            .collect()
    };
    let ids: Vec<String> = victims.iter().map(|(t, _, _)| t.clone()).collect();
    for (_, job, tracker) in victims {
        if !tracker.status().is_final() {
            inner.peer.rt.spawn(crate::client::cancel(job, tracker));
        }
    }
    inner.transfers.lock().retain(|k, _| !ids.contains(k));
    inner.store.remove(&ids);
    // Decline what it is waiting for.
    inner
        .peer
        .pending
        .lock()
        .retain(|_, p| p.info.from_id != id);
    inner.peer.bump();
    inner.emit(TransferEvent::ContactsChanged {});
    tracing::info!(contact = %id, "contact removed");
    Ok(())
}

/// Writes `Denied` and closes.
async fn deny(conn: &Connection, send: &mut iroh::endpoint::SendStream, reason: String) {
    let _ = write_msg(send, &PeerResp::Denied { reason }).await;
    let _ = send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(3), conn.closed()).await;
    conn.close(1u32.into(), b"denied");
}

/// Handles one incoming `toss/peer/1` connection.
pub(crate) async fn handle_peer(inner: Arc<Inner>, conn: Connection) {
    let remote = conn.remote_id();
    let rid = id_hex(remote);
    let short = remote.fmt_short().to_string();
    let first = tokio::time::timeout(xfer::pre_auth_timeout(), async {
        let (send, mut recv) = conn.accept_bi().await?;
        let req: PeerReq = read_msg_max(&mut recv, MAX_HELLO).await?;
        anyhow::Ok((send, recv, req))
    })
    .await;
    let (mut send, recv, req) = match first {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            tracing::debug!(remote = %short, "peer: no request: {e:#}");
            conn.close(1u32.into(), b"bad request");
            return;
        }
        Err(_) => {
            tracing::info!(remote = %short, "peer: no request within the pre-auth timeout");
            conn.close(1u32.into(), b"timeout");
            return;
        }
    };
    match req {
        PeerReq::Pair {
            secret,
            name,
            platform: plat,
            version,
        } => {
            let name = clean_text(&name, MAX_NAME);
            if let Err(reason) = inner.invites.redeem(&secret, &rid, &name) {
                tracing::info!(remote = %short, %name, "peer: pairing denied: {reason}");
                deny(&conn, &mut send, reason).await;
                return;
            }
            let hint = remote_hint(&inner, remote).await;
            let c = inner.contacts.upsert(&rid, &name, &plat, hint);
            tracing::info!(
                remote = %short,
                name = %c.name,
                version = %clean_text(&version, MAX_NAME),
                "paired with a new device (invite)"
            );
            inner.emit(TransferEvent::ContactsChanged {});
            let _live = LiveGuard::new(&inner, remote);
            let resp = PeerResp::Paired {
                name: inner.cfg.device_name.clone(),
                platform: platform(),
                version: inner.cfg.app_version.clone(),
            };
            let _ = write_msg(&mut send, &resp).await;
            let _ = send.finish();
            let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
        }
        _ if !inner.contacts.contains(&rid) => {
            tracing::info!(remote = %short, "peer: denied: not a contact");
            deny(
                &conn,
                &mut send,
                "máy này chưa có trong danh bạ — hãy kết nối lại bằng lời mời".into(),
            )
            .await;
        }
        PeerReq::Hello { name, .. } => {
            let _live = LiveGuard::new(&inner, remote);
            let name = clean_text(&name, MAX_NAME);
            if let Some(s) = inner.contacts.get(&rid) {
                if !name.is_empty() && s.c.their_name != name {
                    let _ = inner.contacts.update(&rid, |c| c.their_name = name);
                    inner.emit(TransferEvent::ContactsChanged {});
                }
            }
            inner
                .contacts
                .set_hint(&rid, remote_hint(&inner, remote).await);
            set_presence(&inner, &rid, true, conn_rtt_ms(&conn));
            let busy = inner
                .transfers
                .lock()
                .values()
                .any(|e| e.tracker.status() == Status::Running);
            let pong = PeerResp::Pong {
                name: inner.cfg.device_name.clone(),
                version: inner.cfg.app_version.clone(),
                busy,
            };
            let _ = write_msg(&mut send, &pong).await;
            let _ = send.finish();
            let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
        }
        PeerReq::Push {
            push_id,
            job,
            files,
            bytes,
        } => {
            let push = PushReq {
                push_id,
                job: clean_text(&job, MAX_JOB_LEN),
                files,
                bytes,
            };
            handle_push(inner, conn, send, recv, push).await;
        }
    }
}

struct PushReq {
    push_id: [u8; 16],
    job: String,
    files: u32,
    bytes: u64,
}

enum Answer {
    Accept,
    Decline(String),
    Gone,
}

/// Queues the push for the user (`PushQueued` + `PushIncoming`) and waits
/// for `answer_push`, the 10 min timeout or the sender going away.
async fn ask_user(
    inner: &Arc<Inner>,
    conn: &Connection,
    send: &mut iroh::endpoint::SendStream,
    contact: &Contact,
    push: &PushReq,
) -> Answer {
    let key = hex::encode(push.push_id);
    let token = inner.peer.next_token.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    let info = IncomingPush {
        push_id: key.clone(),
        from_id: contact.id.clone(),
        from_name: contact.name.clone(),
        job: push.job.clone(),
        files: push.files,
        bytes: push.bytes,
        at: now_unix(),
    };
    inner.peer.pending.lock().insert(
        key.clone(),
        Pending {
            info: info.clone(),
            token,
            answer: tx,
        },
    );
    tracing::info!(from = %contact.name, job = %push.job, "push queued for the user");
    inner.emit(TransferEvent::PushIncoming(info));
    let answer = if write_msg(send, &PeerResp::PushQueued).await.is_err() {
        Answer::Gone
    } else {
        tokio::select! {
            r = tokio::time::timeout(PUSH_ANSWER_TIMEOUT, rx) => match r {
                Ok(Ok(true)) => Answer::Accept,
                Ok(Ok(false)) => Answer::Decline("người nhận đã từ chối".into()),
                // Sender dropped: the contact was removed.
                Ok(Err(_)) => Answer::Decline("người nhận đã từ chối".into()),
                Err(_) => Answer::Decline("không ai trả lời trong 10 phút".into()),
            },
            _ = conn.closed() => Answer::Gone,
        }
    };
    let mut pending = inner.peer.pending.lock();
    if pending.get(&key).is_some_and(|p| p.token == token) {
        pending.remove(&key);
    }
    answer
}

async fn decline(conn: &Connection, send: &mut iroh::endpoint::SendStream, reason: String) {
    let _ = write_msg(send, &PeerResp::PushDeclined { reason }).await;
    let _ = send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(3), conn.closed()).await;
    conn.close(0u32.into(), b"declined");
}

/// Receiving side of a push from a contact.
async fn handle_push(
    inner: Arc<Inner>,
    conn: Connection,
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    push: PushReq,
) {
    let remote = conn.remote_id();
    let rid = id_hex(remote);
    let Some(contact) = inner.contacts.get(&rid).map(|s| s.c) else {
        return;
    };
    let _live = LiveGuard::new(&inner, remote);
    let pid = hex::encode(push.push_id);
    let auto = contact.auto_accept || inner.peer.accepted.lock().contains(&push.push_id);
    if !auto {
        match ask_user(&inner, &conn, &mut send, &contact, &push).await {
            Answer::Accept => {
                inner.peer.accepted.lock().insert(push.push_id);
            }
            Answer::Decline(reason) => {
                tracing::info!(from = %contact.name, job = %push.job, "push declined: {reason}");
                decline(&conn, &mut send, reason).await;
                return;
            }
            Answer::Gone => return,
        }
    }
    // <inbox>/<contact name>/<job>/
    let inbox = inner.peer.inbox_dir.read().clone();
    let contact_dir = sanitize_component(&contact.name, &format!("may-{}", &rid[..8]));
    let job_dir = sanitize_component(&push.job, &format!("push-{}", &pid[..8]));
    let prepared = tokio::task::spawn_blocking(move || -> Result<(PathBuf, PathBuf)> {
        std::fs::create_dir_all(&inbox).with_context(|| format!("create {}", inbox.display()))?;
        let root = canonicalize(&inbox)?;
        let dir = root.join(contact_dir).join(job_dir);
        Ok((root, dir))
    })
    .await
    .map_err(anyhow::Error::from)
    .and_then(|r| r);
    let (root_c, ret_dir) = match prepared {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("push: inbox folder unusable: {e:#}");
            decline(
                &conn,
                &mut send,
                "máy nhận không ghi được vào thư mục nhận".into(),
            )
            .await;
            return;
        }
    };
    if write_msg(&mut send, &PeerResp::PushAccepted).await.is_err() {
        return;
    }
    tracing::info!(
        from = %contact.name,
        job = %push.job,
        files = push.files,
        bytes = push.bytes,
        dest = %ret_dir.display(),
        "push accepted"
    );
    let tracker = Tracker::new(
        format!("{pid}-{}", random_suffix()),
        push.job.clone(),
        Op::Put,
        Side::Host,
        contact.name.clone(),
        Some(ret_dir.display().to_string()),
        inner.events.clone(),
    );
    tracker.update(|s| s.contact = Some(rid.clone()));
    inner.register(tracker.clone(), EntryKind::Host(conn.clone()));
    tracker.started();
    let ticker = tracker.spawn_ticker(conn.clone());
    let res = host::receive_put(
        &inner, &conn, &mut send, &mut recv, &root_c, &ret_dir, &tracker, true,
    )
    .await;
    ticker.abort();
    let path = conn_path(&conn);
    match res {
        Ok(done) if done.ok => tracker.finish_done(path),
        Ok(done) => {
            tracker.tick(path);
            tracker.error(ErrorCode::HashMismatch, done.msg, None);
        }
        Err(e) => {
            let code = match e {
                SessErr::Offline { .. } => ErrorCode::HostOffline,
                SessErr::Io(_) => ErrorCode::Io,
                SessErr::Denied(_) => ErrorCode::Denied,
                SessErr::Other(_) => ErrorCode::Other,
            };
            tracker.error(code, e.to_string(), None);
        }
    }
}
