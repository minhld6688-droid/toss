//! Transfer handles, events and state snapshots (core-api.md).

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use iroh::endpoint::Connection;
use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::{broadcast, watch};

use crate::{util::now_unix, wire};

/// `hex(ticket.id) + "-" + 4 random hex chars`.
pub type TransferId = String;

/// Every event of every transfer plus host status changes.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TransferEvent {
    /// A transfer was created (client) or accepted (host).
    Started {
        /// Transfer id.
        id: TransferId,
        /// Job name.
        job: String,
        /// Direction.
        op: wire::Op,
        /// Which side of the transfer this node is.
        side: Side,
        /// Device name or endpoint id of the other side.
        peer: String,
    },
    /// Manifest exchanged; `to_do_*` is what actually moves this session.
    Manifest {
        /// Transfer id.
        id: TransferId,
        /// Files in the manifest.
        files: u32,
        /// Bytes in the manifest.
        bytes: u64,
        /// Files still to move.
        to_do_files: u32,
        /// Bytes still to move.
        to_do_bytes: u64,
    },
    /// Periodic progress (≤ every 500 ms).
    Progress {
        /// Transfer id.
        id: TransferId,
        /// Bytes done (including already present ones).
        bytes_done: u64,
        /// Total bytes.
        bytes_total: u64,
        /// Files done.
        files_done: u32,
        /// Total files.
        files_total: u32,
        /// Speed, 5 s moving average.
        speed_bps: u64,
        /// Estimated seconds left.
        eta_s: Option<u64>,
        /// Connection path.
        path: ConnPath,
        /// v0.4: RTT of the selected path, ms (None = unknown).
        rtt_ms: Option<u32>,
        /// v0.4: data streams currently open.
        streams: u32,
    },
    /// Finished successfully.
    Done {
        /// Transfer id.
        id: TransferId,
        /// Files in the job.
        files: u32,
        /// Bytes moved in total over all sessions.
        bytes: u64,
        /// Seconds since start.
        elapsed_s: u64,
        /// Destination folder (client get / host put).
        dest: Option<String>,
    },
    /// Error; `retry_in_s` is set while the client keeps retrying.
    Error {
        /// Transfer id.
        id: TransferId,
        /// Category.
        code: ErrorCode,
        /// Details.
        msg: String,
        /// Next retry in seconds, if retrying.
        retry_in_s: Option<u64>,
    },
    /// Paused by the user.
    Paused {
        /// Transfer id.
        id: TransferId,
    },
    /// Host status changed (host mode only).
    HostStatus(wire::HostStatus),
}

/// Which side of a transfer this node is.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// Initiated the connection.
    Client,
    /// Serving.
    Host,
}

/// Network path of the connection.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnPath {
    /// Direct UDP.
    Direct,
    /// Through a relay.
    Relay,
    /// Both open, none selected yet.
    Mixed,
    /// Not connected / unknown.
    Unknown,
}

/// Error category.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Ticket refused (bad signature, path, root, not allowed).
    Denied,
    /// Ticket expired.
    Expired,
    /// Ticket revoked.
    Revoked,
    /// Host unreachable.
    HostOffline,
    /// Host busy.
    Busy,
    /// Local I/O failure.
    Io,
    /// Data failed verification.
    HashMismatch,
    /// Cancelled by the user.
    Cancelled,
    /// Anything else.
    Other,
}

/// Lifecycle status.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Dialing / waiting to retry.
    Connecting,
    /// Moving data.
    Running,
    /// Paused by the user.
    Paused,
    /// Finished.
    Done,
    /// Failed for good.
    Error,
    /// Cancelled.
    Cancelled,
}

impl Status {
    /// Done, Error or Cancelled.
    pub fn is_final(self) -> bool {
        matches!(self, Status::Done | Status::Error | Status::Cancelled)
    }
}

/// Snapshot for UIs.
#[derive(Clone, Debug, Serialize)]
pub struct TransferState {
    /// Transfer id.
    pub id: TransferId,
    /// Job name.
    pub job: String,
    /// Direction.
    pub op: wire::Op,
    /// Side.
    pub side: Side,
    /// Other side.
    pub peer: String,
    /// Status.
    pub status: Status,
    /// Bytes done.
    pub bytes_done: u64,
    /// Bytes total.
    pub bytes_total: u64,
    /// Files done.
    pub files_done: u32,
    /// Files total.
    pub files_total: u32,
    /// Speed (5 s average).
    pub speed_bps: u64,
    /// ETA.
    pub eta_s: Option<u64>,
    /// Connection path.
    pub path: ConnPath,
    /// v0.4: RTT of the selected path, ms.
    pub rtt_ms: Option<u32>,
    /// v0.4: data streams currently open.
    pub streams: u32,
    /// Destination folder.
    pub dest: Option<String>,
    /// Last error message.
    pub error: Option<String>,
    /// Unix seconds.
    pub started_at: u64,
}

/// Handle to a transfer.
#[derive(Clone, Debug)]
pub struct Transfer {
    /// The id; use with `Node::{pause,resume,cancel,transfer}`.
    pub id: TransferId,
    pub(crate) rx: watch::Receiver<TransferState>,
}

impl Transfer {
    /// Waits until the transfer is Done, Error or Cancelled and returns the final state.
    /// (A paused transfer keeps this pending until it is resumed or cancelled.)
    pub async fn wait(&self) -> TransferState {
        let mut rx = self.rx.clone();
        loop {
            {
                let s = rx.borrow_and_update();
                if s.status.is_final() {
                    return s.clone();
                }
            }
            if rx.changed().await.is_err() {
                return rx.borrow().clone();
            }
        }
    }

    /// Current snapshot.
    pub fn state(&self) -> TransferState {
        self.rx.borrow().clone()
    }
}

/// Maps an iroh connection's open paths to [`ConnPath`].
pub fn conn_path(conn: &Connection) -> ConnPath {
    let paths = conn.paths();
    let (mut ip, mut relay, mut selected) = (false, false, None);
    for p in paths.iter() {
        if p.is_selected() {
            selected = Some(if p.is_ip() {
                ConnPath::Direct
            } else if p.is_relay() {
                ConnPath::Relay
            } else {
                ConnPath::Unknown
            });
        }
        ip |= p.is_ip();
        relay |= p.is_relay();
    }
    match (selected, ip, relay) {
        (Some(s), _, _) => s,
        (None, true, true) => ConnPath::Mixed,
        (None, true, false) => ConnPath::Direct,
        (None, false, true) => ConnPath::Relay,
        _ => ConnPath::Unknown,
    }
}

/// RTT of the connection's selected path (or the best open path), in ms.
pub fn conn_rtt_ms(conn: &Connection) -> Option<u32> {
    let paths = conn.paths();
    let mut best: Option<std::time::Duration> = None;
    for p in paths.iter() {
        let rtt = p.rtt();
        if p.is_selected() {
            return Some(ms(rtt));
        }
        best = Some(best.map_or(rtt, |b| b.min(rtt)));
    }
    best.map(ms)
}

fn ms(d: Duration) -> u32 {
    u32::try_from(d.as_millis()).unwrap_or(u32::MAX)
}

/// Per-transfer bookkeeping shared by the worker task and the node.
#[derive(Debug)]
pub(crate) struct Tracker {
    pub id: TransferId,
    events: broadcast::Sender<TransferEvent>,
    state: watch::Sender<TransferState>,
    pub bytes_done: AtomicU64,
    pub files_done: AtomicU32,
    /// Bytes actually moved over the network (all sessions).
    pub moved: AtomicU64,
    /// Data streams currently open (v0.4 `streams`).
    pub streams: AtomicU32,
    started: Instant,
    samples: Mutex<VecDeque<(Instant, u64)>>,
}

impl Tracker {
    pub fn new(
        id: TransferId,
        job: String,
        op: wire::Op,
        side: Side,
        peer: String,
        dest: Option<String>,
        events: broadcast::Sender<TransferEvent>,
    ) -> Arc<Tracker> {
        let st = TransferState {
            id: id.clone(),
            job,
            op,
            side,
            peer,
            status: Status::Connecting,
            bytes_done: 0,
            bytes_total: 0,
            files_done: 0,
            files_total: 0,
            speed_bps: 0,
            eta_s: None,
            path: ConnPath::Unknown,
            rtt_ms: None,
            streams: 0,
            dest,
            error: None,
            started_at: now_unix(),
        };
        let (state, _) = watch::channel(st);
        Arc::new(Tracker {
            id,
            events,
            state,
            bytes_done: AtomicU64::new(0),
            files_done: AtomicU32::new(0),
            moved: AtomicU64::new(0),
            streams: AtomicU32::new(0),
            started: Instant::now(),
            samples: Mutex::new(VecDeque::new()),
        })
    }

    pub fn subscribe_state(&self) -> watch::Receiver<TransferState> {
        self.state.subscribe()
    }

    pub fn snapshot(&self) -> TransferState {
        self.state.borrow().clone()
    }

    pub fn emit(&self, ev: TransferEvent) {
        let _ = self.events.send(ev);
    }

    pub fn update(&self, f: impl FnOnce(&mut TransferState)) {
        self.state.send_modify(f);
    }

    pub fn status(&self) -> Status {
        self.state.borrow().status
    }

    pub fn started(&self) {
        let s = self.snapshot();
        self.emit(TransferEvent::Started {
            id: s.id,
            job: s.job,
            op: s.op,
            side: s.side,
            peer: s.peer,
        });
    }

    /// True once cancelled: a cancelled transfer never changes state again
    /// (a session ending with an error after the cancel must not turn it into
    /// `Error` or emit a second final event).
    fn is_cancelled(&self) -> bool {
        self.status() == Status::Cancelled
    }

    /// Called after the manifest/want exchange of a session.
    pub fn begin_session(&self, files: u32, bytes: u64, to_do_files: u32, to_do_bytes: u64) {
        if self.is_cancelled() {
            return;
        }
        let done_bytes = bytes.saturating_sub(to_do_bytes);
        let done_files = files.saturating_sub(to_do_files);
        self.bytes_done.store(done_bytes, Ordering::Relaxed);
        self.files_done.store(done_files, Ordering::Relaxed);
        self.samples.lock().clear();
        self.update(|s| {
            s.status = Status::Running;
            s.files_total = files;
            s.bytes_total = bytes;
            s.bytes_done = done_bytes;
            s.files_done = done_files;
            s.error = None;
        });
        self.emit(TransferEvent::Manifest {
            id: self.id.clone(),
            files,
            bytes,
            to_do_files,
            to_do_bytes,
        });
    }

    pub fn add_bytes(&self, n: u64) {
        self.bytes_done.fetch_add(n, Ordering::Relaxed);
        self.moved.fetch_add(n, Ordering::Relaxed);
    }

    /// Undo bytes of a file that failed verification.
    pub fn sub_bytes(&self, n: u64) {
        self.bytes_done
            .update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                v.saturating_sub(n)
            });
    }

    pub fn file_done(&self) {
        self.files_done.fetch_add(1, Ordering::Relaxed);
    }

    /// Samples counters, updates state and emits a Progress event.
    pub fn tick(&self, path: ConnPath) {
        let now = Instant::now();
        let done = self.bytes_done.load(Ordering::Relaxed);
        let files_done = self.files_done.load(Ordering::Relaxed);
        let speed = {
            let mut samples = self.samples.lock();
            samples.push_back((now, done));
            while samples
                .front()
                .is_some_and(|(t, _)| now.duration_since(*t) > Duration::from_secs(5))
            {
                samples.pop_front();
            }
            match (samples.front(), samples.back()) {
                (Some((t0, b0)), Some((t1, b1))) if t1 > t0 => {
                    let dt = t1.duration_since(*t0).as_secs_f64();
                    (b1.saturating_sub(*b0) as f64 / dt) as u64
                }
                _ => 0,
            }
        };
        let streams = self.streams.load(Ordering::Relaxed);
        let mut ev = None;
        self.update(|s| {
            s.bytes_done = done;
            s.files_done = files_done;
            s.streams = streams;
            s.speed_bps = speed;
            s.path = path;
            let left = s.bytes_total.saturating_sub(done);
            s.eta_s = if left == 0 {
                Some(0)
            } else if speed > 0 {
                Some(left.div_ceil(speed))
            } else {
                None
            };
            ev = Some(TransferEvent::Progress {
                id: s.id.clone(),
                bytes_done: s.bytes_done,
                bytes_total: s.bytes_total,
                files_done: s.files_done,
                files_total: s.files_total,
                speed_bps: s.speed_bps,
                eta_s: s.eta_s,
                path: s.path,
                rtt_ms: s.rtt_ms,
                streams: s.streams,
            });
        });
        if let Some(ev) = ev {
            self.emit(ev);
        }
    }

    /// Spawns the 500 ms progress ticker for a connection; abort it when the session ends.
    pub fn spawn_ticker(self: &Arc<Self>, conn: Connection) -> tokio::task::JoinHandle<()> {
        let me = self.clone();
        tokio::spawn(async move {
            let mut iv = tokio::time::interval(Duration::from_millis(500));
            iv.tick().await;
            loop {
                iv.tick().await;
                me.set_rtt(conn_rtt_ms(&conn));
                me.tick(conn_path(&conn));
            }
        })
    }

    /// Records the RTT of the selected path (shown in the next Progress).
    pub fn set_rtt(&self, rtt_ms: Option<u32>) {
        self.update(|s| {
            if rtt_ms.is_some() {
                s.rtt_ms = rtt_ms;
            }
        });
    }

    /// Counts an open data stream until the guard is dropped.
    pub fn stream_open(self: &Arc<Self>) -> StreamGuard {
        self.streams.fetch_add(1, Ordering::Relaxed);
        StreamGuard(self.clone())
    }

    /// Final Progress + Done.
    pub fn finish_done(&self, path: ConnPath) {
        if self.is_cancelled() {
            return;
        }
        self.tick(path);
        let mut ev = None;
        self.update(|s| {
            if s.status == Status::Cancelled {
                return;
            }
            s.status = Status::Done;
            s.error = None;
            s.eta_s = Some(0);
            ev = Some(TransferEvent::Done {
                id: s.id.clone(),
                files: s.files_total,
                bytes: s.bytes_total,
                elapsed_s: self.started.elapsed().as_secs(),
                dest: s.dest.clone(),
            });
        });
        if let Some(ev) = ev {
            self.emit(ev);
        }
    }

    /// Emits an Error event; `retry_in_s` = Some keeps status Connecting.
    pub fn error(&self, code: ErrorCode, msg: String, retry_in_s: Option<u64>) {
        let mut changed = false;
        self.update(|s| {
            if s.status == Status::Cancelled {
                return;
            }
            changed = true;
            s.status = match (code, retry_in_s) {
                (_, Some(_)) => Status::Connecting,
                (ErrorCode::Cancelled, None) => Status::Cancelled,
                _ => Status::Error,
            };
            s.error = Some(msg.clone());
            s.speed_bps = 0;
            s.eta_s = None;
        });
        if !changed {
            return;
        }
        self.emit(TransferEvent::Error {
            id: self.id.clone(),
            code,
            msg,
            retry_in_s,
        });
    }

    pub fn paused(&self) {
        let mut changed = false;
        self.update(|s| {
            if s.status == Status::Cancelled {
                return;
            }
            changed = true;
            s.status = Status::Paused;
            s.speed_bps = 0;
            s.eta_s = None;
        });
        if !changed {
            return;
        }
        self.emit(TransferEvent::Paused {
            id: self.id.clone(),
        });
    }

    pub fn set_status(&self, st: Status) {
        self.update(|s| {
            if s.status != Status::Cancelled {
                s.status = st;
            }
        });
    }
}

/// See [`Tracker::stream_open`].
pub(crate) struct StreamGuard(Arc<Tracker>);

impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.0.streams.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker() -> (Arc<Tracker>, broadcast::Receiver<TransferEvent>) {
        let (tx, rx) = broadcast::channel(16);
        let t = Tracker::new(
            "id".into(),
            "job".into(),
            wire::Op::Get,
            Side::Host,
            "peer".into(),
            None,
            tx,
        );
        (t, rx)
    }

    #[test]
    fn cancelled_is_terminal() {
        let (t, mut rx) = tracker();
        t.begin_session(2, 10, 2, 10);
        t.error(ErrorCode::Cancelled, "cancelled".into(), None);
        // The session then fails because its connection was closed.
        t.error(ErrorCode::HostOffline, "connection lost".into(), None);
        t.finish_done(ConnPath::Direct);
        t.paused();
        t.set_status(Status::Connecting);
        assert_eq!(t.status(), Status::Cancelled);
        let mut finals = 0;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                TransferEvent::Error { code, .. } => {
                    assert_eq!(code, ErrorCode::Cancelled);
                    finals += 1;
                }
                TransferEvent::Done { .. } | TransferEvent::Paused { .. } => {
                    panic!("event after cancel: {ev:?}")
                }
                _ => {}
            }
        }
        assert_eq!(finals, 1);
    }

    #[test]
    fn eta_and_speed_without_progress() {
        let (t, _rx) = tracker();
        t.begin_session(1, 100, 1, 100);
        t.tick(ConnPath::Direct);
        let s = t.snapshot();
        assert_eq!(s.speed_bps, 0);
        assert_eq!(s.eta_s, None);
        t.add_bytes(100);
        t.tick(ConnPath::Direct);
        assert_eq!(t.snapshot().eta_s, Some(0));
    }
}
