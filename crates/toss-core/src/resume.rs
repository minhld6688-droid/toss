//! v0.5 transfer persistence (contracts/contacts-v0.5.md §2.3):
//! `<data_dir>/transfers.json` keeps every client transfer (get/put via
//! ticket, pushes to contacts), written atomically on each status change, so
//! that `Node::start` can restore the list and resume what was running.

use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::{
    jobs::write_atomic,
    transfer::{Status, Tracker},
};

/// Finished transfers kept in the file (oldest dropped).
pub const MAX_FINISHED: usize = 50;

/// What kind of client transfer.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Download with a ticket.
    Get,
    /// Upload with a ticket.
    Put,
    /// Push to a contact.
    Push,
}

/// One persisted client transfer.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TransferRecord {
    /// Transfer id.
    pub id: String,
    /// Kind.
    pub kind: Kind,
    /// get/put: the ticket string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
    /// push: contact id (hex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
    /// put/push: the selected files/folders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<PathBuf>>,
    /// get: the parent folder (files land in `dest/<job>/`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest: Option<PathBuf>,
    /// Job name.
    pub job: String,
    /// Last status.
    pub status: Status,
    /// Bytes done at the last status change.
    pub bytes_done: u64,
    /// Bytes total (0 = unknown yet).
    pub bytes_total: u64,
    /// Unix seconds.
    pub created: u64,
    /// push: hex(push id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_id: Option<String>,
    /// Display name of the other side.
    #[serde(default)]
    pub peer: String,
    /// Files total / done (display).
    #[serde(default)]
    pub files_total: u32,
    /// Files done.
    #[serde(default)]
    pub files_done: u32,
    /// Last error message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// v0.8 push: the receiver was not a contact when the push started (a
    /// nearby device, `contact` holds its endpoint id).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stranger: bool,
}

/// File-backed list of client transfers.
#[derive(Debug)]
pub(crate) struct TransferStore {
    path: PathBuf,
    map: Mutex<BTreeMap<String, TransferRecord>>,
    /// Set by `Node::shutdown`: the pauses it causes are not written, so
    /// running transfers resume on the next start.
    frozen: AtomicBool,
    /// Ids removed on purpose (a late status write must not bring them back).
    forgotten: Mutex<HashSet<String>>,
}

impl TransferStore {
    pub fn open(data_dir: &Path) -> TransferStore {
        let path = data_dir.join("transfers.json");
        let list: Vec<TransferRecord> = match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
                tracing::warn!("transfers.json is damaged, starting with an empty list: {e}");
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        TransferStore {
            path,
            map: Mutex::new(list.into_iter().map(|r| (r.id.clone(), r)).collect()),
            frozen: AtomicBool::new(false),
            forgotten: Mutex::new(HashSet::new()),
        }
    }

    /// All records, oldest first.
    pub fn records(&self) -> Vec<TransferRecord> {
        let mut v: Vec<_> = self.map.lock().values().cloned().collect();
        v.sort_by(|a, b| (a.created, &a.id).cmp(&(b.created, &b.id)));
        v
    }

    pub fn upsert(&self, rec: TransferRecord) {
        if self.frozen.load(Ordering::SeqCst) || self.forgotten.lock().contains(&rec.id) {
            return;
        }
        let mut map = self.map.lock();
        map.insert(rec.id.clone(), rec);
        self.save_locked(&mut map);
    }

    /// Removes records for good.
    pub fn remove(&self, ids: &[String]) {
        if ids.is_empty() {
            return;
        }
        self.forgotten.lock().extend(ids.iter().cloned());
        let mut map = self.map.lock();
        for id in ids {
            map.remove(id);
        }
        self.save_locked(&mut map);
    }

    /// Stops writing (see `frozen`).
    pub fn freeze(&self) {
        self.frozen.store(true, Ordering::SeqCst);
    }

    /// Keeps every unfinished record and the newest [`MAX_FINISHED`] finished ones.
    fn save_locked(&self, map: &mut BTreeMap<String, TransferRecord>) {
        let mut finished: Vec<(u64, String)> = map
            .values()
            .filter(|r| r.status.is_final())
            .map(|r| (r.created, r.id.clone()))
            .collect();
        if finished.len() > MAX_FINISHED {
            finished.sort();
            let drop = finished.len() - MAX_FINISHED;
            for (_, id) in finished.into_iter().take(drop) {
                map.remove(&id);
            }
        }
        let mut v: Vec<&TransferRecord> = map.values().collect();
        v.sort_by(|a, b| (a.created, &a.id).cmp(&(b.created, &b.id)));
        let res = serde_json::to_vec_pretty(&v)
            .map_err(anyhow::Error::from)
            .and_then(|b| write_atomic(&self.path, &b));
        if let Err(e) = res {
            tracing::warn!("could not save transfers.json: {e:#}");
        }
    }
}

/// Writes `rec` now and again whenever the transfer's status changes, until
/// the tracker is dropped.
pub(crate) fn track(store: &Arc<TransferStore>, mut rec: TransferRecord, tracker: &Arc<Tracker>) {
    let fill = |rec: &mut TransferRecord, s: &crate::TransferState| {
        rec.status = s.status;
        rec.bytes_done = s.bytes_done;
        rec.bytes_total = s.bytes_total;
        rec.files_done = s.files_done;
        rec.files_total = s.files_total;
        rec.error = s.error.clone();
    };
    let mut rx = tracker.subscribe_state();
    let first = rx.borrow_and_update().clone();
    fill(&mut rec, &first);
    store.upsert(rec.clone());
    let store = store.clone();
    let mut last = (first.status, first.bytes_total);
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            let s = rx.borrow_and_update().clone();
            // Status changes, plus the first manifest (total known).
            if (s.status, s.bytes_total) != last {
                last = (s.status, s.bytes_total);
                fill(&mut rec, &s);
                store.upsert(rec.clone());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, created: u64, status: Status) -> TransferRecord {
        TransferRecord {
            id: id.into(),
            kind: Kind::Get,
            ticket: Some("toss1x".into()),
            contact: None,
            paths: None,
            dest: Some("/tmp/d".into()),
            job: "j".into(),
            status,
            bytes_done: 1,
            bytes_total: 2,
            created,
            push_id: None,
            peer: String::new(),
            files_total: 0,
            files_done: 0,
            error: None,
            stranger: false,
        }
    }

    #[test]
    fn keeps_unfinished_and_newest_finished() {
        let dir = tempfile::tempdir().unwrap();
        let st = TransferStore::open(dir.path());
        st.upsert(rec("run", 0, Status::Running));
        for i in 0..60 {
            st.upsert(rec(&format!("d{i:02}"), 1 + i, Status::Done));
        }
        let st = TransferStore::open(dir.path());
        let v = st.records();
        assert_eq!(v.len(), 1 + MAX_FINISHED);
        assert_eq!(v[0].id, "run");
        assert_eq!(v[1].id, "d10");
        st.remove(&["run".into()]);
        st.upsert(rec("run", 0, Status::Paused));
        assert!(TransferStore::open(dir.path())
            .records()
            .iter()
            .all(|r| r.id != "run"));
        st.freeze();
        st.upsert(rec("late", 0, Status::Paused));
        assert!(st.records().iter().all(|r| r.id != "late"));
    }
}
