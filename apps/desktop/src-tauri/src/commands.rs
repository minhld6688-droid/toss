//! Tauri commands: exactly the names and argument shapes of `Commands` in contracts/ipc.ts
//! (+ the additive `ui_ready`). All real work lives in core.rs.

use std::{path::Path, sync::Arc};

use serde::{Serialize, Serializer};
use serde_json::Value;
use tauri::{AppHandle, State, WebviewWindow};
use tauri_plugin_clipboard_manager::ClipboardExt as _;
use tauri_plugin_dialog::DialogExt as _;
use tauri_plugin_notification::NotificationExt as _;
use tauri_plugin_opener::OpenerExt as _;

use crate::{
    contacts,
    core::{self, Core},
    firewall,
    ipc::{
        AppState, Contact, HostConfig, IncomingPush, InviteInfo, JobRecord, NetDiagUi, TicketInfo,
        TicketSpec,
    },
    settings::QuickShare,
    shell::Shell,
    updater,
};

/// Errors cross the IPC boundary as plain strings (Vietnamese where user-facing).
#[derive(Debug)]
pub struct CmdError(String);

impl Serialize for CmdError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<E: Into<anyhow::Error>> From<E> for CmdError {
    fn from(e: E) -> Self {
        CmdError(format!("{:#}", e.into()))
    }
}

type Res<T> = Result<T, CmdError>;
type C<'a> = State<'a, Arc<Core>>;

#[tauri::command(rename_all = "snake_case")]
pub async fn get_state(core: C<'_>) -> Res<AppState> {
    Ok(core.state().await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn finish_setup(core: C<'_>, device_name: String, dest_dir: String) -> Res<AppState> {
    Ok(core.finish_setup(device_name, dest_dir).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_device_name(core: C<'_>, name: String) -> Res<()> {
    Ok(core.set_device_name(name).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_dest_dir(core: C<'_>, path: String) -> Res<()> {
    Ok(core.set_dest_dir(path)?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_autostart(core: C<'_>, on: bool) -> Res<()> {
    Ok(core.set_autostart(on)?)
}

// ---- quick share (v0.3) ----------------------------------------------------------------------
/// Shares a folder (quick-share root + get ticket), copies the web link to the
/// clipboard and confirms with a system notification.
#[tauri::command(rename_all = "snake_case")]
pub async fn share_folder(
    app: AppHandle,
    core: C<'_>,
    path: String,
    days: u32,
    put: bool,
) -> Res<TicketInfo> {
    let info = core.share_folder(path, days, put).await?;
    match app.clipboard().write_text(info.web_url.clone()) {
        Ok(()) => {
            let _ = app
                .notification()
                .builder()
                .title("Toss")
                .body(format!("Đã copy link Toss cho {}", info.job))
                .show();
        }
        Err(e) => tracing::warn!("clipboard: {e}"),
    }
    Ok(info)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_quick_shares(core: C<'_>) -> Res<Vec<QuickShare>> {
    Ok(core.list_quick_shares())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remove_quick_share(core: C<'_>, alias: String) -> Res<()> {
    Ok(core.remove_quick_share(alias).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_context_menu(core: C<'_>, on: bool) -> Res<()> {
    Ok(core.set_context_menu(on).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_auto_receive(core: C<'_>, on: bool) -> Res<()> {
    Ok(core.set_auto_receive(on)?)
}

/// Native folder dialog. `None` = cancelled.
#[tauri::command(rename_all = "snake_case")]
pub async fn pick_folder(
    app: AppHandle,
    window: WebviewWindow,
    title: String,
) -> Res<Option<String>> {
    let picked = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title(title)
            .set_parent(&window)
            .blocking_pick_folder()
    })
    .await?;
    match picked {
        None => Ok(None),
        Some(p) => {
            let path = p
                .into_path()
                .map_err(|e| anyhow::anyhow!("đường dẫn không hợp lệ: {e}"))?;
            Ok(Some(path.to_string_lossy().into_owned()))
        }
    }
}

/// Reveal / open a folder in Finder / Explorer.
#[tauri::command(rename_all = "snake_case")]
pub async fn open_path(app: AppHandle, path: String) -> Res<()> {
    if !Path::new(&path).exists() {
        return Err(CmdError(format!("Không tìm thấy {path}")));
    }
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| CmdError(format!("Không mở được thư mục: {e}")))
}

// ---- host mode ----------------------------------------------------------------------------------
#[tauri::command(rename_all = "snake_case")]
pub async fn set_host(core: C<'_>, config: Option<HostConfig>) -> Res<()> {
    Ok(core.set_host(config).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn host_status(core: C<'_>) -> Res<Value> {
    Ok(core.host_status().await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_dir(core: C<'_>, root: String, path: String) -> Res<Value> {
    Ok(core.list_dir(root, path).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn create_ticket(core: C<'_>, spec: TicketSpec) -> Res<TicketInfo> {
    Ok(core.create_ticket(spec).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_jobs(core: C<'_>) -> Res<Vec<JobRecord>> {
    Ok(core.list_jobs().await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn revoke(core: C<'_>, id: String) -> Res<()> {
    Ok(core.revoke(id).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn create_admin_ticket(core: C<'_>, days: u32) -> Res<TicketInfo> {
    Ok(core.create_admin_ticket(days).await?)
}

// ---- tickets / transfers ---------------------------------------------------------------------------
#[tauri::command(rename_all = "snake_case")]
pub async fn parse_ticket(core: C<'_>, text: String) -> Res<TicketInfo> {
    Ok(core.parse_ticket(&text).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn start_get(core: C<'_>, ticket: String) -> Res<String> {
    Ok(core.start_get(ticket).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn start_put(core: C<'_>, ticket: String, paths: Vec<String>) -> Res<String> {
    Ok(core.start_put(ticket, paths).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_transfers(core: C<'_>) -> Res<Vec<Value>> {
    Ok(core.list_transfers().await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn pause(core: C<'_>, id: String) -> Res<()> {
    Ok(core.pause(id).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn resume(core: C<'_>, id: String) -> Res<()> {
    Ok(core.resume(id).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn cancel(core: C<'_>, id: String) -> Res<()> {
    Ok(core.cancel(id).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn clear_finished(core: C<'_>) -> Res<()> {
    Ok(core.clear_finished().await?)
}

// ---- remote host control ---------------------------------------------------------------------------------
#[tauri::command(rename_all = "snake_case")]
pub async fn set_admin_ticket(core: C<'_>, ticket: Option<String>) -> Res<()> {
    Ok(core.set_admin_ticket(ticket).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remote_status(core: C<'_>) -> Res<Value> {
    Ok(core.remote_status().await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remote_list_dir(core: C<'_>, root: String, path: String) -> Res<Value> {
    Ok(core.remote_list_dir(root, path).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remote_create_ticket(core: C<'_>, spec: TicketSpec) -> Res<TicketInfo> {
    Ok(core.remote_create_ticket(spec).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remote_list_jobs(core: C<'_>) -> Res<Vec<JobRecord>> {
    Ok(core.remote_list_jobs().await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remote_revoke(core: C<'_>, id: String) -> Res<()> {
    Ok(core.remote_revoke(id).await?)
}

/// Additive (not in ipc.ts): is an admin ticket stored? Never dials the host.
#[tauri::command(rename_all = "snake_case")]
pub async fn has_admin_ticket(core: C<'_>) -> Res<bool> {
    Ok(core.has_admin_ticket())
}

// ---- v0.4 diagnostics / network ---------------------------------------------------------------------
#[tauri::command(rename_all = "snake_case")]
pub async fn get_diagnostics(core: C<'_>) -> Res<NetDiagUi> {
    Ok(core.diagnostics().await?)
}

/// Windows: one UAC prompt; macOS: admin password prompt; Linux: nothing to do.
#[tauri::command(rename_all = "snake_case")]
pub async fn add_firewall_rule() -> Res<()> {
    tokio::task::spawn_blocking(firewall::add_rule).await??;
    Ok(())
}

/// An empty `ticket` tests the stored admin ticket's host (the UI never holds that ticket).
#[tauri::command(rename_all = "snake_case")]
pub async fn test_connect(core: C<'_>, ticket: String) -> Res<toss_core::ConnectReport> {
    Ok(core.test_connect(ticket).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_net(core: C<'_>, net: toss_core::NetConfig) -> Res<()> {
    Ok(core.set_net(net).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn export_logs(core: C<'_>) -> Res<String> {
    Ok(core.export_logs().await?)
}

// ---- v0.5 contacts ("Danh bạ máy") ----------------------------------------------------------------------------
#[tauri::command(rename_all = "snake_case")]
pub async fn list_contacts(core: C<'_>) -> Res<Vec<Contact>> {
    Ok(core.list_contacts().await?)
}

/// Creates the invite and copies its web link to the clipboard (the UI shows link + QR).
#[tauri::command(rename_all = "snake_case")]
pub async fn create_invite(app: AppHandle, core: C<'_>, days: u32) -> Res<InviteInfo> {
    let info = core.create_invite(days).await?;
    if let Err(e) = app.clipboard().write_text(info.web_url.clone()) {
        tracing::warn!("clipboard: {e}");
    }
    Ok(info)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn revoke_invites(core: C<'_>) -> Res<()> {
    Ok(core.revoke_invites().await?)
}

/// Additive (not in ipc.ts), v0.5.2: turns a pasted short link (`https://<host>/<code>`) into the
/// ticket / invite it stands for; any other text comes back unchanged.
#[tauri::command(rename_all = "snake_case")]
pub async fn expand_link(text: String) -> Res<String> {
    Ok(crate::shortlink::expand(&text).await?)
}

/// Additive (not in ipc.ts): decode an invite for the "Kết nối với <name>?" confirm, without pairing.
#[tauri::command(rename_all = "snake_case")]
pub async fn parse_invite(core: C<'_>, text: String) -> Res<InviteInfo> {
    Ok(core.parse_invite(&text).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn accept_invite(core: C<'_>, text: String) -> Res<Contact> {
    Ok(core.accept_invite(text).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rename_contact(core: C<'_>, id: String, name: String) -> Res<()> {
    Ok(core.rename_contact(id, name).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_contact_auto_accept(core: C<'_>, id: String, on: bool) -> Res<()> {
    Ok(core.set_contact_auto_accept(id, on).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remove_contact(core: C<'_>, id: String) -> Res<()> {
    Ok(core.remove_contact(id).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn send_to_contact(core: C<'_>, id: String, paths: Vec<String>) -> Res<String> {
    Ok(core.send_to_contact(id, paths).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_incoming_pushes(core: C<'_>) -> Res<Vec<IncomingPush>> {
    Ok(core.list_incoming_pushes().await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn answer_push(core: C<'_>, push_id: String, accept: bool) -> Res<()> {
    Ok(core.answer_push(push_id, accept).await?)
}

/// macOS: opens System Settings → Privacy & Security → Local Network. Elsewhere: no-op.
#[tauri::command(rename_all = "snake_case")]
pub async fn open_local_network_settings() -> Res<()> {
    Ok(contacts::open_local_network_settings()?)
}

// ---- v0.5.1 updates ----------------------------------------------------------------------------------------
#[tauri::command(rename_all = "snake_case")]
pub async fn check_update(app: AppHandle) -> Res<updater::UpdateInfo> {
    Ok(updater::check(&app).await?)
}

/// Installs the newer version (after running transfers finish, at most 30 min) and restarts.
/// Progress arrives as `update:phase` / `update:progress`; errors also as `update:error`.
#[tauri::command(rename_all = "snake_case")]
pub async fn install_update(app: AppHandle, core: C<'_>) -> Res<()> {
    let core = core.inner().clone();
    match updater::install(&app, &core, updater::Reason::User).await? {
        true => Ok(()),
        false => Err(CmdError(
            "Bạn đang dùng bản mới nhất (hoặc đang cập nhật)".into(),
        )),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_auto_update(core: C<'_>, on: bool) -> Res<()> {
    Ok(core.set_auto_update(on)?)
}

/// Asks the remote host (stored admin ticket) to update itself and restart.
#[tauri::command(rename_all = "snake_case")]
pub async fn remote_update(core: C<'_>) -> Res<()> {
    Ok(core.remote_update().await?)
}

// ---- misc ------------------------------------------------------------------------------------------------------
#[tauri::command(rename_all = "snake_case")]
pub async fn qr_svg(text: String) -> Res<String> {
    Ok(core::qr_svg(&text)?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn copy_text(app: AppHandle, text: String) -> Res<()> {
    app.clipboard()
        .write_text(text)
        .map_err(|e| CmdError(format!("Không chép được vào clipboard: {e}")))
}

/// Additive (not in ipc.ts): the UI has registered its listeners; flush queued deep links.
#[tauri::command(rename_all = "snake_case")]
pub async fn ui_ready(app: AppHandle, shell: State<'_, Shell>) -> Res<()> {
    shell.mark_ready(&app);
    Ok(())
}
