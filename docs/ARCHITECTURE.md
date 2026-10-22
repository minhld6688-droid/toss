# Toss architecture

Toss moves job folders between one unattended **host** (studio machine, Windows) and remote
**editors** (Mac/Windows). There is no Toss server: everything a transfer needs is inside a signed
**ticket** string, and iroh provides discovery, NAT traversal and relay fallback.
Normative specs: `contracts/wire.md` (ticket, URLs, protocols), `contracts/core-api.md` (Rust API),
`contracts/ipc.ts` (UI <-> shell).

## Components

| Part | Path | Role |
| --- | --- | --- |
| Core library | `crates/toss-core` | Tickets, wire protocol, host/client node on iroh, path sandboxing, job store |
| CLI | `crates/toss-cli` | Thin wrapper over the core (headless host, scripting, tests) |
| Desktop app | `apps/desktop` | Tauri 2 shell (Rust) + React UI; deep link, tray, autostart, drag and drop |
| Link page | `web/` | Static page on Vercel; turns `https://<page>/#<ticket>` into `toss://t/<ticket>` |
| Distribution | `installer/`, `.github/workflows/` | NSIS/macOS hooks, host setup script, CI and release |

## Ticket

`toss1` + lowercase base32 (RFC 4648, no padding) of `postcard(Ticket)`, where
`Ticket = { body: TicketBody, sig: ed25519(postcard(body)) }` signed by the host's iroh secret key
(the same key that is the host's `EndpointId`). `TicketBody` carries: host key, job name, root alias,
path under the root, return path, `get`/`put`/`admin` grants, expiry, random job id. It is a **bearer
token**: whoever holds the string has the granted rights until it expires or the host revokes its id
(`<data_dir>/revoked.json`). The host re-verifies every ticket: signature, expiry, revocation, root
alias, and that the canonicalized path (symlinks resolved) stays inside the root.

## Links

- Deep link `toss://t/<ticket>` is registered by the desktop app on both OSes.
- Web link `https://<page-host>/#<ticket>`: the ticket is in the URL fragment, so the page host never
  receives it. The page redirects to the deep link and, if the app has not taken focus within
  1500 ms, shows "open again" and download buttons (assets `Toss.dmg`, `Toss-setup.exe` from
  GitHub Releases `latest/download`). The QR code encodes the web link.

## Transfer (`toss/xfer/1`)

One iroh connection client -> host. The first bidirectional stream is the control stream carrying
length-prefixed postcard messages (`ClientHello`, `HostHello`, `Manifest`, `Want`, `Done`).
Files are raw bytes on **one unidirectional stream per file** (header: index, offset), up to 4 in
parallel.

- **Get**: host walks `root/path`, hashes with blake3, sends `Manifest`; client compares against
  `dest/<job>/`, answers `Want` (size+hash match = skip, `.part` file = resume at its size); host streams
  the wanted files; client verifies blake3, renames `.part` -> final, retries a mismatch once.
- **Put**: mirror image into `return_root/<ret>/`. The host never overwrites a complete file with
  different content; it writes `name (2).ext`, `(3)`, ... and reports renames in `Done`.
- **Resume** needs no state: retry = new connection with the same ticket; `Want` rebuilds the plan.
- **Limits**: host serves at most `max_parallel_jobs` (default 2) connections; extras get
  `Denied{Busy}` and the client retries every 30 s for up to an hour.

## Admin control (`toss/ctl/1`)

A ticket with `admin = true`, created once on the host and pasted into a trusted device, authenticates
a request/response channel: `Status`, `ListDir`, `CreateTicket`, `ListJobs`, `Revoke`. This lets the
operator mint job links remotely. Job records persist in `<data_dir>/jobs.json`.

## Networking and identity

- Secret key at `<data_dir>/key` (mode 0600); `data_dir` = `dirs::data_dir()/toss`.
- iroh `Endpoint` with default n0 discovery and default relays. Connections start through a relay and
  upgrade to direct UDP when hole punching succeeds; progress events report `Direct` or `Relay`.
- No inbound port forwarding is required. The Windows installer adds a Defender Firewall rule for
  `Toss.exe` so direct paths work more often.

## Host deployment

The host runs the desktop app with host mode on, started hidden at logon by a Scheduled Task
(`installer/host-setup/windows-host.ps1`), with Windows auto-login so it survives restarts.

## Build and release

- CI (`.github/workflows/ci.yml`): `cargo test -p toss-core`, UI build, `cargo check --workspace`.
- Release (`.github/workflows/release.yml`) on tag `v*`: `tauri-action` builds macOS universal and Windows
  installers and creates the GitHub Release; a follow-up step uploads copies as `Toss.dmg` and
  `Toss-setup.exe`. macOS signing/notarization is optional and driven by `APPLE_*` secrets.
- `web/` is deployed separately on Vercel (static, no build). The page host is shown to the desktop app as
  `AppState.page_host`.
