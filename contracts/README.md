# Toss — contracts (read-only for feature agents)

Toss is a Blip-style P2P file transfer app for a small photo studio. One machine
("host", Windows, at the studio, unattended) holds the files. Remote editors
(Mac/Windows) receive a job folder by clicking a link / scanning a QR, and send
finished work back by drag & drop. **There is no server.** Everything a transfer
needs is inside a signed *ticket* string; discovery, NAT traversal and relay
fallback come from [iroh](https://docs.rs/iroh) (default n0 discovery + relays).

Three parts, three owners, three contracts:

| Part | Path | Codes against |
| --- | --- | --- |
| Core library + CLI (Rust) | `crates/toss-core`, `crates/toss-cli` | `wire.md`, `core-api.md` |
| Desktop app (Tauri 2 + React) | `apps/desktop` | `core-api.md` (Rust side), `ipc.ts` (UI side) |
| Link page, installers, CI | `web/`, `installer/`, `.github/workflows` | `wire.md` (ticket string + URL shape) |

Rules:
- Do not edit anything under `contracts/`. If a contract blocks you, write the
  proposed change to `contracts/CHANGES.md` (append-only) and code to the
  contract as written until it is approved.
- Stay inside your part's paths. Shared files (`Cargo.toml` at the root,
  `package.json` at the root) are owned by the coordinator; ask via CHANGES.md.
- Names in this folder are the names in the code: `Ticket`, `TicketSpec`,
  `Transfer`, `TransferEvent`, `toss://t/<ticket>` — no synonyms.
- Vietnamese is the UI language; code, identifiers and docs are English.

Glossary:
- **host** — a Toss node with host mode on: has *roots* (named top folders it
  may serve from) and a *return root* (where incoming files land). Signs tickets.
- **ticket** — signed, self-contained grant: which host, which folder, which
  directions (get and/or put), until when. Bearer: anyone holding it may use it.
- **admin ticket** — a ticket with `admin = true`, created on the host once and
  pasted into a trusted device; lets that device create job tickets remotely.
- **job** — one ticket's worth of work (e.g. "12A3 - RAW"). Has a `job` name
  shown in UIs and used as the destination folder name on the receiving side.
- **get** — host → client (editor downloads the job folder).
- **put** — client → host (editor uploads into the job's return folder).
