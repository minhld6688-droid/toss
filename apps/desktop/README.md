# Toss desktop

Tauri 2 shell (`src-tauri`, crate `toss-desktop`) + React 18 / TypeScript / Vite / Tailwind UI (`src`) around the `toss-core` crate.
UI language: Vietnamese. Code and docs: English. Contracts: `../../contracts/` (`ipc.ts` for UI ↔ shell, `core-api.md` for shell ↔ core).

```
apps/desktop
├─ src/                 React UI
│  ├─ lib/ipc-types.ts  verbatim copy of contracts/ipc.ts
│  ├─ lib/ipc.ts        invoke()/listen() wrapper (real) or in-memory mock
│  ├─ lib/mock.ts       mock shell: fake tickets, host mode, remote admin, transfers with progress
│  ├─ screens/          Home · Transfers · Settings   (+ components/: TicketSheet, LinkMaker, JobList, QrLink, Wizard)
├─ src-tauri/src/
│  ├─ core.rs           the ONLY file using toss-core (adapter)
│  ├─ commands.rs       every command of ipc.ts, snake_case names/args
│  ├─ shell.rs          tray, deep links, `.toss` files, hide-to-tray helpers
│  ├─ settings.rs       settings.json
│  ├─ config.rs         DEFAULT_PAGE_HOST (change the link-page host here) and other constants
│  └─ ipc.rs            serde mirrors of the IPC types
├─ app-icon.svg         source of src-tauri/icons (`npx tauri icon app-icon.svg`)
├─ scripts/screenshots.mjs   Playwright screenshots of the mock UI -> screenshots/
```

## Run

Requirements: Node 22, Rust (stable). Linux also needs `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev`.

```sh
cd apps/desktop
npm install
```

**UI only, mock mode** (no Rust; simulates transfers, host mode and a remote host):

```sh
VITE_MOCK=1 npm run dev          # or: npm run dev:mock   -> http://127.0.0.1:1420
```

Mock URL switches: `?host=1` host mode on with sample roots · `?remote=1` admin ticket already stored · `?wizard=1` first-run wizard · `?busy=1` a transfer waiting on a busy host.
Opening the page in a plain browser (no Tauri runtime) falls back to the mock automatically; a "MOCK" banner is shown.

**Real app** (Tauri dev window + toss-core):

```sh
npm run tauri:dev                # = tauri dev; starts Vite on :1420 and the Rust shell
```

Two instances on one machine for testing: set a different data dir per instance and run the binary directly, e.g. `TOSS_DATA_DIR=/tmp/toss-b ./target/debug/toss-desktop` (single-instance forwarding is per user session, so start the second one from another user/session or temporarily without the plugin).
Log level: `RUST_LOG=info,toss_core=debug`.

## Build

```sh
npm run build                    # tsc --noEmit + vite build -> dist/
npm run tauri:build              # installers: .dmg (macOS), NSIS .exe (Windows) -> ../../target/release/bundle/
cd ../.. && cargo check -p toss-desktop     # Rust shell only
cargo test  -p toss-desktop --lib           # adapter test against the real toss-core (offline)
node apps/desktop/scripts/screenshots.mjs   # regenerate screenshots/*.png (mock UI)
```

Not needed for v1: updater, code signing (configure in CI).

## Behaviour

- **Window** 400×600 (min 360×480); closing hides to the tray. Tray menu: `Mở Toss` · `Trạng thái` · `Thoát`. `--hidden` starts without showing the window (autostart passes it; the first-run wizard still shows).
- **Links**: `toss://t/<ticket>` (deep-link scheme `toss`, single-instance forwards argv), `https://<page_host>/#<ticket>`, raw `toss1…`, or a dropped/opened `.toss` text file. All end in the same confirm sheet: **Nhận** (get) and/or a drop zone **Kéo folder vào để trả** (put). Admin tickets offer "Dùng làm máy tổng từ xa".
- **Native notifications** when a transfer finishes or fails while the window is not focused.
- **Host mode** is opt-in (Cài đặt → Chế độ máy tổng). Settings changes apply through `set_host`; roots must exist, aliases `[A-Za-z0-9_-]{1,32}`.
- **Device name** change restarts the P2P node (same identity), deferred to next launch if a transfer is running.

## Where things live

| What | Where |
| --- | --- |
| Settings (`device_name`, `dest_dir`, `host`, `admin_ticket`, `autostart`, `first_run`, optional `page_host`) | `<data_dir>/settings.json` |
| Node key (`key`), `jobs.json`, `revoked.json` (toss-core) | `<data_dir>/` |
| Received files | `dest_dir/<job>/` (default `~/Toss`) |
| Recent links in the UI | WebView `localStorage` (`toss.recents.v1`) — convenience only |

`<data_dir>` = `$TOSS_DATA_DIR` if set, else `dirs::data_dir()/toss`:
`~/Library/Application Support/toss` (macOS) · `%APPDATA%\toss` (Windows) · `~/.local/share/toss` (Linux).
`page_host` defaults to `DEFAULT_PAGE_HOST` in `src-tauri/src/config.rs`; hand-edit `settings.json` (`"page_host": "…"`) to override without rebuilding.

## IPC extras beyond contracts/ipc.ts

`ui_ready`, `has_admin_ticket` (commands) and `navigate`, `drop:hover` (events) — see `src/lib/ipc-extra.ts` and `contracts/CHANGES.md`.
