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
}

impl Default for Settings {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Settings {
            device_name: default_device_name(),
            dest_dir: home.join("Toss").to_string_lossy().into_owned(),
            host: None,
            admin_ticket: None,
            autostart: false,
            first_run: true,
            page_host: None,
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

    pub fn page_host(&self) -> String {
        self.page_host
            .clone()
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| config::DEFAULT_PAGE_HOST.to_string())
    }
}
