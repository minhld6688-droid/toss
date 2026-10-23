//! Window / tray / deep-link plumbing around the Tauri app.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde_json::{json, Value};

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};
use tauri_plugin_notification::NotificationExt as _;

use crate::{config, core::Core, ctxmenu::SHARE_FLAG};

/// Max size of a `.toss` file we are willing to read (a ticket is ~330 chars).
const MAX_TOSS_FILE: u64 = 64 * 1024;

/// An event for the UI: (name, payload).
type Event = (&'static str, Value);

#[derive(Default)]
pub struct Shell {
    /// UI readiness and the events (`deep-link`, `share`, `share:error`) queued until
    /// then, under one lock so an event arriving while the UI becomes ready is neither
    /// lost nor sent twice.
    queue: Mutex<Queue>,
    last: Mutex<Option<(String, Instant)>>,
}

#[derive(Default)]
struct Queue {
    ui_ready: bool,
    pending: Vec<Event>,
}

impl Shell {
    /// The UI registered its listeners: flush events that arrived during startup.
    pub fn mark_ready(&self, app: &AppHandle) {
        for (name, payload) in self.take_ready() {
            let _ = app.emit(name, payload);
        }
    }

    /// Marks the UI ready and returns the queued events to emit, in order.
    fn take_ready(&self) -> Vec<Event> {
        let mut q = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        q.ui_ready = true;
        std::mem::take(&mut q.pending)
    }

    /// Queues the event unless the UI is ready; true = emit it now.
    fn offer(&self, name: &'static str, payload: &Value) -> bool {
        let mut q = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        if !q.ui_ready {
            q.pending.push((name, payload.clone()));
        }
        q.ui_ready
    }

    /// False if the same key (ticket, share path) was delivered less than 2 s ago
    /// (argv and the deep-link plugin both report the link that launched the app).
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

/// Emit `name` now, or queue it until the UI is ready.
fn deliver(app: &AppHandle, name: &'static str, payload: Value) {
    if app.state::<Shell>().offer(name, &payload) {
        let _ = app.emit(name, payload);
    }
}

/// Hand a ticket (or `toss://` URL body) to the UI, queueing it until the UI is ready.
pub fn deliver_ticket(app: &AppHandle, ticket: String) {
    if !app.state::<Shell>().first_delivery(&ticket) {
        return;
    }
    show_main(app);
    deliver(app, "deep-link", json!({ "ticket": ticket }));
}

/// Right-click "Tạo link Toss": `share {path}` for a folder, `share:error {msg}` otherwise.
pub fn deliver_share(app: &AppHandle, raw: &str, cwd: &Path) {
    if !app
        .state::<Shell>()
        .first_delivery(&format!("share\u{0}{raw}"))
    {
        return;
    }
    show_main(app);
    match share_target(raw, cwd) {
        Ok(p) => deliver(app, "share", json!({ "path": p.to_string_lossy() })),
        Err(msg) => {
            tracing::warn!("--share {raw:?}: {msg}");
            deliver(app, "share:error", json!({ "msg": msg }));
        }
    }
}

/// The folder after `--share` (or `--share=<folder>`), if any. Explorer passes a drive
/// root's background as `"D:\"`, which Windows argv parsing turns into `D:"`: repaired.
pub fn share_arg(args: &[String]) -> Option<String> {
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        let v = if a == SHARE_FLAG {
            it.next().cloned()
        } else {
            a.strip_prefix(SHARE_FLAG)
                .and_then(|r| r.strip_prefix('='))
                .map(str::to_string)
        };
        if let Some(v) = v {
            let mut v = v.trim().trim_end_matches('"').to_string();
            if v.len() == 2 && v.ends_with(':') {
                v.push('\\');
            }
            return Some(v);
        }
    }
    None
}

/// Validates a `--share` argument: must be an existing directory (relative paths are
/// taken from `cwd`). Returns the canonical path, or a Vietnamese message for the UI.
pub fn share_target(raw: &str, cwd: &Path) -> Result<PathBuf, String> {
    if raw.trim().is_empty() {
        return Err("Chưa chọn thư mục để chia sẻ".into());
    }
    let p = Path::new(raw.trim());
    let p = if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    };
    if !p.exists() {
        return Err(format!("Không tìm thấy “{raw}”"));
    }
    if !p.is_dir() {
        return Err(format!(
            "“{raw}” là file, không phải thư mục. Chuột phải vào một thư mục để tạo link Toss."
        ));
    }
    toss_core::paths::canonicalize(&p).map_err(|e| format!("Không đọc được “{raw}”: {e:#}"))
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
    // toss://i/<invite> (and a bare toss://tossinv1…) is a pairing invite, not a ticket.
    let is_invite_path = body
        .get(..2)
        .is_some_and(|p| p.eq_ignore_ascii_case("i/"));
    if is_invite_path || starts_with_invite(body) {
        return None;
    }
    let body = body.trim_matches('/').trim();
    (!body.is_empty()).then(|| body.to_string())
}

const INVITE_PREFIX: &str = "tossinv1";

fn starts_with_invite(s: &str) -> bool {
    s.get(..INVITE_PREFIX.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(INVITE_PREFIX))
}

/// Pairing invite (`tossinv1…`) inside a `toss://i/…` deep link, a bare toss://tossinv1…, a web link
/// `https://host/#tossinv1…` or pasted text. Returns it lower-cased (base32); None for anything else.
pub fn invite_from_url(text: &str) -> Option<String> {
    let lower = text.trim().to_ascii_lowercase();
    let at = lower.find(INVITE_PREFIX)?;
    let before = &lower[..at];
    if !(before.is_empty() || before == "toss://i/" || before == "toss://" || before.ends_with('#')) {
        return None;
    }
    let rest: String = lower[at + INVITE_PREFIX.len()..]
        .chars()
        .take_while(|c| c.is_ascii_lowercase() || ('2'..='7').contains(c))
        .collect();
    (rest.len() >= 16).then(|| format!("{INVITE_PREFIX}{rest}"))
}

/// Hand an invite to the UI (`invite {invite}`), queued until the UI is ready.
pub fn deliver_invite(app: &AppHandle, invite: String) {
    if !app.state::<Shell>().first_delivery(&invite) {
        return;
    }
    show_main(app);
    deliver(app, "invite", json!({ "invite": invite }));
}

/// Route a deep-link URL: invite -> `invite`, ticket -> `deep-link`; anything else is ignored.
pub fn deliver_url(app: &AppHandle, url: &str) {
    if let Some(i) = invite_from_url(url) {
        deliver_invite(app, i);
    } else if let Some(t) = ticket_from_url(url) {
        deliver_ticket(app, t);
    }
}

/// Same for text that is already unwrapped (the content of a dropped `.toss` file).
pub fn deliver_link(app: &AppHandle, text: String) {
    match invite_from_url(&text) {
        Some(i) => deliver_invite(app, i),
        None => deliver_ticket(app, text),
    }
}

/// Window-relative drag position in CSS pixels (what `elementFromPoint` wants).
pub fn css_pos(w: &tauri::WebviewWindow, p: &tauri::PhysicalPosition<f64>) -> (f64, f64) {
    let f = w.scale_factor().unwrap_or(1.0).max(0.1);
    ((p.x / f).round(), (p.y / f).round())
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

/// Looks through process arguments for `--share <folder>`, `toss://` links and `.toss`
/// files. `cwd` is the launching process' working directory. True if one was handled.
pub fn handle_args(app: &AppHandle, args: &[String], cwd: &Path) -> bool {
    let mut handled = false;
    if let Some(dir) = share_arg(args) {
        deliver_share(app, &dir, cwd);
        return true;
    }
    for a in args.iter().skip(1) {
        if let Some(i) = invite_from_url(a) {
            deliver_invite(app, i);
            handled = true;
        } else if let Some(t) = ticket_from_url(a).or_else(|| ticket_from_file(a)) {
            deliver_link(app, t);
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
    fn invites_are_not_tickets() {
        let inv = "tossinv1mfrggzdfmz2gq5dsnfxgo3tp";
        assert_eq!(invite_from_url(&format!("toss://i/{inv}")), Some(inv.into()));
        assert_eq!(invite_from_url(&format!("  TOSS://I/{}/  ", inv.to_uppercase())), Some(inv.into()));
        assert_eq!(invite_from_url(inv), Some(inv.into()));
        assert_eq!(
            invite_from_url(&format!("https://host.dev/toss/#{inv}")),
            Some(inv.into())
        );
        assert_eq!(invite_from_url("tossinv1abc"), None, "too short");
        assert_eq!(invite_from_url("toss://t/toss1abcdefghijklmnop"), None);
        assert_eq!(invite_from_url(&format!("xtossinv1{}", &inv[8..])), None);
        // the ticket route must leave them alone
        assert_eq!(ticket_from_url(&format!("toss://i/{inv}")), None);
        assert_eq!(ticket_from_url(&format!("toss://{inv}")), None);
        assert_eq!(ticket_from_url("toss://ỉ/Ảnh"), Some("ỉ/Ảnh".into()));
    }

    #[test]
    fn deep_link_queue() {
        let sh = Shell::default();
        assert!(sh.first_delivery("a"));
        assert!(!sh.first_delivery("a"), "argv + plugin duplicate");
        assert!(sh.first_delivery("b"));
        // Before the UI is ready: queued, in order (deep links and shares alike).
        assert!(!sh.offer("deep-link", &json!({ "ticket": "a" })));
        assert!(!sh.offer("share", &json!({ "path": "/x" })));
        assert!(!sh.offer("deep-link", &json!({ "ticket": "b" })));
        assert_eq!(
            sh.take_ready(),
            vec![
                ("deep-link", json!({ "ticket": "a" })),
                ("share", json!({ "path": "/x" })),
                ("deep-link", json!({ "ticket": "b" })),
            ]
        );
        // After: emitted directly, nothing left behind.
        assert!(sh.offer("share:error", &json!({ "msg": "m" })));
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
                    .filter(|i| s2.offer("deep-link", &json!(i.to_string())))
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
            });
            let flushed: Vec<String> = sh
                .take_ready()
                .into_iter()
                .map(|(_, v)| v.as_str().unwrap().to_string())
                .collect();
            let direct = offerer.join().unwrap();
            let mut all: Vec<String> = flushed.into_iter().chain(direct).collect();
            all.sort_by_key(|s| s.parse::<u32>().unwrap());
            assert_eq!(all, (0..20).map(|i| i.to_string()).collect::<Vec<_>>());
        }
    }

    fn argv(a: &[&str]) -> Vec<String> {
        std::iter::once("Toss")
            .chain(a.iter().copied())
            .map(String::from)
            .collect()
    }

    #[test]
    fn share_argument() {
        assert_eq!(
            share_arg(&argv(&["--share", r"C:\Studio\12A3"])),
            Some(r"C:\Studio\12A3".into())
        );
        assert_eq!(
            share_arg(&argv(&["--share=/Users/m/Ảnh lớp"])),
            Some("/Users/m/Ảnh lớp".into())
        );
        // Explorer background of a drive root: "%V" = `D:\` → argv `D:"`.
        assert_eq!(share_arg(&argv(&["--share", "D:\""])), Some(r"D:\".into()));
        assert_eq!(share_arg(&argv(&["--share"])), None);
        assert_eq!(share_arg(&argv(&["toss://t/toss1abc"])), None);
        assert_eq!(share_arg(&["--share".into(), "/x".into()]), None); // argv[0] is the exe
    }

    #[test]
    fn share_target_must_be_a_folder() {
        let dir = std::env::temp_dir().join(format!("toss-share-arg-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Lớp 12A3")).unwrap();
        std::fs::write(dir.join("a.jpg"), b"x").unwrap();
        let ok = share_target(dir.join("Lớp 12A3").to_str().unwrap(), Path::new("/")).unwrap();
        assert!(ok.is_absolute() && ok.ends_with("Lớp 12A3"));
        // relative to the launching process' cwd
        assert_eq!(share_target("Lớp 12A3", &dir).unwrap(), ok);
        let file = share_target(dir.join("a.jpg").to_str().unwrap(), Path::new("/"));
        assert!(file.unwrap_err().contains("không phải thư mục"));
        assert!(
            share_target(dir.join("nope").to_str().unwrap(), Path::new("/"))
                .unwrap_err()
                .contains("Không tìm thấy")
        );
        assert!(share_target("  ", Path::new("/")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
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
