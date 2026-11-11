//! 0.9 Máy studio: một link cho mỗi folder thợ ngay khi copy xong, và một link
//! gửi-về cố định cho mỗi editor.
//!
//! - Folder thợ = thư mục con của `1_RAW` dưới thư mục Jobs (một "Folder cố định"),
//!   ví dụ `D:\Jobs\2026\11\03.11.26 12A1 Doan Ket HBT\1_RAW\Tuan Future`. Số file và
//!   dung lượng đứng yên 2 phút thì có link chỉ-tải (mặc định 30 ngày). Thợ copy thêm
//!   thẻ sau đó: link giữ nguyên, số liệu cập nhật.
//! - Link backup: chỉ-gửi, không hết hạn, file về `<nơi nhận file gửi lại>/<tên editor>`.
//! - Bật chế độ studio thì máy không ngủ để editor lúc nào cũng tải được.

use std::time::{Instant, UNIX_EPOCH};

use toss_core::jobwatch::{self, Settle, SHOOTER_PARENT};

use super::*;

const LINKS_FILE: &str = "studio-links.json";
/// How often the jobs folder is looked at.
pub(crate) const SCAN_EVERY: Duration = Duration::from_secs(30);
/// A folder whose numbers stay the same this long has finished copying.
const QUIET: Duration = Duration::from_secs(120);
/// Shooter folders untouched for longer are settled jobs: not counted again every half minute
/// (a new card copied into one touches its folder and brings it back).
const WINDOW_SECS: u64 = 2 * 24 * 3600;

/// One link the studio handed out (ipc.ts `StudioLink`).
#[derive(Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StudioLink {
    /// "shooter" (auto, a `1_RAW/<thợ>` folder), "folder" (right-click elsewhere), "backup".
    pub kind: String,
    /// Ticket id (hex).
    pub id: String,
    pub url: String,
    /// Job folder name; the editor's name for a backup link.
    pub job: String,
    #[serde(default)]
    pub shooter: String,
    /// Path under the jobs folder, `/`-separated.
    #[serde(default)]
    pub rel: String,
    #[serde(default)]
    pub files: u64,
    #[serde(default)]
    pub bytes: u64,
    pub at: u64,
}

pub(crate) struct Studio {
    links: StdMutex<Vec<StudioLink>>,
    settle: StdMutex<Settle>,
    /// One pass at a time (timer, "Quét ngay", right-click).
    pass: Mutex<()>,
    /// Last pass: when, and what went wrong if anything.
    last: StdMutex<(u64, Option<String>)>,
}

impl Studio {
    pub(crate) fn load(dir: &Path) -> Studio {
        let links = std::fs::read(dir.join(LINKS_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Studio {
            links: StdMutex::new(links),
            settle: StdMutex::new(Settle::new(QUIET)),
            pass: Mutex::new(()),
            last: StdMutex::new((0, None)),
        }
    }

    fn links(&self) -> std::sync::MutexGuard<'_, Vec<StudioLink>> {
        self.links.lock().unwrap_or_else(|p| p.into_inner())
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `a/b/c` from a relative path.
fn slash(rel: &Path) -> String {
    rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

impl Core {
    fn save_studio_links(&self) -> Result<()> {
        let links = self.studio.links().clone();
        let path = self.data_dir.join(LINKS_FILE);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&links)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// The jobs folder: alias and path of the chosen "Folder cố định".
    fn studio_root(&self) -> Option<(String, PathBuf)> {
        let s = self.settings();
        let h = s.host?;
        h.roots
            .into_iter()
            .find(|r| r.alias == s.studio.root)
            .map(|r| (r.alias, PathBuf::from(r.path)))
    }

    fn studio_note(&self, err: Option<String>) {
        *self.studio.last.lock().unwrap_or_else(|p| p.into_inner()) = (now(), err);
    }

    fn studio_changed(&self) {
        let _ = self.save_studio_links();
        if let Some(app) = self.app_opt() {
            let _ = app.emit("studio:links", json!({}));
        }
    }

    /// What the Links screen shows (ipc.ts `StudioView`).
    pub async fn studio_view(&self) -> Result<Value> {
        let s = self.settings();
        let roots: Vec<String> = s.host.as_ref().map(|h| h.roots.iter().map(|r| r.alias.clone()).collect()).unwrap_or_default();
        let node = self.node().await.ok();
        let jobs: HashMap<String, toss_core::JobRecord> = node
            .as_ref()
            .map(|n| n.list_jobs().into_iter().map(|j| (hex::encode(j.id), j)).collect())
            .unwrap_or_default();
        let moving: Vec<toss_core::TransferState> = node
            .as_ref()
            .map(|n| n.transfers().into_iter().filter(|t| !t.status.is_final() && t.side == toss_core::transfer::Side::Host).collect())
            .unwrap_or_default();
        let links: Vec<Value> = self
            .studio
            .links()
            .iter()
            .rev()
            .map(|l| {
                let mut v = serde_json::to_value(l).unwrap_or_default();
                if let Some(j) = jobs.get(&l.id) {
                    v["revoked"] = json!(j.revoked);
                    v["exp"] = json!(j.exp);
                    v["last"] = serde_json::to_value(&j.last).unwrap_or_default();
                }
                let active: Vec<Value> = moving
                    .iter()
                    .filter(|t| t.id.starts_with(&l.id))
                    .map(|t| json!({ "peer": t.peer, "op": t.op, "bytes_done": t.bytes_done, "bytes_total": t.bytes_total, "speed_bps": t.speed_bps, "path": t.path, "status": t.status }))
                    .collect();
                v["active"] = json!(active);
                v
            })
            .collect();
        let (last_scan, error) = self.studio.last.lock().unwrap_or_else(|p| p.into_inner()).clone();
        Ok(json!({
            "on": s.studio.on,
            "root": s.studio.root,
            "root_path": self.studio_root().map(|(_, p)| p.display().to_string()),
            "days": s.studio.days,
            "roots": roots,
            "return_root": s.host.as_ref().map(|h| h.return_root.clone()),
            "parallel": s.host.as_ref().map(|h| h.max_parallel_jobs),
            "links": links,
            "last_scan": last_scan,
            "error": error,
            "awake": toss_core::awake::is_held(),
        }))
    }

    /// Turns studio mode on/off, picks the jobs folder and how long links last.
    pub async fn studio_set(&self, on: bool, root: String, days: u32) -> Result<Value> {
        let s = self.settings();
        let has_root = s.host.as_ref().is_some_and(|h| h.roots.iter().any(|r| r.alias == root));
        if on && !has_root {
            bail!("Chọn thư mục Jobs trong “Folder cố định” trước");
        }
        self.update_settings(|s| {
            let fresh = on && (!s.studio.on || s.studio.root != root);
            s.studio.on = on;
            s.studio.root = root;
            s.studio.days = days;
            if fresh {
                // Folders copied in the last day get links; older jobs are left alone.
                s.studio.since = now().saturating_sub(24 * 3600);
            }
        })?;
        *self.studio.settle.lock().unwrap_or_else(|p| p.into_inner()) = Settle::new(QUIET);
        if let Ok(n) = self.node().await {
            n.set_stay_awake(on);
        }
        self.studio_note(None);
        self.studio_view().await
    }

    /// One look at the jobs folder: links for shooter folders that finished copying.
    pub async fn studio_tick(&self) {
        let s = self.settings();
        if !s.studio.on {
            return;
        }
        let _g = self.studio.pass.lock().await;
        let Some((alias, root)) = self.studio_root() else {
            self.studio_note(Some("Chưa chọn thư mục Jobs".into()));
            return;
        };
        if !root.is_dir() {
            self.studio_note(Some(format!("Không thấy {} (ổ chưa cắm?)", root.display())));
            return;
        }
        let since = UNIX_EPOCH + Duration::from_secs(s.studio.since.max(now().saturating_sub(WINDOW_SECS)));
        let r = root.clone();
        let found = match tokio::task::spawn_blocking(move || jobwatch::scan(&r, since)).await {
            Ok(f) => f,
            Err(e) => {
                self.studio_note(Some(format!("Lỗi quét: {e}")));
                return;
            }
        };
        let ready = self.studio.settle.lock().unwrap_or_else(|p| p.into_inner()).feed(&found, Instant::now());
        if ready.is_empty() {
            self.studio_note(None);
            return;
        }
        let revoked: HashSet<String> = match self.node().await {
            Ok(n) => n.list_jobs().into_iter().filter(|j| j.revoked).map(|j| hex::encode(j.id)).collect(),
            Err(e) => {
                self.studio_note(Some(format!("{e:#}")));
                return;
            }
        };
        let mut made = Vec::new();
        let mut err = None;
        for f in ready {
            let existing = self
                .studio
                .links()
                .iter()
                .position(|l| l.kind == "shooter" && l.rel == f.rel && !revoked.contains(&l.id));
            if let Some(i) = existing {
                let mut links = self.studio.links();
                links[i].files = f.files;
                links[i].bytes = f.bytes;
                continue;
            }
            let spec = TicketSpec {
                job: format!("{} - {}", f.job, f.shooter),
                root: alias.clone(),
                path: f.rel.clone(),
                ret: String::new(),
                get: true,
                put: false,
                days: s.studio.days,
            };
            match self.create_ticket(spec).await {
                Ok(info) => {
                    self.studio.links().push(StudioLink {
                        kind: "shooter".into(),
                        id: info.id,
                        url: info.web_url,
                        job: f.job.clone(),
                        shooter: f.shooter.clone(),
                        rel: f.rel,
                        files: f.files,
                        bytes: f.bytes,
                        at: now(),
                    });
                    made.push(format!("{} · {}", f.shooter, f.job));
                }
                Err(e) => {
                    tracing::warn!("studio link {}: {e:#}", f.rel);
                    err = Some(format!("Không tạo được link {}: {e:#}", f.rel));
                }
            }
        }
        self.studio_note(err);
        self.studio_changed();
        if !made.is_empty() {
            if let Some(app) = self.app_opt() {
                let body = if made.len() == 1 { format!("Link mới: {}", made[0]) } else { format!("{} link mới, mở Toss để chép", made.len()) };
                notify(app, &body);
            }
        }
    }

    /// Right-click "Tạo link Toss" / "Tạo link" in the app: a shooter folder under
    /// the jobs folder gets (or keeps) its studio link; any other folder a quick share.
    pub async fn studio_link_folder(&self, path: String) -> Result<StudioLink> {
        let p = PathBuf::from(path.trim());
        if !p.is_dir() {
            bail!("Chỉ tạo link cho folder");
        }
        let s = self.settings();
        let canon = toss_core::paths::canonicalize(&p)?;
        let under_root = self.studio_root().filter(|_| s.studio.on).and_then(|(alias, root)| {
            let root = toss_core::paths::canonicalize(&root).ok()?;
            canon.strip_prefix(&root).ok().map(|rel| (alias, slash(rel)))
        });
        let (files, bytes) = {
            let c = canon.clone();
            tokio::task::spawn_blocking(move || jobwatch::measure(&c)).await?
        };
        if let Some((alias, rel)) = under_root.filter(|(_, rel)| !rel.is_empty()) {
            let name = name_of(&canon);
            let parent = canon.parent().map(name_of).unwrap_or_default();
            let (job, shooter) = if parent.eq_ignore_ascii_case(SHOOTER_PARENT) {
                (canon.parent().and_then(Path::parent).map(name_of).unwrap_or_default(), name)
            } else {
                (name, String::new())
            };
            let revoked: HashSet<String> = self.node().await?.list_jobs().into_iter().filter(|j| j.revoked).map(|j| hex::encode(j.id)).collect();
            let found = self.studio.links().iter().position(|l| l.rel == rel && l.kind != "backup" && !revoked.contains(&l.id));
            if let Some(i) = found {
                let mut links = self.studio.links();
                links[i].files = files;
                links[i].bytes = bytes;
                let l = links[i].clone();
                drop(links);
                self.studio_changed();
                return Ok(l);
            }
            let job_name = if shooter.is_empty() { job.clone() } else { format!("{job} - {shooter}") };
            let info = self
                .create_ticket(TicketSpec { job: job_name, root: alias, path: rel.clone(), ret: String::new(), get: true, put: false, days: s.studio.days })
                .await?;
            let l = StudioLink { kind: if shooter.is_empty() { "folder".into() } else { "shooter".into() }, id: info.id, url: info.web_url, job, shooter, rel, files, bytes, at: now() };
            self.studio.links().push(l.clone());
            self.studio_changed();
            return Ok(l);
        }
        // Outside the jobs folder: the regular right-click quick share.
        let info = self.share_folder(canon.to_string_lossy().into_owned(), s.studio.days, false).await?;
        let l = StudioLink { kind: "folder".into(), id: info.id, url: info.web_url, job: name_of(&canon), shooter: String::new(), rel: String::new(), files, bytes, at: now() };
        self.studio.links().push(l.clone());
        self.studio_changed();
        Ok(l)
    }

    /// The editor's standing backup link: put-only, never expires, lands in
    /// `<nơi nhận file gửi lại>/<editor>`. The same editor gets the same link back.
    pub async fn studio_backup_link(&self, editor: String) -> Result<StudioLink> {
        let name = toss_core::paths::sanitize_component(editor.trim(), "");
        if name.is_empty() {
            bail!("Nhập tên editor");
        }
        let alias = match self.studio_root() {
            Some((a, _)) => a,
            None => self
                .settings()
                .host
                .and_then(|h| h.roots.first().map(|r| r.alias.clone()))
                .ok_or_else(|| anyhow!("Thêm một “Folder cố định” trước"))?,
        };
        let revoked: HashSet<String> = self.node().await?.list_jobs().into_iter().filter(|j| j.revoked).map(|j| hex::encode(j.id)).collect();
        if let Some(l) = self.studio.links().iter().find(|l| l.kind == "backup" && l.job.eq_ignore_ascii_case(&name) && !revoked.contains(&l.id)) {
            return Ok(l.clone());
        }
        let info = self
            .create_ticket(TicketSpec {
                job: format!("Backup · {name}"),
                root: alias,
                path: String::new(),
                ret: name.clone(),
                get: false,
                put: true,
                days: 0,
            })
            .await?;
        let l = StudioLink { kind: "backup".into(), id: info.id, url: info.web_url, job: name, shooter: String::new(), rel: String::new(), files: 0, bytes: 0, at: now() };
        self.studio.links().push(l.clone());
        self.studio_changed();
        Ok(l)
    }

    /// Revokes a link and drops it from the list.
    pub async fn studio_remove(&self, id: String) -> Result<Value> {
        if let Err(e) = self.revoke(id.clone()).await {
            tracing::warn!("studio remove {id}: {e:#}");
        }
        self.studio.links().retain(|l| l.id != id);
        self.studio_changed();
        self.studio_view().await
    }
}
