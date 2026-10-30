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
    /// v0.5.1: updates install by themselves (default: on for a host).
    pub auto_update: bool,
    /// v0.8: who may send without asking.
    pub receive_policy: crate::settings::ReceivePolicy,
    /// v0.8: LocalSend bridge on ("Hiện với máy LocalSend").
    pub ls_enabled: bool,
    /// v0.8: port the LocalSend server listens on (None = not running).
    pub ls_port: Option<u16>,
    /// v0.8: LocalSend devices accepted without asking.
    pub ls_trusted: Vec<crate::settings::LsTrusted>,
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
    /// v0.5.2: link to share (short `https://<page_host>/<code>` when available, else the
    /// long `#toss1…` link); "" when `ticket` is "".
    pub web_url: String,
}

// ---- v0.5 "Danh bạ máy" (contracts/contacts-v0.5.md §3/§4) -----------------------------------------
// Same field names/types as the core structs, so the real calls convert with `core_v05::via_serde`.

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Contact {
    pub id: String,
    pub name: String,
    pub their_name: String,
    pub platform: String,
    pub added_at: u64,
    pub auto_accept: bool,
    pub online: bool,
    pub last_seen: u64,
    pub rtt_ms: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct InviteInfo {
    pub invite: String,
    pub web_url: String,
    pub deep_link: String,
    pub exp: u64,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct IncomingPush {
    pub push_id: String,
    pub from_id: String,
    pub from_name: String,
    pub job: String,
    pub files: u32,
    pub bytes: u64,
    pub at: u64,
    /// v0.8: sent by a nearby device that is not a contact (core `IncomingPush.stranger`).
    #[serde(default)]
    pub stranger: bool,
}

// ---- v0.8 nearby + LocalSend (contracts/nearby-v0.8.md §4) ---------------------------------------

/// Core `NearbyPeer` (§1.1), read through serde.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CoreNearby {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub addrs: Vec<String>,
    #[serde(default)]
    pub last_seen: u64,
    #[serde(default)]
    pub contact: bool,
}

/// ipc.ts `Nearby`.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Nearby {
    /// `t:<endpoint hex>` | `l:<fingerprint>`.
    pub key: String,
    /// "toss" | "toss_lan" | "localsend".
    pub kind: &'static str,
    pub name: String,
    pub model: Option<String>,
    pub platform: String,
    pub contact_id: Option<String>,
    pub trusted: bool,
}

/// ipc.ts `Incoming`: one ask, from a Toss push or a LocalSend prepare-upload.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Incoming {
    pub id: String,
    /// "toss" | "localsend".
    pub source: String,
    pub from_key: String,
    pub from_name: String,
    pub stranger: bool,
    pub job: String,
    pub files: u32,
    pub bytes: u64,
    pub names: Vec<String>,
    pub at: u64,
}

/// ipc.ts `Received` (history entry, `<data_dir>/received.json`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Received {
    pub id: String,
    pub from_name: String,
    pub job: String,
    pub files: u32,
    pub bytes: u64,
    pub dest: String,
    pub at: u64,
}

/// ipc.ts `TextMsg`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TextMsg {
    pub from_name: String,
    pub text: String,
    pub at: u64,
}

/// ipc.ts `WebShare`.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct WebShare {
    pub urls: Vec<String>,
    pub qr_svg: String,
}
