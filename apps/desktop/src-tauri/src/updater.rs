//! v0.5.1 in-app updates (tauri-plugin-updater, signed release feed from
//! `tauri.conf.json` → `plugins.updater`).
//!
//! - [`check`]: asks the feed; `forced` when the release notes carry `#force`.
//! - [`install`]: waits until no transfer is running (bounded), downloads with
//!   `update:progress` events, shuts the node down (client transfers are
//!   frozen as running in `transfers.json` and resume after the restart),
//!   installs and restarts.
//! - [`spawn_background`]: first check 60 s after start, then every 6 h;
//!   emits `update:available` and installs by itself when the release is
//!   forced or the `auto_update` setting is on.
//! - [`on_remote_request`]: an admin device asked this host (core event
//!   `update_requested`) → check + install.
//!
//! UI events: `update:available` [`UpdateInfo`], `update:phase`
//! `{phase, active}`, `update:progress {downloaded, total}`, `update:error {msg}`.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::core::Core;

/// Marker in the release notes that makes every app install the release
/// without waiting for the user (still after running transfers finish).
pub const FORCE_MARK: &str = "#force";
/// First background check after start.
const FIRST_CHECK: Duration = Duration::from_secs(60);
/// Background check interval.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
/// How often an install that waits for running transfers looks again.
const IDLE_POLL: Duration = Duration::from_secs(15);

/// `check_update` result / `update:available` payload.
#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct UpdateInfo {
    /// A newer release than the running one exists.
    pub available: bool,
    /// Running version.
    pub current: String,
    /// The newer version (None when up to date).
    pub version: Option<String>,
    /// Release notes (the `#force` marker included).
    pub notes: Option<String>,
    /// Release notes contain `#force`: installs without asking.
    pub forced: bool,
    /// Release date as published in the feed.
    pub date: Option<String>,
}

/// Why an install runs; decides how long it waits for running transfers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// "Cập nhật & khởi động lại" button.
    User,
    /// Background check with the `auto_update` setting on.
    Auto,
    /// Background check found a `#force` release.
    Forced,
    /// An admin device asked (ctl `RequestUpdate`).
    Remote,
}

impl Reason {
    /// Longest wait for running transfers before installing anyway (they are
    /// frozen and resume after the restart).
    pub fn idle_limit(self) -> Duration {
        match self {
            Reason::Remote => Duration::from_secs(10 * 60),
            Reason::User | Reason::Auto | Reason::Forced => Duration::from_secs(30 * 60),
        }
    }
}

/// One install at a time (background, button and remote request can race).
static INSTALLING: AtomicBool = AtomicBool::new(false);

struct InstallGuard;

impl Drop for InstallGuard {
    fn drop(&mut self) {
        INSTALLING.store(false, Ordering::SeqCst);
    }
}

/// `#force` anywhere in the notes, any case.
pub fn is_forced(notes: Option<&str>) -> bool {
    notes.is_some_and(|n| n.to_lowercase().contains(FORCE_MARK))
}

fn info_of(current: &str, u: Option<&Update>) -> UpdateInfo {
    match u {
        None => UpdateInfo {
            current: current.to_string(),
            ..UpdateInfo::default()
        },
        Some(u) => UpdateInfo {
            available: true,
            current: current.to_string(),
            version: Some(u.version.clone()),
            notes: u.body.clone().filter(|b| !b.trim().is_empty()),
            forced: is_forced(u.body.as_deref()),
            date: u.date.map(|d| d.to_string()),
        },
    }
}

fn current_version(app: &AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Asks the release feed; `Some` when a newer version exists.
async fn fetch(app: &AppHandle) -> Result<Option<Update>> {
    let updater = app
        .updater()
        .map_err(|e| anyhow!("trình cập nhật chưa sẵn sàng: {e}"))?;
    updater
        .check()
        .await
        .map_err(|e| anyhow!("không kiểm tra được bản mới: {e}"))
}

/// `check_update`.
pub async fn check(app: &AppHandle) -> Result<UpdateInfo> {
    let current = current_version(app);
    tracing::info!(%current, "update: checking the release feed");
    let u = fetch(app)
        .await
        .inspect_err(|e| tracing::warn!("update: {e:#}"))?;
    let info = info_of(&current, u.as_ref());
    match &info.version {
        Some(v) => {
            tracing::info!(%current, version = %v, forced = info.forced, "update: available")
        }
        None => tracing::info!(%current, "update: up to date"),
    }
    Ok(info)
}

/// Checks, then installs when a newer version exists. Waits for running
/// transfers (bounded by `reason`), downloads (progress events), shuts the
/// node down so client transfers resume after the restart, installs and
/// restarts. `Ok(false)`: nothing to install or another install runs.
/// Errors are also sent as `update:error`.
pub async fn install(app: &AppHandle, core: &Arc<Core>, reason: Reason) -> Result<bool> {
    if INSTALLING.swap(true, Ordering::SeqCst) {
        tracing::info!(?reason, "update: an install is already in progress");
        return Ok(false);
    }
    let _guard = InstallGuard;
    let res = install_inner(app, core, reason).await;
    if let Err(e) = &res {
        tracing::error!(?reason, "update failed: {e:#}");
        let _ = app.emit("update:error", json!({ "msg": format!("{e:#}") }));
    }
    res
}

async fn install_inner(app: &AppHandle, core: &Arc<Core>, reason: Reason) -> Result<bool> {
    let current = current_version(app);
    let Some(update) = fetch(app).await? else {
        tracing::info!(?reason, %current, "update: no newer version, nothing to install");
        return Ok(false);
    };
    let version = update.version.clone();
    tracing::info!(?reason, %current, %version, "update: installing");

    wait_idle(app, core, reason).await;

    let _ = app.emit(
        "update:phase",
        json!({ "phase": "downloading", "active": 0 }),
    );
    let mut downloaded: u64 = 0;
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    let mut last_log_pct = 0u64;
    let started = Instant::now();
    let bytes = update
        .download(
            |chunk, total| {
                downloaded += chunk as u64;
                if let Some(t) = total.filter(|t| *t > 0) {
                    let pct = downloaded * 100 / t;
                    if pct >= last_log_pct + 25 {
                        last_log_pct = pct - pct % 25;
                        tracing::info!(%version, downloaded, total = t, "update: download {pct}%");
                    }
                }
                if last_emit.elapsed() >= Duration::from_millis(150) {
                    last_emit = Instant::now();
                    let _ = app.emit(
                        "update:progress",
                        json!({ "downloaded": downloaded, "total": total }),
                    );
                }
            },
            || {},
        )
        .await
        .map_err(|e| anyhow!("tải bản {version} thất bại: {e}"))?;
    let _ = app.emit(
        "update:progress",
        json!({ "downloaded": bytes.len(), "total": bytes.len() }),
    );
    tracing::info!(
        %version,
        bytes = bytes.len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "update: downloaded and verified"
    );

    // From here on the app goes away: freeze + pause client transfers (they
    // resume from transfers.json after the restart) and close the endpoint.
    let _ = app.emit(
        "update:phase",
        json!({ "phase": "installing", "active": 0 }),
    );
    tracing::info!(%version, "update: stopping the node before installing");
    core.shutdown().await;
    if let Err(e) = update.install(&bytes) {
        // Bring the network (and the frozen transfers) back.
        if let Err(e2) = core.node().await {
            tracing::error!("node restart after a failed install: {e2:#}");
        }
        return Err(anyhow!("cài bản {version} thất bại: {e}"));
    }
    tracing::info!(%version, "update: installed; restarting");
    app.restart();
}

/// Waits (polling every 15 s) until no transfer is running, at most
/// `reason.idle_limit()`; then proceeds anyway.
async fn wait_idle(app: &AppHandle, core: &Core, reason: Reason) {
    let limit = reason.idle_limit();
    let t0 = Instant::now();
    loop {
        let active = core.running_transfers().await;
        if active == 0 {
            return;
        }
        if t0.elapsed() >= limit {
            tracing::info!(
                ?reason,
                active,
                "update: transfers still running after {} min; installing anyway \
                 (they resume after the restart)",
                limit.as_secs() / 60
            );
            return;
        }
        tracing::info!(?reason, active, "update: waiting for running transfers");
        let _ = app.emit(
            "update:phase",
            json!({ "phase": "waiting", "active": active }),
        );
        tokio::time::sleep(IDLE_POLL).await;
    }
}

/// Background checks (started from `setup`).
pub fn spawn_background(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            background_round(&app).await;
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

async fn background_round(app: &AppHandle) {
    let info = match check(app).await {
        Ok(i) => i,
        Err(_) => return, // logged by check(); try again next round
    };
    if !info.available {
        return;
    }
    let _ = app.emit("update:available", &info);
    let core = app.state::<Arc<Core>>().inner().clone();
    let reason = if info.forced {
        Reason::Forced
    } else if core.auto_update() {
        Reason::Auto
    } else {
        tracing::info!("update: auto-update off; waiting for the user");
        return;
    };
    let _ = install(app, &core, reason).await;
}

/// Core event `update_requested`: an admin device asked this host to update.
pub fn on_remote_request(app: &AppHandle, device: &str) {
    tracing::info!(%device, "update: requested remotely");
    crate::core::notify(app, &format!("Cập nhật theo yêu cầu từ {device}"));
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let info = match check(&app).await {
            Ok(i) => i,
            Err(_) => return,
        };
        if !info.available {
            tracing::info!("update: remote request, but already on the newest version");
            return;
        }
        let _ = app.emit("update:available", &info);
        let core = app.state::<Arc<Core>>().inner().clone();
        let _ = install(&app, &core, Reason::Remote).await;
    });
}

/// `a.b.c` ≥ `min`? Unparsable versions count as new enough (the request
/// itself then tells).
pub fn version_at_least(v: &str, min: (u64, u64, u64)) -> bool {
    let mut it = v
        .trim()
        .trim_start_matches('v')
        .split(['.', '-', '+'])
        .map(|p| p.parse::<u64>());
    match (it.next(), it.next(), it.next()) {
        (Some(Ok(a)), Some(Ok(b)), Some(Ok(c))) => (a, b, c) >= min,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_marker_any_case() {
        assert!(is_forced(Some("Sửa lỗi #force")));
        assert!(is_forced(Some("#FORCE\nbản vá")));
        assert!(!is_forced(Some("force push")));
        assert!(!is_forced(None));
    }

    #[test]
    fn versions() {
        assert!(version_at_least("0.5.1", (0, 5, 1)));
        assert!(version_at_least("0.6.0", (0, 5, 1)));
        assert!(version_at_least("1.0.0-beta", (0, 5, 1)));
        assert!(!version_at_least("0.5.0", (0, 5, 1)));
        assert!(!version_at_least("0.4.9", (0, 5, 1)));
        assert!(version_at_least("test", (0, 5, 1)));
    }

    #[test]
    fn update_info_json_shape() {
        let v = serde_json::to_value(info_of("0.5.1", None)).unwrap();
        assert_eq!(
            v,
            json!({"available": false, "current": "0.5.1", "version": null,
                   "notes": null, "forced": false, "date": null})
        );
    }

    #[test]
    fn remote_waits_less() {
        assert!(Reason::Remote.idle_limit() < Reason::User.idle_limit());
        assert_eq!(Reason::Remote.idle_limit(), Duration::from_secs(600));
    }
}
