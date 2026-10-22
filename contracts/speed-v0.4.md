# v0.4 contract — speed: NAT traversal first, then the transfer pipeline

Why: on 22/10 a 57 MB job between a Mac and the Windows host **on the same home
network** ran at 998 KB/s with the card showing *Relay*: traffic went through
n0's public relay (rate-limited per connection, far away). Direct paths ran at
25–33 MB/s in the Azure e2e. v0.4 makes Direct the normal case and makes every
relay fallback explainable, then widens the pipe.

Priority: §2 NAT traversal > §3 diagnostics > §5 own relay > §4 pipeline.
Types are in `crates/toss-core/src/net.rs` (coordinator-owned, do not change
shapes; propose changes in `CHANGES.md`). Everything stays backward compatible
with v0.3 peers on the wire (§4.3).

## 1. Workstreams and file ownership

| WS | Owns (may edit) | Must not edit |
| --- | --- | --- |
| A — NAT traversal | `crates/toss-core/src/{node.rs,ticket.rs,netdiag.rs}`, new files `crates/toss-core/src/mdns.rs` etc., `crates/toss-core/Cargo.toml` (deps only), `crates/toss-cli/src/main.rs` (`diag`, `--relay`, `--no-mdns`, `--no-portmap` only), new tests `crates/toss-core/tests/nat.rs` | xfer/host/client/wire/transfer/tuning |
| B — Diagnostics UI + shell | `apps/desktop/**` | `crates/**` |
| C — Relay kit, installer, CI | `installer/**`, `hub/relay/**` (new), `.github/workflows/**`, `docs/INSTALL.md`, `web/**` (only if needed) | `crates/**`, `apps/desktop/src/**` |
| D — Pipeline | `crates/toss-core/src/{xfer.rs,host.rs,client.rs,wire.rs,transfer.rs,fsutil.rs,tuning.rs}`, `crates/toss-cli/src/main.rs` (`bench` only), new tests `crates/toss-core/tests/pipeline.rs` | node/ticket/netdiag |

Shared files (`lib.rs`, root `Cargo.toml`, `Cargo.lock`): add lines only
(new `mod`, new dependency); never reorder or rewrite. The coordinator merges.
Version bump to 0.4.0 is done by the coordinator at release.

## 2. NAT traversal (WS-A) — behaviour

Layers, tried together by iroh, listed by what Toss must add:

0. **Same LAN.** mDNS address lookup on every node when `NetConfig::mdns`
   (crate `iroh-mdns-address-lookup` or equivalent compatible with iroh 1.4;
   if none compiles against iroh 1.4, implement a minimal lookup over
   `swarm-discovery`/`mdns-sd` behind the same `AddressLookup` trait). Tickets:
   `HostAddr::from_endpoint_addr` orders addresses **mapped/public v4, global
   v6, private LAN v4** and always keeps at least 2 private LAN addresses within
   `MAX_TICKET_ADDRS` (raise the cap to 8 if needed; ticket stays ≤ ~900 chars).
1. **Host opens a port.** Host mode binds `DEFAULT_HOST_PORT` (41820) unless
   the user set another `bind_port` (desktop default for *new* host configs;
   existing configs with `bind_port: null` get 41820 on next start; if the port
   is taken, fall back to random and add a reason). Port mapping via iroh's
   built-in portmapper (`Endpoint::builder().portmapper_config`) when
   `NetConfig::portmap`; the mapped public address must appear in
   `Endpoint::addr()` and thus in new tickets.
2. **Hole punching.** iroh does it. Toss adds: if a client transfer's
   connection is still `Relay` 10 s after connecting, re-add the ticket
   addresses + any mDNS/lookup addresses (`MemoryLookup::add_endpoint_info`)
   once and log `still on relay after 10s: <known paths>`; never interrupt the
   running transfer.
3. **Own relay.** `NetConfig::relay_urls` non-empty → `RelayMode::Custom`
   with those URLs first and n0's default relays after them (so a dead own
   relay never breaks Toss).

Diagnostics (`Node::diagnostics` → `NetDiag`, `Node::test_connect` →
`ConnectReport`, implemented in `netdiag.rs`):
- `relay_connected` from `home_relay_status`; `nat` from `Endpoint::net_report()`
  (udp, global v4/v6, `mapping_varies_by_dest`, hairpin, lowest relay latency);
  `portmap` from the portmapper result (status off/probing/mapped/unavailable).
- `direct_likelihood`: High if a mapped public addr or a global v6 exists;
  Medium if UDP works and mapping does not vary by destination; Low if
  symmetric NAT or no UDP; Unknown while the report is pending.
- `reasons`: short Vietnamese sentences, e.g. "Router không hỗ trợ UPnP — mở
  cổng UDP 41820 thủ công để editor ở xa nối trực tiếp",
  "NAT đối xứng: editor ở xa sẽ đi qua relay", "Chưa nối được relay".
- `test_connect`: dial ALPN xfer, keep the connection up to 8 s or until a
  direct path is selected; report path, RTT (`Connection` path stats), connect
  time, time-to-direct, known paths and a reason when not direct. Must not
  start a job; the host closes such a connection after the hello timeout
  without logging an error (a client sends nothing).
- CLI: `toss-cli diag` prints `NetDiag` as JSON; `toss-cli diag --ticket T`
  also runs `test_connect`.

Logging: the library keeps logging through `tracing` (target `toss_core`).

## 3. Diagnostics UI + shell (WS-B)

IPC (exact names, see `contracts/ipc.ts` v0.4 block):
- `get_diagnostics(): NetDiagUi` = core `NetDiag` + `firewall` from the shell:
  Windows `Get-NetFirewallRule`/`netsh` check for an inbound UDP allow rule for
  the current `Toss.exe` → `"ok" | "missing" | "unknown"`; macOS
  `socketfilterfw --getglobalstate` + `--getappblocked` → same values.
- `add_firewall_rule(): void` — Windows only: elevated (UAC, one prompt)
  `New-NetFirewallRule -DisplayName "Toss" -Direction Inbound -Action Allow
  -Protocol UDP -Program <exe> -Profile Any` (replace an existing "Toss" rule);
  macOS: `socketfilterfw --add/--unblockapp` via an admin prompt
  (`osascript … with administrator privileges`). Errors are returned as text.
- `test_connect(args: { ticket }): ConnectReport`.
- `set_net(args: { net: NetConfig }): void` — persisted in `settings.json`
  (`net` key), applied by restarting the node when no transfer is active
  (same deferred-restart rule as device name).
- `export_logs(): string` — zips `<data_dir>/logs/` + `settings.json` (no
  `key`) to `~/Desktop/toss-logs-<date>.zip` (Windows: Desktop too), returns
  the path.
- Log file: `tracing_appender` rolling file `<data_dir>/logs/toss.log`
  (daily or 5 MB, keep 3) in addition to stderr; default filter
  `info,iroh=warn,toss_core=info`.
- UI (keep the 0.3.1 design language: Fraunces + Bricolage, own icon set in
  `components/icons.tsx`, aura, motion with reduced-motion support):
  - Settings → new section **Kết nối** (Chẩn đoán): likelihood badge
    (Cao/Trung bình/Thấp/Đang đo), relay + connected dot, bind port, port
    mapping status + external addr, NAT facts, firewall row with button
    "Cho phép qua firewall" when missing, reasons list, button "Thử nối tới
    máy tổng" (uses the admin ticket's host or a pasted link) showing path +
    RTT + reason, button "Xuất log". Advanced: own relay URL field, toggles
    mDNS / port mapping, streams (8/16), congestion (BBR/Cubic).
  - Transfer card: next to the Trực tiếp/Relay chip show `rtt_ms` ("12 ms")
    and, when relay, a small "Vì sao?" link opening the diagnostics section.
  - First run of host mode: if firewall is `missing`, show one step "Cho phép
    Toss qua firewall" before finishing.
- Mock IPC (`VITE_MOCK=1`) implements all new commands; screenshots updated.

## 4. Pipeline (WS-D)

### 4.1 Transport (`tuning.rs::transport_config`)
From `Tuning`: `stream_receive_window = stream_window_mb`, `send_window =
send_window_mb`, connection `receive_window` ≥ streams × stream window,
`max_concurrent_uni_streams` ≥ 2 × `streams_high_rtt`, congestion controller
BBR or Cubic, `initial_rtt` 50 ms, keep iroh's keep-alive/idle defaults. If
noq lacks a knob, skip it and say so in the PR.

### 4.2 Sender/receiver
- Streams in flight per transfer = `tuning.streams`, raised to
  `streams_high_rtt` while the path is relay or RTT > 30 ms (checked every 2 s).
  Host `max_parallel_streams` keeps working as an upper cap if lower.
- Chunk size `chunk_kb`; per-stream stall timeout instead of per-chunk `timed()`
  wrapping where that saves work, but the 120 s stall rule stays.
- Large files (> `range_split_mb`) are sent as several *segments* on parallel
  streams; small files (< `small_file_kb`) are batched several per stream
  (batch ≤ 16 MiB). Receiver writes segments at their offsets into `.part`,
  finalizes a file when all its bytes arrived, verifies blake3 of the whole
  file (incremental when a single sequential segment, else re-read from disk),
  renames. Resume still uses `.part` size, i.e. on resume only a prefix is
  trusted: a receiver that had out-of-order segments in flight truncates the
  `.part` to the longest contiguous prefix on disconnect (keep a per-file
  contiguous watermark).
- Receiver buffering: write in `chunk_kb` blocks, `sync_data` once per file.
- Host prehash: when a get ticket is created (host-local or ctl) start hashing
  its source folder in the background (`HashCache`), up to 4 files in
  parallel; manifest building also hashes up to 4 files in parallel. Goal:
  first byte < 2 s for an already-prehashed 80 GB job.
- Progress: fill `rtt_ms` (selected path RTT) and `streams` (open data
  streams) in `TransferState` / `Progress`.

### 4.3 Wire `toss/xfer/2` (compatible)
- New ALPN `toss/xfer/2` (`ALPN_XFER2`). Hosts accept both `xfer/1` and
  `xfer/2`. Clients connect with `xfer/2` and
  `ConnectOptions::with_additional_alpns([xfer/1])`, then branch on the
  negotiated ALPN. xfer/1 behaviour is unchanged (old peers keep working).
- xfer/2 data streams carry a sequence of segments:
  `u32 LE index, u64 LE offset, u64 LE len, <len bytes>` repeated until the
  stream finishes. Control messages (Hello, Manifest, Want, Done) unchanged.
- `ClientHello`/`HostHello` unchanged; the ALPN is the version.
- Update `wire.md` is the coordinator's job; describe the final format in the PR.

### 4.4 Bench
`toss-cli bench [--size-mb 2048] [--files N] [--rtt-ms R]` starts two nodes in
one process (loopback), runs a get, prints JSON
`{path, rtt_ms, mb_s, cpu_pct, streams, files, bytes, elapsed_s}`. `--rtt-ms`
may be ignored where delay injection is impossible (say so in the output).
Report before/after numbers in the PR.

## 5. Own relay kit (WS-C)

- `hub/relay/`: deploy kit for `iroh-relay` (version matching iroh 1.4) on a
  small Ubuntu VPS: `install.sh` (download/build binary, systemd unit,
  Let's Encrypt via the relay's built-in ACME or certbot, UFW rules 80/443
  TCP + 3478/7842 UDP as required by the relay version), `relay.toml`
  template with access control limited to allowed endpoint ids
  (file `allowed.txt`) or open, metrics on localhost, `README.md` in
  Vietnamese with the 6 steps for the owner. No VPS is bought here.
- Installer: NSIS `installMode: "both"` so `hooks.nsh` can add the firewall
  rule when elevated; the rule must be UDP inbound for `Toss.exe`; per-user
  installs keep working (shell `add_firewall_rule` covers them).
  `installer/host-setup/windows-host.ps1`: UDP rule, check port 41820 bound by
  Toss, print the portmap/likelihood line from `toss-cli diag` if available.
- CI: `e2e.yml` asserts the get ran on `Direct` (from `toss-cli get` output),
  prints MB/s, and adds a job variant where direct UDP is blocked on the
  client (firewall rule) and the transfer must still finish via relay.
  Keep the workflow green-able without secrets.
- `docs/INSTALL.md`: section "Tốc độ & kết nối" (firewall, UPnP/port forward
  41820/UDP, own relay, reading the diagnostics panel).

## 6. Acceptance (coordinator runs these)

- `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`,
  `npm run build` in `apps/desktop` all green.
- New tests: ticket address ordering keeps LAN addrs; diagnostics JSON shape;
  xfer/2 get+put incl. a file split into ≥ 3 segments, a batch of ≥ 50 small
  files, resume after a mid-transfer disconnect, and xfer/1 client ↔ xfer/2
  host and xfer/2 client ↔ xfer/1-only host.
- `toss-cli bench` loopback ≥ 2× v0.3 throughput or a written reason.
- Real machines (coordinator + owner): Mac ↔ Windows on one LAN → Direct within
  3 s.
