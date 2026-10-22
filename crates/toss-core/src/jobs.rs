//! Host-side job records (`<data_dir>/jobs.json`) and the revocation list
//! (`<data_dir>/revoked.json`).
//!
//! The files are re-read on every access so that a CLI process (`toss-cli
//! ticket`, `toss-cli revoke`) and a running host see each other's changes.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::{ticket::TicketBody, util::now_unix, wire::Op};

/// One issued ticket as the host remembers it (wire.md §4).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct JobRecord {
    /// Ticket id.
    pub id: [u8; 16],
    /// Job name.
    pub job: String,
    /// Root alias.
    pub root: String,
    /// Path under root.
    pub path: String,
    /// Download allowed.
    pub get: bool,
    /// Upload allowed.
    pub put: bool,
    /// Expiry (unix s, 0 = never).
    pub exp: u64,
    /// Creation time (unix s).
    pub created: u64,
    /// Revoked?
    pub revoked: bool,
    /// Last finished transfer.
    pub last: Option<JobEvent>,
}

/// A finished (or failed) transfer of a job.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct JobEvent {
    /// Unix seconds.
    pub at: u64,
    /// Client device name.
    pub device: String,
    /// Direction.
    pub op: Op,
    /// Files moved.
    pub files: u32,
    /// Bytes moved.
    pub bytes: u64,
    /// Success.
    pub ok: bool,
    /// Details.
    pub msg: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct StoredJob {
    #[serde(flatten)]
    record: JobRecord,
    #[serde(default)]
    ret: String,
    #[serde(default)]
    admin: bool,
    #[serde(default)]
    host: Option<String>,
}

/// File-backed job store.
#[derive(Debug)]
pub struct JobStore {
    jobs_path: PathBuf,
    revoked_path: PathBuf,
    lock: Mutex<()>,
}

impl JobStore {
    /// Opens (does not create) the store in `data_dir`.
    pub fn open(data_dir: &Path) -> JobStore {
        JobStore {
            jobs_path: data_dir.join("jobs.json"),
            revoked_path: data_dir.join("revoked.json"),
            lock: Mutex::new(()),
        }
    }

    fn load_jobs(&self) -> Vec<StoredJob> {
        match std::fs::read(&self.jobs_path) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
                tracing::warn!("jobs.json unreadable ({e}); starting empty");
                Vec::new()
            }),
            Err(_) => Vec::new(),
        }
    }

    fn save_jobs(&self, jobs: &[StoredJob]) -> Result<()> {
        write_atomic(&self.jobs_path, &serde_json::to_vec_pretty(jobs)?)
    }

    fn load_revoked(&self) -> BTreeSet<String> {
        match std::fs::read(&self.revoked_path) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_default(),
            Err(_) => BTreeSet::new(),
        }
    }

    /// All jobs, oldest first, with `revoked` reflecting `revoked.json`.
    pub fn list(&self) -> Vec<JobRecord> {
        let _g = self.lock.lock();
        let revoked = self.load_revoked();
        self.load_jobs()
            .into_iter()
            .map(|mut j| {
                j.record.revoked |= revoked.contains(&hex::encode(j.record.id));
                j.record
            })
            .collect()
    }

    /// Appends a record for a freshly signed ticket.
    pub fn add(&self, body: &TicketBody) -> Result<()> {
        let _g = self.lock.lock();
        let mut jobs = self.load_jobs();
        jobs.push(StoredJob {
            record: JobRecord {
                id: body.id,
                job: body.job.clone(),
                root: body.root.clone(),
                path: body.path.clone(),
                get: body.get,
                put: body.put,
                exp: body.exp,
                created: now_unix(),
                revoked: false,
                last: None,
            },
            ret: body.ret.clone(),
            admin: body.admin,
            host: Some(hex::encode(body.host)),
        });
        self.save_jobs(&jobs).context("save jobs.json")
    }

    /// Reconstructs the ticket body of a stored job (to re-sign it for display).
    pub fn body(&self, id: [u8; 16], host: [u8; 32]) -> Option<TicketBody> {
        let _g = self.lock.lock();
        self.load_jobs()
            .into_iter()
            .find(|j| j.record.id == id)
            .map(|j| TicketBody {
                v: crate::ticket::TICKET_VERSION,
                id,
                host,
                job: j.record.job,
                root: j.record.root,
                path: j.record.path,
                ret: j.ret,
                get: j.record.get,
                put: j.record.put,
                admin: j.admin,
                exp: j.record.exp,
            })
    }

    /// Updates `last` of a job (no-op for unknown ids).
    pub fn record_event(&self, id: [u8; 16], ev: JobEvent) -> Result<()> {
        let _g = self.lock.lock();
        let mut jobs = self.load_jobs();
        if let Some(j) = jobs.iter_mut().find(|j| j.record.id == id) {
            j.record.last = Some(ev);
            self.save_jobs(&jobs).context("save jobs.json")?;
        }
        Ok(())
    }

    /// Adds `id` to `revoked.json` and marks the record.
    pub fn revoke(&self, id: [u8; 16]) -> Result<()> {
        let _g = self.lock.lock();
        let mut revoked = self.load_revoked();
        revoked.insert(hex::encode(id));
        write_atomic(&self.revoked_path, &serde_json::to_vec_pretty(&revoked)?)
            .context("save revoked.json")?;
        let mut jobs = self.load_jobs();
        if let Some(j) = jobs.iter_mut().find(|j| j.record.id == id) {
            j.record.revoked = true;
            self.save_jobs(&jobs).context("save jobs.json")?;
        }
        Ok(())
    }

    /// Is `id` in `revoked.json`?
    pub fn is_revoked(&self, id: &[u8; 16]) -> bool {
        let _g = self.lock.lock();
        self.load_revoked().contains(&hex::encode(id))
    }
}

/// Writes via a temp file + rename.
pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, data).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ticket::{body_from_spec, TicketSpec};

    #[test]
    fn store_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = JobStore::open(dir.path());
        assert!(store.list().is_empty());
        let sk = iroh::SecretKey::generate();
        let spec = TicketSpec {
            job: "j".into(),
            root: "RAW".into(),
            path: "".into(),
            ret: "r".into(),
            get: true,
            put: false,
            days: 0,
        };
        let body = body_from_spec(&sk.public(), &spec, [9; 16]);
        store.add(&body).unwrap();
        assert_eq!(store.body([9; 16], body.host).unwrap(), body);
        let ev = JobEvent {
            at: 1,
            device: "d".into(),
            op: Op::Get,
            files: 2,
            bytes: 3,
            ok: true,
            msg: String::new(),
        };
        store.record_event([9; 16], ev.clone()).unwrap();
        assert!(!store.is_revoked(&[9; 16]));
        store.revoke([9; 16]).unwrap();
        assert!(store.is_revoked(&[9; 16]));
        let other = JobStore::open(dir.path());
        let list = other.list();
        assert_eq!(list.len(), 1);
        assert!(list[0].revoked);
        assert_eq!(list[0].last.as_ref(), Some(&ev));
    }
}
