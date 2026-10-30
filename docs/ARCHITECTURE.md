# Toss architecture (v0.8)

Toss moves job folders and files between machines. Every machine runs the same desktop app (since
v0.6 there is no "host" vs "editor" role): it can send to a contact or a nearby device, receive, and
make links. There is no Toss server: a link carries its grant in a signed **ticket**, contacts are
trusted by their iroh endpoint id, and iroh provides discovery, NAT traversal and relay fallback.
v0.8 adds a **LocalSend bridge** for LAN discovery and interop with every LocalSend app (iOS, Android,
Windows, macOS, Linux) and browser downloads on the LAN.

Normative specs: `contracts/wire.md` (ticket, URLs, protocols), `contracts/core-api.md` (Rust API),
`contracts/contacts-v0.5.md`, `contracts/nearby-v0.8.md`, `contracts/ipc.ts` (UI <-> shell).

## Components

| Part | Path | Role |
| --- | --- | --- |
| Core library | `crates/toss-core` | Tickets, wire protocols, node on iroh (links, contacts, pushes, nearby store, text), path sandboxing, job store |
| CLI | `crates/toss-cli` | Thin wrapper over the core (headless node, scripting, tests) |
| LocalSend bridge | `crates/toss-ls` | LocalSend v2 server/client + multicast discovery on the vendored, unmodified `third_party/localsend-core` (Apache-2.0); browser share |
| Desktop app | `apps/desktop` | Tauri 2 shell (Rust, `src-tauri`) + React UI (`src`); tray, deep links, autostart, drag and drop, right-click menu, notifications |
| Link page | `web/` | Static page + short-link API on Vercel; turns `https://<page>/#<ticket>` into `toss://t/<ticket>` |
| Distribution | `apps/desktop/src-tauri/bundle/`, `installer/`, `.github/workflows/` | NSIS hooks, macOS entitlements, CI and release |

## Two transports, one list

**Toss ↔ Toss stays on iroh** (QUIC, resumable, hash-verified, works across the Internet). The
LocalSend protocol is used only where iroh cannot help: devices that are not Toss, and Toss machines
on a LAN that blocks mDNS.

- *Toss nearby* (core): mDNS advertises `n=<name>;p=<platform>;v=<version>`; the core keeps a store of
  peers seen on the LAN (`Node::nearby`). A contact seen nearby is online immediately.
- *LocalSend* (toss-ls): the shell runs an `LsBridge` (HTTPS server on 53317, falling back to
  53318–53327, persisted self-signed cert under `<data_dir>/ls/`, announced as `device_model = "Toss"`).
- The shell merges both into **one nearby list** (`list_nearby`): core peers are `kind: "toss"`
  (`key = t:<endpoint hex>`); LocalSend devices are `kind: "localsend"` (`key = l:<fingerprint>`). A
  LocalSend device that is a Toss whose IP matches a core peer's address is the same machine and hidden;
  a Toss seen only through LocalSend is `kind: "toss_lan"` and is reached over the LocalSend path.
- `send_to(key, paths)`: `t:` / a contact id → `Node::send_to_peer` (iroh push); `l:` → `LsBridge::send`.

## Receiving and the receive policy

One setting, `receive_policy` (`off` | `contacts` (default) | `everyone`), persisted by the shell and
pushed to the core at start:

| | Toss contact with auto-accept | Nearby Toss stranger | LocalSend device in `ls_trusted` | Other LocalSend device |
| --- | --- | --- | --- | --- |
| `off` | ask | ask | ask | ask |
| `contacts` | accept | ask | accept | ask |
| `everyone` | accept | accept | accept | accept |

Every ask reaches the UI as one `incoming` event (`source: "toss" | "localsend"`) and is answered with
`answer_incoming(id, accept, always)`. `always` on a Toss stranger pairs both machines as contacts
(core `PushFrom` / `PushAcceptedPaired`); on a LocalSend device it adds the fingerprint to `ls_trusted`.
An ask shows and focuses the window and posts a notification. Unanswered asks expire (LocalSend 60 s,
Toss 10 min) → `incoming:closed`.

Files land in `<receive folder>/<sender name>/<job>/` for both transports (LocalSend paths are
sanitized component by component and never clobber existing files). When a receive finishes the shell
posts "Đã nhận <job> từ <name>" (click opens the folder), appends it to `<data_dir>/received.json`
(last 30) and emits `received`. Text messages (≤ 60 KiB) arrive as `text` events, nothing is saved.

Transfers list = core transfers ∪ LocalSend transfers (same `TransferState` shape, `contact: null`,
`path: "direct"`); `resend` repeats a finished send with the same items to the same target.

## Ticket (links)

`toss1` + lowercase base32 (RFC 4648, no padding) of `postcard(Ticket)`, where
`Ticket = { body: TicketBody, sig: ed25519(postcard(body)) }` signed by the issuing node's iroh secret
key (the same key that is its `EndpointId`). `TicketBody` carries: issuer key, job name, root alias,
path under the root (a folder, or since v0.8 a single file), return path, `get`/`put`/`admin` grants,
expiry, random job id. It is a **bearer token** until it expires or is revoked
(`<data_dir>/revoked.json`). The issuer re-verifies every ticket: signature, expiry, revocation, root
alias, and that the canonicalized path (symlinks resolved) stays inside the root. A quick share
("Tạo link") serves the folder — or a file's parent folder — as a `qs-<hash>` root.

Deep link `toss://t/<ticket>`; web link `https://<page-host>/<code>` (short) or `https://<page-host>/#<ticket>`.
The fragment never reaches the page host; the page redirects to the deep link and offers downloads
(`Toss.dmg`, `Toss-setup.exe` from GitHub Releases `latest/download`) when the app does not open.

## Transfer protocols

- `toss/xfer/1`, `toss/xfer/2` (links): one QUIC connection client → issuer; a control stream with
  length-prefixed postcard messages (`ClientHello`, `HostHello`, `Manifest`, `Want`, `Done`) and data on
  parallel unidirectional streams (xfer/2: segmented). Get compares against `dest/<job>/` (size+hash match
  = skip, `.part` = resume); put never overwrites a complete file with different content
  (`name (2).ext`). Resume needs no state: retry = new connection, `Want` rebuilds the plan.
- `toss/peer/1` (contacts, v0.5 + v0.8): pairing (`Pair`), presence (`Hello`), pushes (`Push` to contacts,
  `PushFrom` to nearby strangers — accepted only if the sender is in the nearby store), `Text`. After
  acceptance the data phase is xfer/2 put semantics on the same connection.
- `toss/ctl/1` (admin control link): `Status`, `ListDir`, `CreateTicket`, `ListJobs`, `Revoke`,
  `RequestUpdate` — lets an operator drive an unattended machine remotely.

## Networking, identity, OS integration

- Secret key at `<data_dir>/key` (0600); `data_dir` = `dirs::data_dir()/toss` (`TOSS_DATA_DIR` overrides).
- iroh endpoint with n0 discovery, default (or own) relays, mDNS, port mapping. Connections start through
  a relay and upgrade to direct UDP when hole punching succeeds. A machine with links or fixed folders
  binds UDP 41820 (falls back to random when taken).
- Windows firewall: rule "Toss" (UDP, any port) and "Toss LocalSend" (TCP + UDP 53317–53327) for
  `Toss.exe`, added by the installer when elevated, or by the in-app button (one UAC prompt).
- macOS: the first-run screen sends one multicast datagram (`request_local_network`) so the Local
  Network permission prompt appears before the first transfer; the core probes for a denial and the UI
  shows a banner.
- Autostart is on for every machine (forced on once by the 0.8 update, default for new installs); an
  autostart launch passes `--hidden` and stays in the tray/menu bar.
- Right-click "Gửi bằng Toss" on folders (`--share`) and files (`--send`), Windows *Send to → Toss*
  (`%APPDATA%\Microsoft\Windows\SendTo\Toss.lnk`), macOS Quick Action for `public.item`. A second launch
  forwards its arguments to the running app (tauri-plugin-single-instance), which opens "Gửi tới ai?".

## Build and release

- CI (`.github/workflows/ci.yml`): core/CLI/toss-ls tests, web API tests, UI build, workspace clippy;
  `desktop-check.yml`: desktop clippy + tests on macOS and Windows runners.
- Release (`.github/workflows/release.yml`) on tag `v*`: `tauri-action` builds macOS universal and Windows
  installers (+ signed updater artifacts) and creates the GitHub Release; copies are uploaded as
  `Toss.dmg` and `Toss-setup.exe`. macOS signing/notarization is optional (`APPLE_*` secrets).
- `web/` is deployed separately on Vercel. The page host is `AppState.page_host`.
