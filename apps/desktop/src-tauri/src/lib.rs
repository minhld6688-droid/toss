//! Toss desktop shell: Tauri 2 app around `toss-core`.

mod commands;
pub mod config;
mod contacts;
mod core;
mod ctxmenu;
mod firewall;
mod ipc;
mod logs;
mod ls;
mod nearby;
mod services;
mod settings;
mod shell;
mod shortlink;
#[cfg(target_os = "macos")]
mod traydrop;
mod updater;

use std::sync::Arc;

use tauri::{DragDropEvent, Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;

use crate::{config::HIDDEN_FLAG, core::Core, shell::Shell};

pub fn run() {
    // stderr + `<data_dir>/logs/toss.log` (the "Xuất log" button zips that folder).
    logs::init(&settings::data_dir().join("logs"));

    // reqwest (updater, short links) is built with `rustls-no-provider`: without a process
    // default it panics when a client is built. iroh already links ring.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let core = Arc::new(Core::new());
    let start_hidden = std::env::args().any(|a| a == HIDDEN_FLAG);

    let setup_core = core.clone();
    let app = tauri::Builder::default()
        // single-instance must be registered first: forwards argv (deep links on Windows/Linux).
        // Also carries `--share <folder>` from the right-click menu.
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            let handled = shell::handle_args(app, &argv, std::path::Path::new(&cwd));
            if !handled && !argv.iter().any(|a| a == HIDDEN_FLAG) {
                shell::show_main(app);
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![HIDDEN_FLAG]),
        ))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        // v0.5.1 in-app updates; everything runs in Rust commands (no JS permission needed).
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(core.clone())
        .manage(Shell::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::finish_setup,
            commands::set_device_name,
            commands::set_dest_dir,
            commands::set_autostart,
            commands::pick_folder,
            commands::open_path,
            commands::set_host,
            commands::host_status,
            commands::list_dir,
            commands::create_ticket,
            commands::list_jobs,
            commands::revoke,
            commands::create_admin_ticket,
            commands::parse_ticket,
            commands::start_get,
            commands::start_put,
            commands::list_transfers,
            commands::pause,
            commands::resume,
            commands::cancel,
            commands::clear_finished,
            commands::set_admin_ticket,
            commands::remote_status,
            commands::remote_list_dir,
            commands::remote_create_ticket,
            commands::remote_list_jobs,
            commands::remote_revoke,
            commands::has_admin_ticket,
            commands::qr_svg,
            commands::copy_text,
            commands::ui_ready,
            commands::share_folder,
            commands::list_quick_shares,
            commands::remove_quick_share,
            commands::set_context_menu,
            commands::set_auto_receive,
            commands::get_diagnostics,
            commands::add_firewall_rule,
            commands::test_connect,
            commands::set_net,
            commands::export_logs,
            commands::list_contacts,
            commands::create_invite,
            commands::revoke_invites,
            commands::parse_invite,
            commands::expand_link,
            commands::accept_invite,
            commands::rename_contact,
            commands::set_contact_auto_accept,
            commands::remove_contact,
            commands::send_to_contact,
            commands::list_incoming_pushes,
            commands::answer_push,
            commands::open_local_network_settings,
            commands::check_update,
            commands::install_update,
            commands::set_auto_update,
            commands::remote_update,
            commands::pick_files,
            commands::list_nearby,
            commands::rescan_nearby,
            commands::send_to,
            commands::send_text,
            commands::list_incoming,
            commands::answer_incoming,
            commands::set_receive_policy,
            commands::set_ls_enabled,
            commands::forget_trusted,
            commands::list_received,
            commands::resend,
            commands::web_share_start,
            commands::web_share_stop,
            commands::request_local_network,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            setup_core.attach(handle.clone());

            // v0.8 migration (once): autostart on, with `--hidden`.
            if let Err(e) = setup_core.migrate_v08() {
                tracing::warn!("0.8 migration: {e:#}");
            }

            // Notification "Đã nhận …" clicked → open that folder.
            {
                use tauri_plugin_notification::NotificationExt as _;
                use tauri_plugin_opener::OpenerExt as _;
                let h = handle.clone();
                let _ = app.notification().on_action(move |a| {
                    if let Some(dir) = core::clicked_folder(a) {
                        if let Err(e) = h.opener().open_path(dir, None::<&str>) {
                            tracing::warn!("open received folder: {e}");
                        }
                    }
                });
            }

            // Start the P2P node and the LocalSend bridge in the background; commands also
            // start the node lazily.
            let c = setup_core.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = c.node().await {
                    tracing::error!("node start failed: {e:#}");
                }
                if let Err(e) = c.start_ls().await {
                    tracing::error!("LocalSend start failed: {e:#}");
                }
                // v0.8: the receive folder may sit on a drive that comes and goes.
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    c.refresh_inbox();
                }
            });

            // Deep links: toss://t/<ticket>
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                #[cfg(any(windows, target_os = "linux"))]
                if cfg!(debug_assertions) {
                    let _ = app.deep_link().register_all();
                }
                let h = handle.clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        shell::deliver_url(&h, url.as_str());
                    }
                });
                if let Ok(Some(urls)) = app.deep_link().get_current() {
                    for url in urls {
                        shell::deliver_url(&handle, url.as_str());
                    }
                }
            }
            let argv: Vec<String> = std::env::args().collect();
            let cwd = std::env::current_dir().unwrap_or_default();
            shell::handle_args(&handle, &argv, &cwd);

            // Right-click "Tạo link Toss": (re)apply the saved choice, idempotent
            // (also repairs the entry after the app moved / was reinstalled).
            let menu_on = setup_core.settings().context_menu;
            std::thread::spawn(move || {
                if let Err(e) = ctxmenu::apply(menu_on) {
                    tracing::warn!("context menu ({menu_on}): {e:#}");
                }
            });

            shell::build_tray(&handle)?;

            // v0.5.1: update checks (60 s after start, then every 6 h).
            updater::spawn_background(&handle);

            if let Some(win) = app.get_webview_window(config::MAIN_WINDOW) {
                // Hidden start (autostart on a host machine) unless the setup wizard still has to run.
                if !start_hidden || setup_core.first_run() {
                    let _ = win.show();
                }
                let w = win.clone();
                let h = handle.clone();
                win.on_window_event(move |event| match event {
                    // Hide to tray instead of quitting.
                    WindowEvent::CloseRequested { api, .. } => {
                        api.prevent_close();
                        let _ = w.hide();
                    }
                    // Positions (CSS px, window-relative) let the UI hit-test the contact rows (v0.5).
                    WindowEvent::DragDrop(DragDropEvent::Enter { position, .. }) => {
                        let (x, y) = shell::css_pos(&w, position);
                        let _ = h.emit(
                            "drop:hover",
                            serde_json::json!({ "on": true, "x": x, "y": y }),
                        );
                    }
                    WindowEvent::DragDrop(DragDropEvent::Over { position }) => {
                        let (x, y) = shell::css_pos(&w, position);
                        let _ = h.emit("drop:over", serde_json::json!({ "x": x, "y": y }));
                    }
                    WindowEvent::DragDrop(DragDropEvent::Leave) => {
                        let _ = h.emit("drop:hover", serde_json::json!({ "on": false }));
                    }
                    WindowEvent::DragDrop(DragDropEvent::Drop { paths, position }) => {
                        let _ = h.emit("drop:hover", serde_json::json!({ "on": false }));
                        let (x, y) = shell::css_pos(&w, position);
                        let mut files = Vec::new();
                        for p in paths {
                            let s = p.to_string_lossy().into_owned();
                            match shell::ticket_from_file(&s) {
                                // a dropped .toss file behaves like a deep link (ticket or invite)
                                Some(t) => shell::deliver_link(&h, t),
                                None => files.push(s),
                            }
                        }
                        if !files.is_empty() {
                            let _ = h.emit(
                                "drop",
                                serde_json::json!({ "paths": files, "x": x, "y": y }),
                            );
                        }
                    }
                    _ => {}
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Toss");

    app.run(move |app, event| match event {
        // Keep running in the tray when the last window is closed/hidden.
        RunEvent::ExitRequested { api, code, .. } if code.is_none() => api.prevent_exit(),
        // Any real exit (tray "Thoát", OS logout/shutdown): pause transfers and close the
        // endpoint so `.part` files are complete up to what was received. Idempotent.
        RunEvent::Exit => {
            let core = core.clone();
            tauri::async_runtime::block_on(async move {
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(3), core.shutdown()).await;
            });
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => shell::show_main(app),
        // v0.6: files / folders dropped on the Dock icon → "Gửi tới ai?" (toss:// links are the
        // deep-link plugin's).
        #[cfg(target_os = "macos")]
        RunEvent::Opened { urls } => {
            let mut files = Vec::new();
            for u in urls.iter().filter(|u| u.scheme() == "file") {
                let Ok(p) = u.to_file_path() else { continue };
                let p = p.to_string_lossy().into_owned();
                match shell::ticket_from_file(&p) {
                    Some(t) => shell::deliver_link(app, t),
                    None => files.push(p),
                }
            }
            shell::deliver_send(app, files);
        }
        _ => {
            let _ = app;
        }
    });
}
