//! Tauri commands: exactly the names and argument shapes of `Commands` in contracts/ipc.ts
//! (+ the additive `ui_ready`). All real work lives in core.rs.

use std::{path::Path, sync::Arc};

use serde::{Serialize, Serializer};
use serde_json::Value;
use tauri::{AppHandle, State, WebviewWindow};
use tauri_plugin_clipboard_manager::ClipboardExt as _;
use tauri_plugin_dialog::DialogExt as _;
use tauri_plugin_opener::OpenerExt as _;

use crate::{
    core::{self, Core},
    ipc::{AppState, HostConfig, JobRecord, TicketInfo, TicketSpec},
    shell::Shell,
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
    Ok(core.parse_ticket(&text)?)
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
