# Toss

Blip-style peer-to-peer file transfer for a photo studio. A **host** machine at the studio keeps the
files; remote **editors** receive a job folder by clicking a link or scanning a QR code, and send
finished work back by drag and drop. No server in the middle: a signed link carries the grant and
[iroh](https://iroh.computer) handles connection, NAT traversal and relay fallback.

- Tauri 2 desktop app (Mac + Windows), Rust core, React UI
- Short link `https://toss-link.vercel.app/<code>` (or the long `https://<page>/#<ticket>`) opens the app via `toss://t/<ticket>`
- Resumable, hash-verified transfers; host never overwrites finished files
- Unattended host mode: starts hidden at logon, survives Windows Update restarts

## Layout

| Path | What |
| --- | --- |
| `crates/toss-core`, `crates/toss-cli` | Core library and CLI |
| `apps/desktop` | Desktop app (Tauri 2 + React) |
| `web/` | Link page for Vercel (static) |
| `installer/` | NSIS hooks, macOS entitlements, studio host setup script |
| `.github/workflows/` | CI and release |
| `contracts/` | Wire format, core API, IPC (source of truth) |
| `docs/` | Install guide and architecture |

## Docs

- [Hướng dẫn cài đặt (Vietnamese)](docs/INSTALL.md): editors, studio host, troubleshooting
- [Architecture](docs/ARCHITECTURE.md)
- [Host setup (Vietnamese)](installer/host-setup/README.md)
- [Installer wiring and signing](installer/README.md)
- Specs: [wire](contracts/wire.md), [core API](contracts/core-api.md), [IPC](contracts/ipc.ts)

## Develop

```sh
cargo test -p toss-core                 # core tests
cd apps/desktop && npm install && npm run tauri dev
cd web/tests && npm i && PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers npm test   # link page e2e + screenshots
```

## Release

Push a tag `v*` (e.g. `git tag v0.1.0 && git push --tags`). The workflow publishes the GitHub Release
with stable asset names `Toss.dmg` and `Toss-setup.exe`, which the link page downloads from
`releases/latest/download/`.

## Deploy the link page (+ short links)

Import the repo on Vercel as project **`toss-link`** (→ `toss-link.vercel.app`, the app's default `page_host`),
**Root Directory = `web`**, framework "Other", no build command. Then *Storage → Create → Blob* and connect the
store to the project (sets `BLOB_READ_WRITE_TOKEN`), and redeploy.

- `/api/s` (web/api/s.js) turns a signed ticket/invite into a short link `https://toss-link.vercel.app/<9 chars>`
  and back. It verifies the ed25519 signature before storing (only real Toss links), refuses admin tickets, and
  answers 410 once a link expires. The app verifies the ticket again, and the host checks it, so the service
  cannot grant access by itself. If it is down, the app shares the long `#toss1…` link instead.
- After editing the inline script in `web/index.html`, run `node web/scripts/csp-hash.mjs` to refresh the
  CSP hash in `web/vercel.json`.
- Tests: `cd web && node --test tests/api.test.mjs` (API, real signed fixtures) and the Playwright e2e above.

## License

MIT
