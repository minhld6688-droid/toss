//! Window / tray / deep-link plumbing around the Tauri app.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};
use tauri_plugin_notification::NotificationExt as _;

use crate::{config, core::Core};

/// Max size of a `.toss` file we are willing to read (a ticket is ~330 chars).
const MAX_TOSS_FILE: u64 = 64 * 1024;

#[derive(Default)]
pub struct Shell {
    /// UI readiness and the deep links queued until then, under one lock so a
    /// link arriving while the UI becomes ready is neither lost nor sent twice.
    queue: Mutex<Queue>,
    last: Mutex<Option<(String, Instant)>>,
}

#[derive(Default)]
struct Queue {
    ui_ready: bool,
    pending: Vec<String>,
}

impl Shell {
    /// The UI registered its listeners: flush deep links that arrived during startup.
    pub fn mark_ready(&self, app: &AppHandle) {
        for t in self.take_ready() {
            let _ = app.emit("deep-link", serde_json::json!({ "ticket": t }));
        }
    }

    /// Marks the UI ready and returns the queued tickets to emit.
    fn take_ready(&self) -> Vec<String> {
        let mut q = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        q.ui_ready = true;
        std::mem::take(&mut q.pending)
    }

    /// Queues `ticket` unless the UI is ready; true = emit it now.
    fn offer(&self, ticket: &str) -> bool {
        let mut q = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        if !q.ui_ready {
            q.pending.push(ticket.to_string());
        }
        q.ui_ready
    }

    /// False if the same ticket was delivered less than 2 s ago (argv and the
    /// deep-link plugin both report the link that launched the app).
    fn first_delivery(&self, ticket: &str) -> bool {
        let mut last = self.last.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((t, at)) = last.as_ref() {
            if t == ticket && at.elapsed() < Duration::from_secs(2) {
                return false;
            }
        }
        *last = Some((ticket.to_string(), Instant::now()));
        true
    }
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(config::MAIN_WINDOW) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Hand a ticket (or `toss://` URL body) to the UI, queueing it until the UI is ready.
pub fn deliver_ticket(app: &AppHandle, ticket: String) {
    let shell = app.state::<Shell>();
    if !shell.first_delivery(&ticket) {
        return;
    }
    show_main(app);
    if shell.offer(&ticket) {
        let _ = app.emit("deep-link", serde_json::json!({ "ticket": ticket }));
    }
}

/// `toss://t/<ticket>` -> `<ticket>`.
pub fn ticket_from_url(url: &str) -> Option<String> {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    let rest = lower.strip_prefix("toss://")?;
    let off = u.len() - rest.len();
    let body = &u[off..];
    let body = body
        .strip_prefix("t/")
        .or_else(|| body.strip_prefix("T/"))
        .unwrap_or(body);
    let body = body.trim_matches('/').trim();
    (!body.is_empty()).then(|| body.to_string())
}

/// Reads a `.toss` file (a text file containing a ticket or link).
pub fn ticket_from_file(path: &str) -> Option<String> {
    if !path.to_ascii_lowercase().ends_with(".toss") {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_TOSS_FILE {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    Some(ticket_from_url(t).unwrap_or_else(|| t.to_string()))
}

/// Looks through process arguments for `toss://` links and `.toss` files. True if one was handled.
pub fn handle_args(app: &AppHandle, args: &[String]) -> bool {
    let mut handled = false;
    for a in args.iter().skip(1) {
        if let Some(t) = ticket_from_url(a).or_else(|| ticket_from_file(a)) {
            deliver_ticket(app, t);
            handled = true;
        }
    }
    handled
}

pub fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Mở Toss", true, None::<&str>)?;
    let status = MenuItem::with_id(app, "status", "Trạng thái", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Thoát", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &status, &sep, &quit])?;

    let mut b = TrayIconBuilder::with_id("toss")
        .tooltip("Toss")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "status" => {
                show_main(app);
                let _ = app.emit("navigate", serde_json::json!({ "screen": "transfers" }));
                let app = app.clone();
                tauri::async_runtime::spawn(async move { notify_host_status(&app).await });
            }
            "quit" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let core = app.state::<Arc<Core>>().inner().clone();
                    let _ = tokio::time::timeout(Duration::from_secs(3), core.shutdown()).await;
                    app.exit(0);
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        b = b.icon(icon.clone());
    }
    b.build(app)?;
    Ok(())
}

/// Tray "Trạng thái" on a host machine: a native notification with the host summary.
async fn notify_host_status(app: &AppHandle) {
    let core = app.state::<Arc<Core>>().inner().clone();
    if !core.host_mode() {
        return;
    }
    let body = match core.host_status().await {
        Ok(v) if !v.is_null() => format!(
            "Máy tổng “{}” · {} đang chạy · {} chờ · {} thư mục gốc",
            v["device"].as_str().unwrap_or(""),
            v["active"],
            v["queued"],
            v["roots"].as_array().map(|r| r.len()).unwrap_or(0)
        ),
        Ok(_) => "Chế độ máy tổng đang tắt".to_string(),
        Err(e) => format!("Không lấy được trạng thái: {e:#}"),
    };
    let _ = app.notification().builder().title("Toss").body(body).show();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_to_ticket() {
        assert_eq!(
            ticket_from_url("toss://t/toss1abc"),
            Some("toss1abc".into())
        );
        assert_eq!(
            ticket_from_url("TOSS://T/toss1abc/"),
            Some("toss1abc".into())
        );
        assert_eq!(
            ticket_from_url("  toss://t/toss1AbC  "),
            Some("toss1AbC".into())
        );
        assert_eq!(ticket_from_url("toss://t/"), None);
        assert_eq!(ticket_from_url("https://x/#toss1abc"), None);
        assert_eq!(ticket_from_url("--hidden"), None);
    }

    #[test]
    fn deep_link_queue() {
        let sh = Shell::default();
        assert!(sh.first_delivery("a"));
        assert!(!sh.first_delivery("a"), "argv + plugin duplicate");
        assert!(sh.first_delivery("b"));
        // Before the UI is ready: queued, in order.
        assert!(!sh.offer("a"));
        assert!(!sh.offer("b"));
        assert_eq!(sh.take_ready(), vec!["a".to_string(), "b".to_string()]);
        // After: emitted directly, nothing left behind.
        assert!(sh.offer("c"));
        assert!(sh.take_ready().is_empty());
    }

    #[test]
    fn deep_link_racing_ui_ready_is_delivered_once() {
        // Links offered while another thread marks the UI ready end up either
        // emitted directly or in the flushed queue: never both, never neither.
        for _ in 0..200 {
            let sh = Arc::new(Shell::default());
            let s2 = sh.clone();
            let offerer = std::thread::spawn(move || {
                (0..20)
                    .filter(|i| s2.offer(&i.to_string()))
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
            });
            let flushed = sh.take_ready();
            let direct = offerer.join().unwrap();
            let mut all: Vec<String> = flushed.into_iter().chain(direct).collect();
            all.sort_by_key(|s| s.parse::<u32>().unwrap());
            assert_eq!(all, (0..20).map(|i| i.to_string()).collect::<Vec<_>>());
        }
    }

    #[test]
    fn toss_file() {
        let dir = std::env::temp_dir().join(format!("toss-desktop-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("job.toss");
        std::fs::write(&f, "toss://t/toss1xyz\n").unwrap();
        assert_eq!(
            ticket_from_file(f.to_str().unwrap()),
            Some("toss1xyz".into())
        );
        let g = dir.join("notes.txt");
        std::fs::write(&g, "toss1xyz").unwrap();
        assert_eq!(ticket_from_file(g.to_str().unwrap()), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
