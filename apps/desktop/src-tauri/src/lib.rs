//! Toss desktop shell: Tauri 2 app around `toss-core`.

mod commands;
pub mod config;
mod core;
mod ipc;
mod settings;
mod shell;

use std::sync::Arc;

use tauri::{DragDropEvent, Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;

use crate::{config::HIDDEN_FLAG, core::Core, shell::Shell};

pub fn run() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,iroh=warn,quinn=warn"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let core = Arc::new(Core::new());
    let start_hidden = std::env::args().any(|a| a == HIDDEN_FLAG);

    let setup_core = core.clone();
    let app = tauri::Builder::default()
        // single-instance must be registered first: forwards argv (deep links on Windows/Linux).
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            let handled = shell::handle_args(app, &argv);
            if !handled && !argv.iter().any(|a| a == HIDDEN_FLAG) {
                shell::show_main(app);
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![HIDDEN_FLAG])))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
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
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            setup_core.attach(handle.clone());

            // Start the P2P node in the background; commands also start it lazily.
            let c = setup_core.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = c.node().await {
                    tracing::error!("node start failed: {e:#}");
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
                        if let Some(t) = shell::ticket_from_url(url.as_str()) {
                            shell::deliver_ticket(&h, t);
                        }
                    }
                });
                if let Ok(Some(urls)) = app.deep_link().get_current() {
                    for url in urls {
                        if let Some(t) = shell::ticket_from_url(url.as_str()) {
                            shell::deliver_ticket(&handle, t);
                        }
                    }
                }
            }
            let argv: Vec<String> = std::env::args().collect();
            shell::handle_args(&handle, &argv);

            shell::build_tray(&handle)?;

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
                    WindowEvent::DragDrop(DragDropEvent::Enter { .. }) => {
                        let _ = h.emit("drop:hover", serde_json::json!({ "on": true }));
                    }
                    WindowEvent::DragDrop(DragDropEvent::Leave) => {
                        let _ = h.emit("drop:hover", serde_json::json!({ "on": false }));
                    }
                    WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) => {
                        let _ = h.emit("drop:hover", serde_json::json!({ "on": false }));
                        let mut files = Vec::new();
                        for p in paths {
                            let s = p.to_string_lossy().into_owned();
                            match shell::ticket_from_file(&s) {
                                // a dropped .toss file behaves like a deep link
                                Some(t) => shell::deliver_ticket(&h, t),
                                None => files.push(s),
                            }
                        }
                        if !files.is_empty() {
                            let _ = h.emit("drop", serde_json::json!({ "paths": files }));
                        }
                    }
                    _ => {}
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Toss");

    app.run(|app, event| match event {
        // Keep running in the tray when the last window is closed/hidden.
        RunEvent::ExitRequested { api, code, .. } if code.is_none() => api.prevent_exit(),
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => shell::show_main(app),
        _ => {
            let _ = app;
        }
    });
}
