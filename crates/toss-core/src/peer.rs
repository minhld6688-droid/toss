//! ALPN `toss/peer/1` (contracts/contacts-v0.5.md §2.2): pairing with an
//! invite, presence pings between contacts and pushes from contacts.
//!
//! Every node accepts it. The first bi stream carries one [`PeerReq`] and
//! its [`PeerResp`] (postcard, u32-LE length prefix, ≤ 64 KiB). A contact is
//! authenticated by `conn.remote_id()` (QUIC/TLS) against `contacts.json`;
//! anything but `Pair` from a stranger is denied. After `PushAccepted` the
//! same stream and connection carry xfer/2 put semantics.
//!
//! v0.8 (nearby-v0.8.md §1.2): a stranger that this node sees on its LAN
//! (nearby store) may also send `PushFrom` and `Text`; the receive policy
//! decides which pushes are accepted without asking.

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
    nearby::{NearbyPeer, ReceivePolicy},
    node::{EntryKind, Inner},
    paths::{canonicalize, sanitize_component},
    ticket::{HostAddr, MAX_JOB_LEN},
    transfer::{conn_path, conn_rtt_ms, ErrorCode, Side, Status, Tracker, TransferEvent},
    util::{clean_message, clean_text, now_unix, random_suffix},
    vi::ui,
    wire::{read_msg_max, write_msg, Op, PeerReq, PeerResp, MAX_HELLO, MAX_TEXT},
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

/// A push waiting for the user.
#[derive(Debug)]
struct Pending {
    info: IncomingPush,
    /// Sender platform (a stranger answered with "always" becomes a contact).
    platform: String,
    token: u64,
    /// (accept, always)
    answer: oneshot::Sender<(bool, bool)>,
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
    pub kick: Arc<Notify>,
    /// Live peer connections per contact (a contact with one is online).
    live: Mutex<HashMap<EndpointId, usize>>,
    /// Runtime for work started from sync API calls.
    rt: tokio::runtime::Handle,
    /// v0.8: who is accepted without asking.
    pub policy: RwLock<ReceivePolicy>,
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
            policy: RwLock::new(ReceivePolicy::default()),
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

/// `Node::answer_push`: `always` on a contact turns its `auto_accept` on; on
/// a stranger it adds it as a contact (auto-accept) and the push is answered
/// `PushAcceptedPaired` so that the sender adds this device too.
pub(crate) fn answer_push(
    inner: &Arc<Inner>,
    push_id: &str,
    accept: bool,
    always: bool,
) -> Result<()> {
    let p = inner
        .peer
        .pending
        .lock()
        .remove(&push_id.trim().to_ascii_lowercase())
        .ok_or_else(|| ui("Không còn yêu cầu gửi file này (đã hết hạn hoặc đã trả lời)"))?;
    if accept && always {
        let id = p.info.from_id.clone();
        if inner.contacts.contains(&id) {
            let _ = inner.contacts.update(&id, |c| c.auto_accept = true);
        } else {
            let hint = nearby_hint(inner, &id);
            inner
                .contacts
                .upsert(&id, &p.info.from_name, &p.platform, hint);
            let _ = inner.contacts.update(&id, |c| c.auto_accept = true);
            tracing::info!(contact = %id, name = %p.info.from_name, "nearby device added to contacts (always accept)");
            set_presence(inner, &id, true, None);
        }
        inner.emit(TransferEvent::ContactsChanged {});
    }
    let _ = p.answer.send((accept, always));
    Ok(())
}

/// Direct addresses of a nearby device as a dialing hint.
fn nearby_hint(inner: &Inner, id: &str) -> HostAddr {
    HostAddr {
        relay: String::new(),
        addrs: inner.nearby.get(id).map(|p| p.addrs).unwrap_or_default(),
    }
}

/// Display name of a nearby device: its advertised name, the contact name,
/// or "Máy xxxxxx".
pub(crate) fn nearby_name(p: &NearbyPeer) -> String {
    if !p.name.trim().is_empty() {
        return p.name.clone();
    }
    format!("Máy {}", &p.id[..p.id.len().min(6)])
}

/// The nearby store changed: event + wake pushes waiting for a peer.
pub(crate) fn nearby_changed(inner: &Inner) {
    inner.emit(TransferEvent::NearbyChanged {});
    inner.peer.bump();
}

/// mDNS saw `peer` (announcement or lookup answer). A contact seen nearby is
/// online at once.
pub(crate) fn nearby_seen(inner: &Inner, peer: NearbyPeer) {
    if peer.id == id_hex(inner.endpoint.id()) {
        return;
    }
    let id = peer.id.clone();
    let changed = inner.nearby.upsert(peer);
    if inner.contacts.contains(&id) {
        set_presence(inner, &id, true, None);
    }
    if changed {
        nearby_changed(inner);
    }
}

/// mDNS expired these ids (or they went stale and were swept already).
pub(crate) fn nearby_gone(inner: &Inner, ids: &[String]) {
    let mut any = false;
    for id in ids {
        any |= inner.nearby.remove(id);
    }
    if any {
        nearby_changed(inner);
    }
    // A contact that left the LAN: let presence decide (it may be on the relay).
    if ids.iter().any(|id| inner.contacts.contains(id)) {
        inner.peer.kick.notify_one();
    }
}

/// `Node::nearby`: the store with `contact` filled in (self excluded).
pub(crate) fn nearby_list(inner: &Inner) -> Vec<NearbyPeer> {
    let me = id_hex(inner.endpoint.id());
    inner
        .nearby
        .list()
        .into_iter()
        .filter(|p| p.id != me)
        .map(|mut p| {
            if let Some(c) = inner.contacts.get(&p.id) {
                p.contact = true;
                if p.name.is_empty() {
                    p.name = c.c.name;
                }
                if p.platform.is_empty() {
                    p.platform = c.c.platform;
                }
            } else if p.name.is_empty() {
                p.name = nearby_name(&p);
            }
            p
        })
        .collect()
}

/// Records a presence result and emits `ContactPresence` when it changed.
/// "Offline" is ignored while a connection with the contact is alive. (A
/// contact seen by mDNS is set online at once, but a failed ping still wins:
/// the mDNS cache keeps a device for a while after it quit.)
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

/// Waits until push target `id` can be dialed: a contact that is online
/// ("Chờ <name> online" meanwhile; mDNS sets a contact online as soon as it
/// sees it) or a non-contact in the nearby store. Err when
/// a contact push lost its contact (removed) and the device is not nearby;
/// a push to a stranger (`stranger`) waits for it to show up on the LAN.
pub(crate) async fn wait_reachable(
    inner: &Arc<Inner>,
    id: EndpointId,
    tracker: &Tracker,
    stranger: bool,
) -> Result<(), String> {
    let hex = id_hex(id);
    let mut gen = inner.peer.online_gen.subscribe();
    let mut announced: Option<String> = None;
    loop {
        let nearby = inner.nearby.contains(&hex);
        let msg = match inner.contacts.get(&hex) {
            Some(c) if c.c.online => return Ok(()),
            Some(c) => format!("Chờ {} online", c.c.name),
            None if nearby => return Ok(()),
            None if !stranger => return Err("Máy nhận đã bị xoá khỏi danh bạ".into()),
            None => format!(
                "Chờ {} mở Toss trong cùng mạng LAN",
                tracker.snapshot().peer
            ),
        };
        if announced.as_ref() != Some(&msg) {
            // Maybe it just came up: check now instead of at the next round.
            inner.peer.kick.notify_one();
            tracker.error(
                ErrorCode::HostOffline,
                msg.clone(),
                Some(PRESENCE_INTERVAL.as_secs()),
            );
            announced = Some(msg);
        }
        if gen.changed().await.is_err() {
            return Err("Toss đang tắt".into());
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
    dial_peer(inner, id).await
}

/// Dials a contact or nearby device on `toss/peer/1`: the addresses mDNS saw
/// on the LAN first, then the contact's last known relay/addresses.
pub(crate) async fn dial_peer(inner: &Inner, id: EndpointId) -> Result<Connection> {
    let hex = id_hex(id);
    let mut hint = inner.contacts.get(&hex).map(|s| s.addr).unwrap_or_default();
    if let Some(n) = inner.nearby.get(&hex) {
        let mut addrs = n.addrs;
        for a in hint.addrs {
            if !addrs.contains(&a) {
                addrs.push(a);
            }
        }
        hint.addrs = addrs;
    }
    inner
        .connect_alpns(addr_from_hint(id, &hint), ALPN_PEER, Vec::new())
        .await
}

/// After a v0.8 request (`PushFrom`, `Text`) got no answer: true when the
/// peer closed the connection because it could not decode it (Toss < 0.8).
pub(crate) async fn refused_as_old(conn: &Connection) -> bool {
    let _ = tokio::time::timeout(Duration::from_secs(2), conn.closed()).await;
    matches!(
        conn.close_reason(),
        Some(iroh::endpoint::ConnectionError::ApplicationClosed(ref c))
            if &c.reason[..] == b"bad request"
    )
}

/// The receiver of our `PushFrom` answered `PushAcceptedPaired`: add it as a
/// contact too (both sides now trust each other).
pub(crate) async fn pair_back(inner: &Arc<Inner>, conn: &Connection, name: &str, plat: &str) {
    let remote = conn.remote_id();
    let rid = id_hex(remote);
    let mut hint = remote_hint(inner, remote).await;
    if hint.addrs.is_empty() {
        hint = nearby_hint(inner, &rid);
    }
    let name = if name.trim().is_empty() {
        inner
            .nearby
            .get(&rid)
            .map(|p| nearby_name(&p))
            .unwrap_or_default()
    } else {
        name.to_string()
    };
    inner.contacts.upsert(&rid, &name, plat, hint);
    tracing::info!(contact = %remote.fmt_short(), %name, "paired back (receiver chose always accept)");
    inner.emit(TransferEvent::ContactsChanged {});
    set_presence(inner, &rid, true, conn_rtt_ms(conn));
}

/// `Node::send_text`: a short message to a contact or a nearby device.
pub(crate) async fn send_text(inner: &Arc<Inner>, id: &str, text: &str) -> Result<()> {
    let hex = id.trim().to_ascii_lowercase();
    let eid = parse_endpoint_id(&hex)?;
    if eid == inner.endpoint.id() {
        return Err(ui("Không thể gửi cho chính máy này"));
    }
    if text.trim().is_empty() {
        return Err(ui("Tin nhắn trống"));
    }
    if text.len() > MAX_TEXT {
        return Err(ui(format!(
            "Tin nhắn quá dài (tối đa {} KB)",
            MAX_TEXT / 1024
        )));
    }
    let contact = inner.contacts.contains(&hex);
    if !contact && !inner.nearby.contains(&hex) {
        return Err(ui(
            "Không thấy máy này trong mạng LAN nữa — kiểm tra máy kia còn mở Toss và cùng mạng",
        ));
    }
    let conn = tokio::time::timeout(PING_DIAL_TIMEOUT + PING_DIAL_TIMEOUT, dial_peer(inner, eid))
        .await
        .map_err(|_| anyhow!("timeout"))
        .and_then(|r| r)
        .map_err(|e| {
            tracing::info!(to = %eid.fmt_short(), "text: dial failed: {e:#}");
            ui("Không kết nối được tới máy kia")
        })?;
    let res = async {
        let (mut send, mut recv) = conn.open_bi().await?;
        let req = PeerReq::Text {
            name: inner.device_name().to_string(),
            text: text.to_string(),
        };
        write_msg(&mut send, &req).await?;
        let resp: PeerResp =
            tokio::time::timeout(ANSWER_TIMEOUT, read_msg_max(&mut recv, MAX_HELLO))
                .await
                .map_err(|_| anyhow!("no answer"))??;
        anyhow::Ok(resp)
    }
    .await;
    let resp = match res {
        Ok(r) => r,
        Err(e) => {
            let old = refused_as_old(&conn).await;
            conn.close(0u32.into(), b"text");
            tracing::info!(to = %eid.fmt_short(), "text: no answer: {e:#}");
            return Err(ui(if old {
                crate::vi::OLD_PEER
            } else {
                "Máy kia không trả lời"
            }));
        }
    };
    let rtt = conn_rtt_ms(&conn);
    conn.close(0u32.into(), b"text");
    match resp {
        PeerResp::TextOk => {
            if contact {
                set_presence(inner, &hex, true, rtt);
            }
            tracing::info!(to = %eid.fmt_short(), bytes = text.len(), "text message delivered");
            Ok(())
        }
        PeerResp::Denied { reason } => Err(ui(clean_text(&reason, 300))),
        other => {
            tracing::info!("text: unexpected answer {other:?}");
            Err(ui(crate::vi::PROTOCOL))
        }
    }
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
        .map_err(|e| {
            tracing::info!("pairing: dial failed: {e:#}");
            ui(format!(
                "Không nối được tới {} — kiểm tra máy đó đang mở Toss và có mạng",
                inv.body.name
            ))
        })?;
    let res = async {
        let (mut send, mut recv) = conn.open_bi().await?;
        let req = PeerReq::Pair {
            secret: inv.body.secret,
            name: inner.device_name().to_string(),
            platform: platform(),
            version: inner.cfg.app_version.clone(),
        };
        write_msg(&mut send, &req).await?;
        let resp: PeerResp =
            tokio::time::timeout(ANSWER_TIMEOUT, read_msg_max(&mut recv, MAX_HELLO))
                .await
                .map_err(|_| ui("Máy kia không trả lời"))??;
        anyhow::Ok(resp)
    }
    .await;
    let rtt = conn_rtt_ms(&conn);
    conn.close(0u32.into(), b"paired");
    let res = res.map_err(|e| {
        if crate::vi::ui_text(&e).is_some() {
            e
        } else {
            tracing::info!("pairing: {e:#}");
            ui("Mất kết nối khi đang kết nối hai máy — thử lại")
        }
    });
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
                .ok_or_else(|| ui("Máy vừa kết nối đã bị xoá khỏi danh bạ"))
        }
        PeerResp::Denied { reason } => Err(ui(format!(
            "{} từ chối kết nối: {}",
            inv.body.name,
            clean_text(&reason, 300)
        ))),
        other => {
            tracing::info!("pairing: unexpected answer {other:?}");
            Err(ui(crate::vi::PROTOCOL))
        }
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
            name: inner.device_name().to_string(),
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
                name: inner.device_name().to_string(),
                platform: platform(),
                version: inner.cfg.app_version.clone(),
            };
            let _ = write_msg(&mut send, &resp).await;
            let _ = send.finish();
            let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
        }
        PeerReq::PushFrom {
            push_id,
            job,
            files,
            bytes,
            name,
            platform: plat,
            version,
        } if inner.contacts.contains(&rid) || inner.nearby.contains(&rid) => {
            let push = PushReq {
                push_id,
                job: clean_text(&job, MAX_JOB_LEN),
                files,
                bytes,
                from: Some(PushOrigin {
                    name: clean_text(&name, MAX_NAME),
                    platform: clean_text(&plat, 16),
                    version: clean_text(&version, 32),
                }),
            };
            handle_push(inner, conn, send, recv, push).await;
        }
        PeerReq::Text { name, text }
            if inner.contacts.contains(&rid) || inner.nearby.contains(&rid) =>
        {
            let from_name = match inner.contacts.get(&rid) {
                Some(c) => c.c.name,
                None => {
                    let n = clean_text(&name, MAX_NAME);
                    if n.trim().is_empty() {
                        inner
                            .nearby
                            .get(&rid)
                            .map(|p| nearby_name(&p))
                            .unwrap_or_default()
                    } else {
                        n
                    }
                }
            };
            let text = clean_message(&text, MAX_TEXT);
            tracing::info!(from = %from_name, bytes = text.len(), "text message received");
            inner.emit(TransferEvent::TextIncoming {
                from_id: rid.clone(),
                from_name,
                text,
                at: now_unix(),
            });
            let _ = write_msg(&mut send, &PeerResp::TextOk).await;
            let _ = send.finish();
            let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
        }
        PeerReq::PushFrom { .. } | PeerReq::Text { .. } => {
            tracing::info!(remote = %short, "peer: denied: not a contact and not on this LAN");
            deny(
                &conn,
                &mut send,
                "máy này chưa có trong danh bạ và không ở cùng mạng LAN — hãy kết nối bằng lời mời"
                    .into(),
            )
            .await;
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
                name: inner.device_name().to_string(),
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
                from: None,
            };
            handle_push(inner, conn, send, recv, push).await;
        }
    }
}

/// Who sent a `PushFrom` (cleaned).
struct PushOrigin {
    name: String,
    platform: String,
    #[allow(dead_code)]
    version: String,
}

struct PushReq {
    push_id: [u8; 16],
    job: String,
    files: u32,
    bytes: u64,
    /// `PushFrom` (v0.8); None = `Push` from a contact.
    from: Option<PushOrigin>,
}

enum Answer {
    /// Accepted ("always" is applied by `answer_push`).
    Accept,
    Decline(String),
    Gone,
}

/// The sender of a push as the receiver shows it.
struct PushSender {
    id: String,
    name: String,
    platform: String,
    stranger: bool,
}

/// Queues the push for the user (`PushQueued` + `PushIncoming`) and waits
/// for `answer_push`, the 10 min timeout or the sender going away.
async fn ask_user(
    inner: &Arc<Inner>,
    conn: &Connection,
    send: &mut iroh::endpoint::SendStream,
    sender: &PushSender,
    push: &PushReq,
) -> Answer {
    let key = hex::encode(push.push_id);
    let token = inner.peer.next_token.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    let info = IncomingPush {
        push_id: key.clone(),
        from_id: sender.id.clone(),
        from_name: sender.name.clone(),
        job: push.job.clone(),
        files: push.files,
        bytes: push.bytes,
        at: now_unix(),
        stranger: sender.stranger,
    };
    inner.peer.pending.lock().insert(
        key.clone(),
        Pending {
            info: info.clone(),
            platform: sender.platform.clone(),
            token,
            answer: tx,
        },
    );
    tracing::info!(from = %sender.name, job = %push.job, stranger = sender.stranger, "push queued for the user");
    inner.emit(TransferEvent::PushIncoming(info));
    let answer = if write_msg(send, &PeerResp::PushQueued).await.is_err() {
        Answer::Gone
    } else {
        tokio::select! {
            r = tokio::time::timeout(PUSH_ANSWER_TIMEOUT, rx) => match r {
                Ok(Ok((true, _))) => Answer::Accept,
                Ok(Ok((false, _))) => Answer::Decline("người nhận đã từ chối".into()),
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

/// Receiving side of a push from a contact (`Push` / `PushFrom`) or from a
/// nearby stranger (`PushFrom`; the caller checked the nearby store).
async fn handle_push(
    inner: Arc<Inner>,
    conn: Connection,
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    push: PushReq,
) {
    let remote = conn.remote_id();
    let rid = id_hex(remote);
    let contact: Option<Contact> = inner.contacts.get(&rid).map(|s| s.c);
    let sender = match (&contact, &push.from) {
        (Some(c), _) => PushSender {
            id: rid.clone(),
            name: c.name.clone(),
            platform: c.platform.clone(),
            stranger: false,
        },
        (None, Some(f)) => {
            let name = if f.name.trim().is_empty() {
                inner
                    .nearby
                    .get(&rid)
                    .map(|p| nearby_name(&p))
                    .unwrap_or_else(|| format!("Máy {}", &rid[..6]))
            } else {
                f.name.clone()
            };
            PushSender {
                id: rid.clone(),
                name,
                platform: f.platform.clone(),
                stranger: true,
            }
        }
        (None, None) => return,
    };
    let _live = LiveGuard::new(&inner, remote);
    let pid = hex::encode(push.push_id);
    let policy = *inner.peer.policy.read();
    let auto = policy.auto_accepts(contact.as_ref().map(|c| c.auto_accept))
        || inner.peer.accepted.lock().contains(&push.push_id);
    if !auto {
        match ask_user(&inner, &conn, &mut send, &sender, &push).await {
            Answer::Accept => {
                inner.peer.accepted.lock().insert(push.push_id);
            }
            Answer::Decline(reason) => {
                tracing::info!(from = %sender.name, job = %push.job, "push declined: {reason}");
                decline(&conn, &mut send, reason).await;
                return;
            }
            Answer::Gone => return,
        }
    }
    // <inbox>/<sender name>/<job>/ (a stranger answered "always" is a
    // contact by now: use its contact name).
    let shown = inner
        .contacts
        .get(&rid)
        .map(|s| s.c.name)
        .unwrap_or_else(|| sender.name.clone());
    let inbox = inner.peer.inbox_dir.read().clone();
    let contact_dir = sanitize_component(&shown, &format!("may-{}", &rid[..8]));
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
    // v0.8: a `PushFrom` sender that is (now) our contact pairs back.
    let accepted = if push.from.is_some() && inner.contacts.contains(&rid) {
        PeerResp::PushAcceptedPaired {
            name: inner.device_name().to_string(),
            platform: platform(),
            version: inner.cfg.app_version.clone(),
        }
    } else {
        PeerResp::PushAccepted
    };
    if write_msg(&mut send, &accepted).await.is_err() {
        return;
    }
    tracing::info!(
        from = %shown,
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
        shown.clone(),
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
            tracing::warn!(id = %tracker.id, "push incomplete: {}", done.msg);
            tracker.error(ErrorCode::HashMismatch, crate::vi::DONE_FAILED.into(), None);
        }
        Err(e) => {
            let code = match e {
                SessErr::Offline { .. } => ErrorCode::HostOffline,
                SessErr::Io(_) => ErrorCode::Io,
                SessErr::Denied(_) => ErrorCode::Denied,
                SessErr::Other(_) => ErrorCode::Other,
            };
            tracing::info!(id = %tracker.id, "push receive failed: {e}");
            tracker.error(code, crate::vi::sess_msg(&e), None);
        }
    }
}
