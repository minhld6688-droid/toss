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

## 2026-10-22 core-agent (review pass) — wire.md §4 / core-api.md / ipc.ts (additive)
- `CtlRequest::JobTicket { id: [u8; 16] }` → `CtlResponse::Ticket(Ticket)`, appended as the **last** `CtlRequest` variant (postcard index 6; indices of existing variants unchanged; no new `CtlResponse` variant). The host re-signs the stored job body (identical string to the one handed out at creation). Refused with `Denied{NotAllowed}` for admin tickets (a holder could otherwise keep control after its own admin ticket is revoked) and `Denied{Other("unknown job")}` for unknown ids. Old hosts close the control connection on the unknown variant.
- `CtlClient::job_ticket(&self, id: [u8; 16]) -> Result<Ticket>`. The desktop `remote_list_jobs` fills `JobRecord.ticket` with it for usable jobs (not revoked/expired/admin), caches per id, verifies id/host/signature, and leaves `""` (stops asking) if the host does not support the request.
- `HostConfig.bind_port: Option<u16>` (serde default, omitted when `None`) — fixed UDP port for IPv4 `0.0.0.0:port` (+ `[::]:port` if IPv6 is available), e.g. for a router port forward. `None` = random port (unchanged default). The endpoint binds at start, so it is applied by the new `Node::start_with_port(cfg, port)` (`Node::start(cfg)` = `start_with_port(cfg, None)`); `Node::set_host` stores a different port but only logs that a restart is needed. `Node::bind_port() -> Option<u16>`. CLI: `toss-cli host --port N`.
- ipc.ts: `HostConfig` gains optional `bind_port?: number` (absent/null = random). The desktop shell persists it in settings, starts the node with it, restarts the node when it changes (deferred to next launch while transfers are active), and falls back to a random port if the fixed one cannot be bound.

## 2026-10-22 core-agent (review pass) — behaviour (no wire change)
- Pre-auth limits: the first message of a connection (`ClientHello`, ctl `Auth`) is capped at `wire::MAX_HELLO` = 64 KiB and must arrive within 15 s; framed reads no longer pre-allocate the announced length.
- Stalled peers: a transfer with no progress for 120 s (no chunk read/written, no next stream while none is running, no `Done`) ends as a connection error (client retries as `HostOffline`); the host waits up to 1 h for a client's `Want` (it may be hashing a big resumed job).
- Put targets are reserved host-wide while a put runs: two concurrent uploads of the same name get `<name>` and `<stem> (2).<ext>` and never share a `.part`; a target that appears during the transfer is not replaced (reported as a mismatch → the client's one retry picks a free name).
- A damaged `revoked.json` fails closed (every ticket denied with `Other("host revocation list is unreadable")`, `revoke` refuses to overwrite it); a damaged `jobs.json` is moved to `jobs.json.bad-<unix>` before a fresh one is written.
- Peer device names / app versions are stored without control characters, ≤ 64 chars; a client's `Done.msg` ≤ 2000 chars in `jobs.json`.
- A cancelled transfer stays `cancelled` with a single final event (host-side cancel previously turned into `error`). Starting a get for a job that is already in progress into the same folder returns the running transfer (resuming it if paused) instead of a second one sharing its `.part` files.

## 2026-10-22 core-agent — wire.md §1 + §5: ticket v2 with host addresses (coordinator pre-approved)
Why: the real-network e2e (Windows host, macOS client) never connected: the client dialed the bare `EndpointId` and depended entirely on n0 discovery (host pkarr publish → dns.iroh.link → client pkarr/DNS resolve) and on the host's relay actually being connected. Neither is visible in our logs (iroh logs `home is now relay …` on relay *selection*, relay dial failures only at debug; a failed lookup fails `connect` before any packet is sent, so the host sees nothing).
- `TicketBody.v = 2`; appended `relay: String` (home relay URL, `""` = unknown) and `addrs: Vec<String>` (direct socket addrs as text, best first, ≤ 8). Signed like the rest. v1 tickets are refused with `BadVersion(1)` and a "ask the host for a new link" message (pre-release; old links stop working). Typical string ~350–550 chars (≤ ~900 with 8 IPv6 addrs; v1 was ~230–330).
- Filled from the live endpoint on every signing (create, admin, ctl `CreateTicket`, `job_ticket` / ctl `JobTicket` re-sign). Consequence: a re-signed ticket string equals the original only while the host's addresses are unchanged (id/job/permissions always equal) — the desktop already treats `JobRecord.ticket` as "a valid link for this job", not as an identity.
- Clients (get/put/ctl) connect to `EndpointAddr{id, relay, addrs}` from the ticket; DNS/pkarr lookup stays active as fallback. Per-attempt connect timeout 30 s → 45 s (bounds lookup + handshake); retry policy unchanged.
- core API (additive unless noted): `ticket::HostAddr { relay, addrs }` (re-exported as `toss_core::HostAddr`) with `from_endpoint_addr`, `is_empty`, `summary`; `TicketBody::{set_host_addr, host_addr}`; `Ticket::endpoint_addr()`; `ticket::MAX_TICKET_ADDRS`; `Node::{host_addr, host_addr_changed, wait_online}`; `node::CONNECT_TIMEOUT`. **Changed signatures** (only the CLI uses them): `issue_ticket_offline(data_dir, cfg, spec, addr: &HostAddr)`, `issue_admin_ticket_offline(data_dir, days, addr: &HostAddr)`. `toss-core` gains a direct dependency on `n0-future` (already in the lock via iroh) for the lookup diagnostics.
- CLI: `cli-host.json` = host config (flattened, old files still load) + `addr` + `addr_updated`; `toss-cli host` waits ≤ 15 s for its relay, prints relay + direct addrs, and rewrites the file whenever they change. `toss-cli ticket` embeds them and prints `host relay: … direct addresses: N (…)` on stderr (warns when none are known). `toss-cli id` prints a `relay:` and `direct addresses:` line after the existing `<id>  <name>` line (from `cli-host.json`; it never starts an endpoint, which would publish a competing pkarr record for the same key). New global `--max-wait <secs>` for get/put (default 0 = unchanged behaviour): exits non-zero with the last error once the host has been unreachable/busy that long in a row (reset when a session reaches the manifest); `.part` files are kept.
- Desktop: `create_ticket` / `create_admin_ticket` wait ≤ 2 s for the relay when none is known yet, so fresh tickets carry it.
