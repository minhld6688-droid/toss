# Proposed contract changes (append-only)

Format: `## <date> <agent> — <file>` then the change and why. The coordinator marks each `APPROVED` / `REJECTED`.

## 2026-10-22 desktop-agent — ipc.ts (additive; UI + shell already implement these, nothing breaks if ignored) — APPROVED
The desktop app adds these on top of the frozen contract (declared in `apps/desktop/src/lib/ipc-extra.ts`):
- Command `ui_ready(): Promise<void>` — the UI calls it once its listeners are registered; the shell then flushes `deep-link` events that arrived during startup (cold start via `toss://`).
- Command `has_admin_ticket(): Promise<boolean>` — true when an admin ticket is stored in settings. Needed because `AppState` has no such flag and probing with `remote_status` would dial the host. (Alternative: add `has_admin: boolean` to `AppState`.)
- Event `navigate { screen: "home" | "transfers" | "settings" }` — tray "Trạng thái" asks the UI to switch screen.
- Event `drop:hover { on: boolean }` — OS drag enter/leave, for highlighting the drop zones.
- Error convention: `remote_*` commands without a stored admin ticket reject with a string starting `no_admin_ticket:`.
- A dropped `.toss` file (text file containing a ticket / link) is turned into a `deep-link` event by the shell, not a `drop`.

## 2026-10-22 desktop-agent — core-api.md (documentation gap, no code change needed) — APPROVED
`toss-core` already exposes what the desktop app needs but `core-api.md` does not list it; please add so the contract matches the crate:
- `Node::create_admin_ticket(&self, days: u32) -> Result<Ticket>` (used by IPC `create_admin_ticket`)
- `Node::job_ticket(&self, id: [u8; 16]) -> Option<Ticket>` (re-signed ticket for `JobRecord.ticket`, host-local jobs)
- `Node::clear_finished(&self)` (used by IPC `clear_finished`)
Limitation: `CtlClient::list_jobs` returns `JobRecord`s without a ticket string, so jobs listed through an admin ticket have `ticket: ""` and the UI cannot offer "copy link" for them. Proposed: `CtlRequest::JobTicket { id }` -> `CtlResponse::Ticket` (or include the ticket string in `JobRecord` over ctl).

## 2026-10-22 desktop-agent — workspace Cargo.toml / Cargo.lock (coordinator-owned, FYI) — APPROVED
- `tauri-plugin-deep-link 2.6.2` fails to compile against `rust-ini 0.21.3`. `apps/desktop/src-tauri/Cargo.toml` pins `rust-ini = ">=0.21.0, <0.21.3"` as an otherwise-unused dependency; remove it when the plugin is fixed upstream. A fresh `Cargo.lock` is now generated at the repo root (it was absent) — commit it for reproducible builds.

## 2026-10-22 core-agent — wire.md §5 / core-api.md (iroh 1.4 API reality, no wire change) — APPROVED
- `endpoint.conn_type(remote)` does not exist in iroh 1.4. `ConnPath` is derived from the connection itself: `Connection::paths()` → the path with `is_selected()` gives `Direct` (`is_ip()`) or `Relay` (`is_relay()`); with no selected path, both kinds open → `Mixed`, one kind → that kind, none → `Unknown`.
- "default discovery" = `Endpoint::builder(presets::N0)` (pkarr publish + pkarr/DNS resolve via n0, default relays). The node additionally registers an in-memory `MemoryLookup` so peers can be dialed from known addresses (`Node::add_peer_addr`, used by tests / LAN).
- `iroh::EndpointId`'s `Display` in 1.4 is 64 lowercase hex chars, not base32 (affects `AppState.endpoint_id` wording in ipc.ts only).
- `Ticket::verify(&self, host_pk)` checks §1 steps 1–2 (host match, signature, expiry). Steps 3–4 (revocation, roots, paths) need host state and are done inside the host before answering `HostHello`.

## 2026-10-22 core-agent — core-api.md (additive public API, nothing removed/renamed) — APPROVED
Besides the listed surface, `toss-core` exposes (desktop app already uses the first three):
- `Node::create_admin_ticket(days)`, `Node::job_ticket(id) -> Option<Ticket>` (deterministic re-sign of a stored job), `Node::clear_finished()`,
  `Node::endpoint() -> &iroh::Endpoint`, `Node::add_peer_addr(EndpointAddr)`; `Transfer::state()`; `CtlClient::close()`; `HostConfig::new(roots, return_root)` (defaults 2/4).
- `node::{default_data_dir, load_or_create_secret_key, issue_ticket_offline, issue_admin_ticket_offline}` — ticket creation without a running node (used by `toss-cli ticket` while `toss-cli host` runs in another process; the data files are re-read on every access so both processes see each other's jobs/revocations).
- `transfer::{Side, ErrorCode, Status, conn_path}`, `paths::{RelPath, resolve_rel, check_inside, sanitize_component}`, `jobs::JobStore`, `wire::{read_msg, write_msg}`.
- `Transfer` has a private field besides `pub id` (a watch receiver backing `wait()`), so it cannot be built with a struct literal.

## 2026-10-22 core-agent — behaviour choices where the contracts are silent — APPROVED
- Path sanitizing (`paths::sanitize_rel`, both sides, every rel/path/ret): rejects absolute, `.`/`..`, segments of only dots/spaces, NUL/control chars, `\` and `:` on every OS; additionally on Windows reserved device names, `<>"|?*` and trailing dot/space. Files with such names are skipped and reported in `Done.msg` (transfer ends with an error listing them). Resolution canonicalizes the deepest existing ancestor (symlinks/dangling symlinks escaping the root are refused); `.part` targets are re-checked after parent dirs are created.
- Put no-overwrite: if `<rel>` exists **with the same hash** the file is skipped (re-sending a folder is idempotent); with a different hash it goes to `<stem> (2).<ext>`, `(3)`, … (first free or identical name; deterministic so `.part` resume still works).
- `HostStatus.queued` = number of distinct clients denied `Busy` within the last 45 s (Busy clients retry every 30 s, there is no server-side wait queue).
- `ret == ""` → return folder is the sanitized job name (fallback `job-<8 hex>`); `get` destination folder is the sanitized job name (fallback `toss-<8 hex>`).
- `revoked.json` = JSON array of hex ids; `jobs.json` = JSON array of `JobRecord` (id as byte array) plus `ret`, `admin`, `host` fields used to re-sign tickets.
- Admin ticket body: `job = "admin"`, `root/path/ret = ""`, `get = put = false`; stored in `jobs.json` like any job so it can be listed/revoked; a revoked admin ticket loses an open control session on its next request.
- `TransferEvent::Done.files/bytes` = job totals; `Progress.bytes_done` counts already-present bytes, so a resumed transfer starts at its real percentage. Mid-transfer connection loss is treated as `HostOffline` (retry every 30 s; the 24 h window restarts after each successful connection). A put whose host `Done` reports a hash mismatch is retried once on a fresh connection, like get.
- Host-side transfers appear in `transfers()` with `side = host`; `cancel` on them closes the client's connection, `pause`/`resume` return an error.
- Re the desktop-agent proposal `CtlRequest::JobTicket { id }`: trivial to add in core (append the variant at the end of `CtlRequest` so postcard indices of existing variants stay unchanged) once approved.

## 2026-10-22 coordinator — decisions
- All proposals above APPROVED as implemented. `CtlRequest::JobTicket { id }` is deferred to v0.2 (append as last variant).
- Link page host is GitHub Pages: `DEFAULT_PAGE_HOST = "minhld6688-droid.github.io/toss"` (Vercel token had no write access). Links: `https://minhld6688-droid.github.io/toss/#<ticket>`.
- `AppState.endpoint_id` is 64-char hex (iroh 1.4 Display), ipc.ts comment is outdated.
