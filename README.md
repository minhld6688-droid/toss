# Toss

Blip-style peer-to-peer file transfer for a photo studio. A **host** machine at the studio keeps the
files; remote **editors** receive a job folder by clicking a link or scanning a QR code, and send
finished work back by drag and drop. No server in the middle: a signed link carries the grant and
[iroh](https://iroh.computer) handles connection, NAT traversal and relay fallback.

- Tauri 2 desktop app (Mac + Windows), Rust core, React UI
- Link `https://<page>/#<ticket>` opens the app via `toss://t/<ticket>`
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

## Deploy the link page

Import the repo on Vercel with **Root Directory = `web`**, framework "Other", no build command.
After editing the inline script in `web/index.html`, run `node web/scripts/csp-hash.mjs` to refresh the
CSP hash in `web/vercel.json`. The deployed hostname must be what the desktop app uses as `page_host`.

## License

MIT
