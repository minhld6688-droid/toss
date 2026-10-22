//! Serde mirrors of the types in contracts/ipc.ts (snake_case, same field names).

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Root {
    pub alias: String,
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct HostConfig {
    pub roots: Vec<Root>,
    pub return_root: String,
    pub max_parallel_jobs: u32,
    /// Additive (CHANGES.md): fixed UDP port; absent/null = random. Applied when
    /// the node (re)starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_port: Option<u16>,
}

#[derive(Serialize, Clone, Debug)]
pub struct AppState {
    pub version: String,
    pub endpoint_id: String,
    pub device_name: String,
    pub dest_dir: String,
    pub host: Option<HostConfig>,
    pub page_host: String,
    pub autostart: bool,
    pub first_run: bool,
    /// v0.3: "Tạo link Toss" registered in the OS right-click menu.
    pub context_menu: bool,
    /// v0.3: clicked `get` links start downloading immediately.
    pub auto_receive: bool,
    /// v0.4: network settings (own relay, mDNS, port mapping, tuning).
    pub net: toss_core::NetConfig,
}

/// v0.4 `get_diagnostics`: the core snapshot plus what only the shell can see.
#[derive(Serialize, Clone, Debug)]
pub struct NetDiagUi {
    #[serde(flatten)]
    pub diag: toss_core::NetDiag,
    pub firewall: crate::firewall::FirewallState,
    /// Folder holding `toss.log` (shown to the user, opened by "Xuất log").
    pub log_dir: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct TicketInfo {
    pub ticket: String,
    pub id: String,
    pub host: String,
    pub job: String,
    pub root: String,
    pub path: String,
    pub ret: String,
    pub get: bool,
    pub put: bool,
    pub admin: bool,
    pub exp: u64,
    pub web_url: String,
    pub deep_link: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct TicketSpec {
    pub job: String,
    pub root: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub ret: String,
    pub get: bool,
    pub put: bool,
    pub days: u32,
}

#[derive(Serialize, Clone, Debug)]
pub struct JobLast {
    pub at: u64,
    pub device: String,
    pub op: &'static str,
    pub files: u32,
    pub bytes: u64,
    pub ok: bool,
    pub msg: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct JobRecord {
    pub id: String,
    pub job: String,
    pub root: String,
    pub path: String,
    pub get: bool,
    pub put: bool,
    pub exp: u64,
    pub created: u64,
    pub revoked: bool,
    pub last: Option<JobLast>,
    /// Re-signed ticket string for display/copy; "" when this app did not mint it.
    pub ticket: String,
}
