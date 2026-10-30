//! Persisted app settings: `<data_dir>/settings.json`.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{config, ipc::HostConfig};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub device_name: String,
    pub dest_dir: String,
    pub host: Option<HostConfig>,
    /// Admin ticket of a remote host (at most one).
    pub admin_ticket: Option<String>,
    pub autostart: bool,
    pub first_run: bool,
    /// Override of `config::DEFAULT_PAGE_HOST` (hand-edit only).
    pub page_host: Option<String>,
    /// v0.3: "Tạo link Toss" registered in the OS right-click menu (applied at startup).
    pub context_menu: bool,
    /// v0.3: a clicked link with `get` starts downloading at once (UI behaviour).
    pub auto_receive: bool,
    /// v0.3: folders shared via right-click; served as extra host roots (never in `host.roots`).
    pub quick_shares: Vec<QuickShare>,
    /// Host mode was turned on by a quick share (not by the user): it is turned off
    /// again when the last quick share is removed.
    pub host_auto: bool,
    /// v0.4: network settings (own relay, mDNS, port mapping, tuning). Old files load with defaults.
    pub net: toss_core::NetConfig,
    /// v0.5.1: install updates by themselves. None (old files, never touched)
    /// = on for a host (máy tổng, often unattended), off otherwise; see
    /// [`Settings::auto_update`].
    pub auto_update: Option<bool>,
    /// v0.8: who may send without asking (Toss pushes and LocalSend). Default "contacts".
    pub receive_policy: ReceivePolicy,
    /// v0.8: show this machine to LocalSend apps / run the LocalSend bridge (default on).
    pub ls_enabled: bool,
    /// v0.8: LocalSend devices accepted without asking ("Luôn nhận từ máy này").
    pub ls_trusted: Vec<LsTrusted>,
    /// v0.8: the one-time 0.8 migration (autostart forced on) has run.
    pub migrated_v08: bool,
}

/// v0.8 receive policy (ipc.ts `ReceivePolicy`: "off" | "contacts" | "everyone"); persisted here,
/// pushed to the core on start.
pub use toss_core::ReceivePolicy;

/// One trusted LocalSend device (ipc.ts `AppState.ls_trusted`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LsTrusted {
    pub fingerprint: String,
    pub alias: String,
}

/// One quick-shared folder (ipc.ts `list_quick_shares`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct QuickShare {
    /// `qs-<first 8 hex of blake3(canonical path)>`.
    pub alias: String,
    /// Canonical folder path.
    pub path: String,
    /// Id (hex) of the latest ticket created for it.
    pub job_id: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Settings {
            device_name: default_device_name(),
            dest_dir: home.join("Toss").to_string_lossy().into_owned(),
            host: None,
            admin_ticket: None,
            // v0.8: every machine starts with the OS unless the user turns it off.
            autostart: true,
            first_run: true,
            page_host: None,
            context_menu: true,
            auto_receive: true,
            quick_shares: Vec::new(),
            host_auto: false,
            net: toss_core::NetConfig::default(),
            auto_update: None,
            receive_policy: ReceivePolicy::Contacts,
            ls_enabled: true,
            ls_trusted: Vec::new(),
            migrated_v08: false,
        }
    }
}

fn default_device_name() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "Máy của tôi".to_string())
}

/// `TOSS_DATA_DIR` or `dirs::data_dir()/toss` (wire.md §5).
pub fn data_dir() -> PathBuf {
    if let Ok(p) = std::env::var(config::DATA_DIR_ENV) {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(config::DATA_DIR_NAME)
}

impl Settings {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("settings.json")
    }

    /// Loads settings; a missing file gives defaults, a corrupt one is kept as `settings.json.bad`.
    pub fn load(dir: &Path) -> Settings {
        let p = Self::path(dir);
        match fs::read(&p) {
            Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("settings.json unreadable ({e}); starting from defaults");
                    let _ = fs::rename(&p, dir.join("settings.json.bad"));
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        }
    }

    /// Atomic save (write temp, rename).
    pub fn save(&self, dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(dir)?;
        let tmp = dir.join("settings.json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(&tmp, Self::path(dir))?;
        Ok(())
    }

    /// Effective auto-update flag. v0.6: every machine is the same, so an untouched
    /// setting means on (machines are often reached only remotely).
    pub fn auto_update(&self) -> bool {
        self.auto_update.unwrap_or(true)
    }

    /// True when the LocalSend device `fingerprint` is in `ls_trusted`.
    pub fn ls_is_trusted(&self, fingerprint: &str) -> bool {
        self.ls_trusted.iter().any(|t| t.fingerprint == fingerprint)
    }

    pub fn page_host(&self) -> String {
        self.page_host
            .clone()
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| config::DEFAULT_PAGE_HOST.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::Root;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("toss-settings-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn v03_fields_round_trip() {
        let dir = tmp("rt");
        let s = Settings {
            context_menu: false,
            auto_receive: false,
            host_auto: true,
            host: Some(HostConfig {
                roots: vec![Root {
                    alias: "RAW".into(),
                    path: "/raw".into(),
                }],
                return_root: "/ret".into(),
                max_parallel_jobs: 2,
                bind_port: None,
            }),
            quick_shares: vec![QuickShare {
                alias: "qs-0123abcd".into(),
                path: "/photos/12A3".into(),
                job_id: Some("00ff".into()),
            }],
            ..Settings::default()
        };
        s.save(&dir).unwrap();
        let back = Settings::load(&dir);
        assert!(!back.context_menu && !back.auto_receive && back.host_auto);
        assert_eq!(back.quick_shares, s.quick_shares);
        assert_eq!(back.host, s.host);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_settings_get_v03_defaults() {
        let dir = tmp("old");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            Settings::path(&dir),
            r#"{"device_name":"May","dest_dir":"/d","host":null,"admin_ticket":null,"autostart":true,"first_run":false,"page_host":null}"#,
        )
        .unwrap();
        let s = Settings::load(&dir);
        assert_eq!(s.device_name, "May");
        assert!(s.context_menu, "context menu defaults to on");
        assert!(s.auto_receive, "auto receive defaults to on");
        assert!(s.quick_shares.is_empty() && !s.host_auto);
        assert_eq!(
            s.net,
            toss_core::NetConfig::default(),
            "pre-0.4 files get the default net config"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_update_defaults_to_on() {
        let mut s = Settings::default();
        assert!(s.auto_update(), "v0.6: every machine updates by itself unless turned off");
        s.host = Some(HostConfig::default());
        assert!(s.auto_update());
        s.auto_update = Some(false);
        assert!(!s.auto_update());
        s.host = None;
        s.auto_update = Some(true);
        assert!(s.auto_update());
        let dir = tmp("au");
        s.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir).auto_update, Some(true));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn v08_fields_default_and_round_trip() {
        let dir = tmp("v08");
        fs::create_dir_all(&dir).unwrap();
        // A 0.6 file: no v0.8 fields, autostart off.
        fs::write(
            Settings::path(&dir),
            r#"{"device_name":"May","autostart":false,"first_run":false}"#,
        )
        .unwrap();
        let s = Settings::load(&dir);
        assert_eq!(s.receive_policy, ReceivePolicy::Contacts);
        assert!(s.ls_enabled && s.ls_trusted.is_empty());
        assert!(!s.migrated_v08, "0.6 files still need the 0.8 migration");
        assert!(!s.autostart, "the file value is kept until the migration runs");
        assert!(Settings::default().autostart, "new installs start with the OS");
        let s = Settings {
            receive_policy: ReceivePolicy::Everyone,
            ls_enabled: false,
            ls_trusted: vec![LsTrusted {
                fingerprint: "ab12".into(),
                alias: "iPhone của Minh".into(),
            }],
            migrated_v08: true,
            ..s
        };
        s.save(&dir).unwrap();
        let raw = fs::read_to_string(Settings::path(&dir)).unwrap();
        assert!(raw.contains("\"receive_policy\": \"everyone\""), "{raw}");
        let back = Settings::load(&dir);
        assert_eq!(back.receive_policy, ReceivePolicy::Everyone);
        assert!(!back.ls_enabled && back.migrated_v08);
        assert!(back.ls_is_trusted("ab12") && !back.ls_is_trusted("cd34"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn net_round_trips_and_partial_net_fills_defaults() {
        let dir = tmp("net");
        let mut s = Settings::default();
        s.net.relay_urls = vec!["https://relay.example.vn".into()];
        s.net.mdns = false;
        s.net.tuning.streams = 16;
        s.net.tuning.congestion = toss_core::net::Congestion::Cubic;
        s.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir).net, s.net);
        fs::write(
            Settings::path(&dir),
            r#"{"device_name":"May","net":{"portmap":false}}"#,
        )
        .unwrap();
        let back = Settings::load(&dir);
        assert!(!back.net.portmap && back.net.mdns && back.net.relay_urls.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
