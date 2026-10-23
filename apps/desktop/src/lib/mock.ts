// In-memory mock of the shell (VITE_MOCK=1). Simulates tickets, host mode, remote
// admin control and transfers with progress so the UI can be built and screenshotted
// without Rust. Implements the same surface as Commands (+ CommandsV05 + ExtraCommands).
// URL switches: ?host=1 ?remote=1 ?wizard=1 ?net=relay|medium|direct|probing ?share=1 ?deeplink=1|exists
//   v0.5: ?push=1 (Anh Hùng sends an incoming push 1 s after start) ?invite=1 (toss://i/… deep link arrives)
//         ?lan=blocked (macOS blocks the local network) ?contacts=0 (empty book) ?restored=1 (paused + queued pushes after a "restart")
//   v0.5.1: ?update=1 (0.5.2 available, announced 0.4 s after start) &force=1 (marked #force) ?remoteold=1 (remote host on 0.5.0)
import type {
  AppState, Commands, CommandsV05, CommandsV051, ConnectReport, Contact, IncomingPush, InviteInfo, DirEntry, FirewallState, HostConfig, HostStatus, JobRecord,
  NetConfig, NetDiagUi, TicketInfo, TicketSpec, TransferEvent, TransferState, UpdateInfo,
} from "./ipc-types";
import type { ExtraCommands } from "./ipc-extra";

const MB = 1024 * 1024;
const GB = 1024 * MB;
const now = () => Math.floor(Date.now() / 1000);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

// ---- events --------------------------------------------------------------
const listeners = new Map<string, Set<(p: unknown) => void>>();
export function mockOn(name: string, h: (p: unknown) => void): () => void {
  if (!listeners.has(name)) listeners.set(name, new Set());
  listeners.get(name)!.add(h);
  return () => listeners.get(name)?.delete(h);
}
function emit(name: string, payload: unknown) {
  listeners.get(name)?.forEach((h) => h(payload));
}
function emitTransfer(ev: TransferEvent) {
  emit("transfer:event", ev);
}

// ---- ticket codec (fake, but same look: "toss1" + lowercase base32) --------
const ALPHA = "abcdefghijklmnopqrstuvwxyz234567";
function b32enc(bytes: Uint8Array): string {
  let bits = 0, val = 0, out = "";
  for (const b of bytes) {
    val = (val << 8) | b; bits += 8;
    while (bits >= 5) { out += ALPHA[(val >>> (bits - 5)) & 31]; bits -= 5; }
  }
  if (bits > 0) out += ALPHA[(val << (5 - bits)) & 31];
  return out;
}
function b32dec(s: string): Uint8Array {
  let bits = 0, val = 0; const out: number[] = [];
  for (const ch of s.toLowerCase()) {
    const i = ALPHA.indexOf(ch); if (i < 0) continue;
    val = (val << 5) | i; bits += 5;
    if (bits >= 8) { out.push((val >>> (bits - 8)) & 255); bits -= 8; }
  }
  return new Uint8Array(out);
}
const HOST_ID = "q7x2k5m4v6b3n8c2w5e7r4t6y3u5i2o4p7a3s5d6f2g4h7j3k5l2";
function hex(n: number) { return Array.from({ length: n }, () => Math.floor(Math.random() * 16).toString(16)).join(""); }

function fakeCode(s: string): string {
  const A = "abcdefghijklmnopqrstuvwxyz234567";
  let h = 2166136261, out = "";
  for (let i = 0; i < 9; i++) {
    for (let k = 0; k < s.length; k++) h = Math.imul(h ^ s.charCodeAt(k) ^ i, 16777619) >>> 0;
    out += A[h % 32];
  }
  return out;
}

function makeInfo(body: Omit<TicketInfo, "ticket" | "web_url" | "deep_link">): TicketInfo {
  const json = JSON.stringify(body);
  const ticket = "toss1" + b32enc(new TextEncoder().encode(json)) + b32enc(crypto.getRandomValues(new Uint8Array(24)));
  return withUrls({ ...body, ticket });
}
function withUrls(t: Omit<TicketInfo, "web_url" | "deep_link">): TicketInfo {
  // v0.5.2: links are short (https://<page_host>/<9 chars>); the mock derives a stable fake code.
  return { ...t, web_url: `https://${state.page_host}/${fakeCode(t.ticket)}`, deep_link: `toss://t/${t.ticket}` };
}
function decodeTicket(text: string): TicketInfo {
  const m = text.match(/toss1[a-z2-7]{16,}/i);
  if (!m) throw new Error("Không phải link Toss hợp lệ");
  const raw = m[0].toLowerCase();
  const known = issued.get(raw);
  if (known) return known;
  try {
    const bytes = b32dec(raw.slice(5));
    const text8 = new TextDecoder().decode(bytes);
    const end = text8.lastIndexOf("}");
    if (end > 0) {
      const body = JSON.parse(text8.slice(0, end + 1));
      if (body && typeof body.job === "string") return withUrls({ ...body, ticket: raw });
    }
  } catch { /* fall through to demo ticket */ }
  return withUrls({
    ticket: raw, id: hex(32), host: HOST_ID, job: "Demo - 12A3", root: "RAW", path: "2026/12A3", ret: "12A3",
    get: true, put: true, admin: false, exp: now() + 6 * 86400,
  });
}
const issued = new Map<string, TicketInfo>();

// ---- state ---------------------------------------------------------------
const params = typeof location !== "undefined" ? new URLSearchParams(location.search) : new URLSearchParams();
const hostOn = params.get("host") === "1";
const wizard = params.get("wizard") === "1";

const demoHost: HostConfig = {
  roots: [
    { alias: "RAW", path: "D:\\Studio\\RAW" },
    { alias: "Retouch", path: "D:\\Studio\\Retouch" },
  ],
  return_root: "D:\\Studio\\Tra-ve",
  max_parallel_jobs: 2,
  bind_port: 41820,
};

const defaultNet = (): NetConfig => ({
  relay_urls: [], mdns: true, portmap: true,
  tuning: { stream_window_mb: 8, send_window_mb: 64, congestion: "bbr", streams: 8, streams_high_rtt: 16, chunk_kb: 1024, range_split_mb: 512, small_file_kb: 1024 },
});

const state: AppState = {
  version: "0.5.1",
  endpoint_id: HOST_ID,
  device_name: "MacBook của Minh",
  dest_dir: "/Users/minh/Toss",
  host: hostOn ? structuredClone(demoHost) : null,
  page_host: "toss-link.vercel.app",
  autostart: true,
  first_run: wizard,
  context_menu: true,
  auto_receive: params.get("auto") !== "0",
  net: defaultNet(),
  auto_update: hostOn,
};
let adminTicket: string | null = params.get("remote") === "1" ? "toss1mockadmin" + "abcdefghijklmnopqrstuvwxyz234567" : null;

const transfers = new Map<string, TransferState>();
const timers = new Map<string, number>();
const bytesPerTick = new Map<string, number>();
const startedAt = Date.now();

function tfBase(p: Partial<TransferState> & { id: string; job: string }): TransferState {
  return {
    op: "get", side: "client", peer: "Máy tổng", status: "connecting", bytes_done: 0, bytes_total: 0,
    files_done: 0, files_total: 0, speed_bps: 0, eta_s: null, path: "unknown", rtt_ms: null, streams: 0, dest: null, error: null,
    started_at: now(), contact: null, ...p,
  };
}

function seed() {
  const dest = `${state.dest_dir}/11B2 - RAW`;
  transfers.set("a1-0001", tfBase({
    id: "a1-0001", job: "11B2 - RAW", status: "done", bytes_done: 4.2 * GB, bytes_total: 4.2 * GB,
    files_done: 318, files_total: 318, path: "direct", rtt_ms: 6, dest, started_at: now() - 3600,
  }));
  transfers.set("a2-0002", tfBase({
    id: "a2-0002", job: "12A3 - Retouch (trả)", op: "put", status: "done", bytes_done: 1.1 * GB, bytes_total: 1.1 * GB,
    files_done: 96, files_total: 96, path: "relay", rtt_ms: 164, started_at: now() - 7200,
  }));
  transfers.set("a3-0003", tfBase({
    id: "a3-0003", job: "12A3 - RAW", status: "running", bytes_done: 1.7 * GB, bytes_total: 5.6 * GB,
    files_done: 121, files_total: 402, speed_bps: 38 * MB, eta_s: 103, path: "direct", rtt_ms: 12, streams: 8,
    dest: `${state.dest_dir}/12A3 - RAW`, started_at: now() - 60,
  }));
  runLoop("a3-0003", 6 * MB);
  if (params.get("busy") === "1") {
    transfers.set("a4-0004", tfBase({ id: "a4-0004", job: "10C1 - RAW", status: "connecting", error: "Máy tổng đang bận, thử lại sau 30 giây" }));
  }
}

function runLoop(id: string, perTick: number) {
  window.clearInterval(timers.get(id));
  bytesPerTick.set(id, perTick);
  const t = window.setInterval(() => {
    const s = transfers.get(id);
    if (!s || s.status !== "running") return;
    const jitter = 0.7 + Math.random() * 0.6;
    const inc = Math.min(s.bytes_total - s.bytes_done, (bytesPerTick.get(id) ?? 6 * MB) * jitter * 4 + Math.random() * MB * 10);
    s.bytes_done += inc;
    s.speed_bps = Math.round(inc * 2);
    s.files_done = Math.min(s.files_total, Math.floor((s.bytes_done / s.bytes_total) * s.files_total));
    s.eta_s = s.speed_bps ? Math.round((s.bytes_total - s.bytes_done) / s.speed_bps) : null;
    if (s.bytes_done >= s.bytes_total) {
      s.status = "done"; s.files_done = s.files_total; s.speed_bps = 0; s.eta_s = null;
      window.clearInterval(t); timers.delete(id);
      emitTransfer({ type: "progress", id, bytes_done: s.bytes_total, bytes_total: s.bytes_total, files_done: s.files_total, files_total: s.files_total, speed_bps: 0, eta_s: 0, path: s.path, rtt_ms: s.rtt_ms, streams: 0 });
      emitTransfer({ type: "done", id, files: s.files_total, bytes: s.bytes_total, elapsed_s: Math.max(1, now() - s.started_at), dest: s.dest });
      return;
    }
    emitTransfer({ type: "progress", id, bytes_done: Math.round(s.bytes_done), bytes_total: s.bytes_total, files_done: s.files_done, files_total: s.files_total, speed_bps: s.speed_bps, eta_s: s.eta_s, path: s.path, rtt_ms: s.rtt_ms, streams: s.streams });
  }, 500);
  timers.set(id, t);
}

async function launch(info: TicketInfo, op: "get" | "put", paths?: string[]): Promise<string> {
  const id = `${info.id.slice(0, 6)}-${hex(4)}`;
  const putName = paths && paths.length ? paths[0].split(/[\\/]/).filter(Boolean).pop() : null;
  const files = op === "get" ? 180 + Math.floor(Math.random() * 300) : 40 + Math.floor(Math.random() * 80);
  const total = op === "get" ? (1.5 + Math.random() * 4) * GB : (0.4 + Math.random() * 1.2) * GB;
  const s = tfBase({ id, job: putName && op === "put" ? `${info.job} · ${putName}` : info.job, op, peer: "Máy tổng · Studio" });
  transfers.set(id, s);
  emitTransfer({ type: "started", id, job: s.job, op, side: "client", peer: s.peer });
  void (async () => {
    await sleep(900);
    if (s.status !== "connecting") return;
    s.bytes_total = Math.round(total); s.files_total = files;
    s.status = "running"; s.path = Math.random() < 0.7 ? "direct" : "relay";
    s.rtt_ms = s.path === "direct" ? 4 + Math.floor(Math.random() * 12) : 120 + Math.floor(Math.random() * 100);
    s.streams = s.path === "direct" ? 8 : 16;
    if (op === "get") s.dest = `${state.dest_dir}/${info.job}`;
    emitTransfer({ type: "manifest", id, files, bytes: s.bytes_total, to_do_files: files, to_do_bytes: s.bytes_total });
    runLoop(id, s.path === "direct" ? 9 * MB : 3 * MB);
  })();
  return id;
}

// ---- fake file tree ------------------------------------------------------
const TREE: Record<string, string[]> = {
  "": ["2026", "2025", "Lưu trữ"],
  "2026": ["12A3", "12A4", "11B2", "10C1"],
  "2025": ["9A1", "9A2"],
  "2026/12A3": ["RAW", "Retouch", "Chọn"],
  "2026/12A4": ["RAW"],
  "2026/11B2": ["RAW", "Retouch"],
  "2026/10C1": ["RAW"],
};
function listFake(path: string): DirEntry[] {
  const kids = TREE[path.replace(/^\/|\/$/g, "")] ?? [];
  const dirs: DirEntry[] = kids.map((n) => ({ name: n, is_dir: true, size: 0, mtime: now() - Math.floor(Math.random() * 30 * 86400) }));
  if (dirs.length === 0 || path.split("/").length >= 3) {
    for (let i = 1; i <= 4; i++) dirs.push({ name: `IMG_${String(4000 + i * 37)}.CR3`, is_dir: false, size: 28 * MB + i * 1.3 * MB, mtime: now() - i * 3600 });
  }
  return dirs;
}

// ---- jobs ----------------------------------------------------------------
const jobs: JobRecord[] = [];
function seedJobs() {
  const mk = (job: string, root: string, path: string, get: boolean, put: boolean, daysLeft: number, last: JobRecord["last"], revoked = false) => {
    const info = makeInfo({ id: hex(32), host: HOST_ID, job, root, path, ret: job, get, put, admin: false, exp: now() + daysLeft * 86400 });
    issued.set(info.ticket, info);
    jobs.push({ id: info.id, job, root, path, get, put, exp: info.exp, created: now() - 86400 * 2, revoked, last, ticket: info.ticket, web_url: info.web_url });
  };
  mk("12A3 - RAW", "RAW", "2026/12A3/RAW", true, false, 5, { at: now() - 600, device: "Anh Hùng - iMac", op: "get", files: 402, bytes: 5.6 * GB, ok: true, msg: "" });
  mk("12A3 - Retouch", "Retouch", "2026/12A3", false, true, 5, { at: now() - 7200, device: "Chị Lan - Win", op: "put", files: 96, bytes: 1.1 * GB, ok: true, msg: "" });
  mk("10C1 - RAW", "RAW", "2026/10C1/RAW", true, true, 1, null);
  mk("9A1 - RAW", "RAW", "2025/9A1", true, false, -2, { at: now() - 86400 * 3, device: "Anh Hùng - iMac", op: "get", files: 12, bytes: 300 * MB, ok: false, msg: "host_offline" }, true);
}

function validateSpec(spec: TicketSpec) {
  if (!spec.job.trim()) throw new Error("Thiếu tên job");
  if (!spec.get && !spec.put) throw new Error("Chọn ít nhất một quyền (nhận hoặc trả)");
}

function doCreate(spec: TicketSpec, remote: boolean): TicketInfo {
  validateSpec(spec);
  const info = makeInfo({
    id: hex(32), host: HOST_ID, job: spec.job, root: spec.root, path: spec.path, ret: spec.ret || spec.job,
    get: spec.get, put: spec.put, admin: false, exp: spec.days ? now() + spec.days * 86400 : 0,
  });
  issued.set(info.ticket, info);
  jobs.unshift({ id: info.id, job: info.job, root: info.root, path: info.path, get: info.get, put: info.put, exp: info.exp, created: now(), revoked: false, last: null, ticket: remote ? "" : info.ticket, web_url: remote ? "" : info.web_url });
  return info;
}

function hostStatus(): HostStatus {
  const cfg = state.host ?? demoHost;
  const active = [...transfers.values()].filter((t) => t.status === "running").length;
  return { device: remoteDevice(), version: adminTicket ? remoteVersion : state.version, roots: cfg.roots.map((r) => r.alias), active, queued: 0, uptime_s: Math.floor((Date.now() - startedAt) / 1000) + 5400 };
}
const remoteDevice = () => (adminTicket ? "Máy tổng - Studio" : state.device_name);

// ---- v0.5.1 updates -----------------------------------------------------------
const updateOn = params.get("update") === "1";
const NEW_RELEASE: UpdateInfo = {
  available: true, current: "0.5.1", version: "0.5.2", forced: params.get("force") === "1", date: "2026-10-23T08:00:00Z",
  notes: "• Gửi file cho máy trong danh bạ ổn định hơn khi mạng LAN chập chờn\n• Máy tổng tự cập nhật theo lệnh từ máy quản lý\n• Sửa lỗi hạn link hiển thị lệch giờ" + (params.get("force") === "1" ? "\n#force" : ""),
};
let remoteVersion = params.get("remoteold") === "1" ? "0.5.0" : "0.5.1";
let installing = false;

let pickN = 0;
let uiReadyDone = false;

// ---- v0.3 quick share ------------------------------------------------------
const baseOf = (p: string) => p.split(/[\\/]+/).filter(Boolean).pop() ?? p;
const quickShares: { alias: string; path: string; job_id: string | null }[] = [];
function seedShares() {
  for (const [path, put] of [["/Users/minh/Pictures/Giao hàng/12A3 - RAW", false], ["D:\\Studio\\Chon\\11B2", true]] as const) {
    const info = makeInfo({ id: hex(32), host: HOST_ID, job: baseOf(path), root: "qs-x", path: "", ret: baseOf(path), get: true, put, admin: false, exp: now() + 5 * 86400 });
    issued.set(info.ticket, info);
    quickShares.push({ alias: `qs-${hex(8)}`, path, job_id: info.id });
  }
}
// jobs already refused once by start_get ("đã nhận xong"); the second try (sheet "Tải lại") goes through
const reloadOk = new Set<string>();

// ---- v0.5 contacts / invites / pushes --------------------------------------
const lanBlocked = params.get("lan") === "blocked";
const contacts: Contact[] = params.get("contacts") === "0" ? [] : [
  { id: hex(64), name: "Từ Đạt – Mac", their_name: "MacBook của Đạt", platform: "macos", added_at: now() - 9 * 86400, auto_accept: true, online: true, last_seen: now(), rtt_ms: 12 },
  { id: hex(64), name: "Chị Lan – PC", their_name: "DESKTOP-LAN", platform: "windows", added_at: now() - 21 * 86400, auto_accept: true, online: false, last_seen: now() - 2 * 3600 - 600, rtt_ms: null },
  { id: hex(64), name: "Anh Hùng – iMac", their_name: "iMac của Hùng", platform: "macos", added_at: now() - 3 * 86400, auto_accept: false, online: true, last_seen: now(), rtt_ms: 31 },
];
const pushes: IncomingPush[] = [];
const hung = () => contacts.find((c) => c.name.startsWith("Anh Hùng"));
const dat = () => contacts.find((c) => c.name.startsWith("Từ Đạt"));
const lan = () => contacts.find((c) => c.name.startsWith("Chị Lan"));

function makeInvite(name: string, days: number): InviteInfo {
  const body = b32enc(new TextEncoder().encode(JSON.stringify({ name })));
  const invite = "tossinv1" + body + "ab" + b32enc(crypto.getRandomValues(new Uint8Array(20)));
  return { invite, web_url: `https://${state.page_host}/#${invite}`, deep_link: `toss://i/${invite}`, exp: now() + days * 86400, name: state.device_name };
}
function decodeInvite(text: string): InviteInfo {
  const m = text.match(/tossinv1[a-z2-7]{16,}/i);
  if (!m) throw new Error("Không phải lời mời Toss hợp lệ");
  const invite = m[0].toLowerCase();
  let name = "Mai – Laptop";
  try {
    const raw = new TextDecoder().decode(b32dec(invite.slice(8)));
    const j = JSON.parse(raw.slice(0, raw.lastIndexOf("}") + 1));
    if (j && typeof j.name === "string") name = j.name;
  } catch { /* demo name */ }
  return { invite, web_url: `https://${state.page_host}/#${invite}`, deep_link: `toss://i/${invite}`, exp: now() + 6 * 86400, name };
}
let issuedInvites = 0;

function seedRestored() {
  // what a restart leaves behind: a half-sent push (paused), one queued for an offline contact, one just received
  const d = dat(), l = lan(), h = hung();
  if (d) transfers.set("r1-0001", tfBase({
    id: "r1-0001", job: "11B2 - Retouch", op: "put", peer: d.name, contact: d.id, status: "paused",
    bytes_done: 0.6 * GB, bytes_total: 1.6 * GB, files_done: 41, files_total: 108, path: "direct", rtt_ms: 12, started_at: now() - 900,
  }));
  if (l) transfers.set("r2-0002", tfBase({
    id: "r2-0002", job: "12A3 - Chọn", op: "put", peer: l.name, contact: l.id, status: "connecting", started_at: now() - 300,
  }));
  if (h) transfers.set("r3-0003", tfBase({
    id: "r3-0003", job: "10C1 - Ảnh lớp", op: "put", side: "host", peer: h.name, contact: h.id, status: "done",
    bytes_done: 2.3 * GB, bytes_total: 2.3 * GB, files_done: 188, files_total: 188, path: "direct", rtt_ms: 31,
    dest: `${state.dest_dir}/${h.name}/10C1 - Ảnh lớp`, started_at: now() - 5000,
  }));
}

// ---- v0.4 diagnostics ------------------------------------------------------
// ?net=relay (default: symmetric NAT, no port mapping, firewall missing) | medium | direct | probing
const scenario = params.get("net") ?? "relay";
let firewall: FirewallState = (params.get("fw") as FirewallState | null) ?? (scenario === "direct" ? "ok" : scenario === "probing" ? "unknown" : "missing");
const RELAY = "https://aps1-1.relay.n0.iroh.link./";
const PORT_REASON = "Router không hỗ trợ UPnP — mở cổng UDP 41820 thủ công để editor ở xa nối trực tiếp";

function mockDiag(): NetDiagUi {
  const d = mockDiag0();
  return lanBlocked
    ? { ...d, lan_blocked: true, reasons: ["macOS đang chặn Toss truy cập mạng cục bộ — vào Cài đặt hệ thống → Quyền riêng tư & Bảo mật → Mạng cục bộ và bật Toss, rồi mở lại Toss", ...d.reasons] }
    : d;
}
function mockDiag0(): NetDiagUi {
  const base = { lan_blocked: false, relay_url: RELAY, relay_connected: true, custom_relay: state.net.relay_urls.length > 0, direct_addrs: ["192.168.1.20:41820", "113.190.12.45:41820"], bind_port: 41820, mdns: state.net.mdns, firewall, log_dir: "/Users/minh/Library/Application Support/toss/logs" };
  switch (scenario) {
    case "direct": return {
      ...base, direct_addrs: ["113.190.12.45:41820", "[2402:800:6344:1c3::2]:41820", "192.168.1.20:41820"],
      portmap: { status: "mapped", external_addr: "113.190.12.45:41820", protocol: "upnp" },
      nat: { udp: true, global_v4: "113.190.12.45", global_v6: "2402:800:6344:1c3::2", mapping_varies_by_dest: false, hairpin: true, relay_latency_ms: 31 },
      direct_likelihood: "high", reasons: [],
    };
    case "medium": return {
      ...base, portmap: { status: "unavailable", external_addr: null, protocol: null },
      nat: { udp: true, global_v4: "113.190.12.45", global_v6: null, mapping_varies_by_dest: false, hairpin: false, relay_latency_ms: 38 },
      direct_likelihood: "medium", reasons: [PORT_REASON],
    };
    case "probing": return {
      ...base, relay_url: "", relay_connected: false, direct_addrs: [], bind_port: null,
      portmap: { status: "probing", external_addr: null, protocol: null },
      nat: { udp: false, global_v4: null, global_v6: null, mapping_varies_by_dest: null, hairpin: null, relay_latency_ms: null },
      direct_likelihood: "unknown", reasons: [],
    };
    default: return {
      ...base, portmap: { status: "unavailable", external_addr: null, protocol: null },
      nat: { udp: true, global_v4: "113.190.12.45", global_v6: null, mapping_varies_by_dest: true, hairpin: false, relay_latency_ms: 38 },
      direct_likelihood: "low", reasons: ["NAT đối xứng: editor ở xa sẽ đi qua relay", PORT_REASON],
    };
  }
}

function mockReport(): ConnectReport {
  if (scenario === "direct") return {
    ok: true, path: "direct", rtt_ms: 11, connect_ms: 420, direct_after_ms: 1300,
    known_paths: ["udp 113.190.12.45:41820 (đang dùng)", "udp 192.168.1.20:41820", "relay https://aps1-1.relay.n0.iroh.link./"], reason: "",
  };
  if (scenario === "probing") return { ok: false, path: "unknown", rtt_ms: null, connect_ms: 8000, direct_after_ms: null, known_paths: [], reason: "Chưa nối được relay nên chưa thể thử." };
  return {
    ok: true, path: "relay", rtt_ms: 168, connect_ms: 1240, direct_after_ms: null,
    known_paths: ["relay https://aps1-1.relay.n0.iroh.link./ (đang dùng)", "udp 192.168.1.20:41820 (không phản hồi)", "udp 113.190.12.45:41820 (không phản hồi)"],
    reason: "Chưa đục được lỗ NAT sau 8 giây: máy tổng đứng sau NAT đối xứng và router chưa mở cổng UDP 41820.",
  };
}

// ---- api -----------------------------------------------------------------
export const mockApi: Commands & CommandsV05 & CommandsV051 & ExtraCommands = {
  async get_state() { await sleep(60); return structuredClone(state); },
  async finish_setup({ device_name, dest_dir }) {
    state.device_name = device_name; state.dest_dir = dest_dir; state.first_run = false; return structuredClone(state);
  },
  async set_device_name({ name }) { state.device_name = name; },
  async set_dest_dir({ path }) { state.dest_dir = path; },
  async set_autostart({ on }) { state.autostart = on; },
  async pick_folder({ title }) {
    await sleep(150);
    const picks = ["/Users/minh/Pictures/Giao hàng", "D:\\Studio\\RAW", "/Volumes/Studio/Tra-ve", "/Users/minh/Toss"];
    void title;
    return picks[pickN++ % picks.length];
  },
  async open_path({ path }) { console.info("[mock] open_path", path); },

  async set_host({ config }) { state.host = config ? structuredClone(config) : null; },
  async host_status() { return state.host ? hostStatus() : null; },
  async list_dir({ path }) { await sleep(80); return listFake(path); },
  async create_ticket({ spec }) { await sleep(120); return doCreate(spec, false); },
  async list_jobs() { return structuredClone(jobs); },
  async revoke({ id }) { const j = jobs.find((x) => x.id === id); if (j) j.revoked = true; },
  async create_admin_ticket({ days }) {
    const info = makeInfo({ id: hex(32), host: HOST_ID, job: "admin", root: "", path: "", ret: "", get: false, put: false, admin: true, exp: days ? now() + days * 86400 : 0 });
    issued.set(info.ticket, info);
    return info;
  },

  async parse_ticket({ text }) { await sleep(60); return decodeTicket(text); },
  async start_get({ ticket }) {
    const info = decodeTicket(ticket);
    const dup = [...transfers.values()].some((t) => t.op === "get" && t.status === "done" && t.job === info.job);
    if (dup && !reloadOk.has(info.job)) {
      reloadOk.add(info.job);
      throw new Error("Job này đã nhận xong, folder đã có đầy đủ file");
    }
    return launch(info, "get");
  },
  async start_put({ ticket, paths }) { return launch(decodeTicket(ticket), "put", paths); },
  async list_transfers() { return structuredClone([...transfers.values()]).sort((a, b) => b.started_at - a.started_at); },
  async pause({ id }) {
    const s = transfers.get(id); if (!s || s.status !== "running") return;
    s.status = "paused"; s.speed_bps = 0; s.eta_s = null; emitTransfer({ type: "paused", id });
  },
  async resume({ id }) {
    const s = transfers.get(id); if (!s || s.status !== "paused") return;
    s.status = "running"; runLoop(id, bytesPerTick.get(id) ?? 6 * MB);
    emitTransfer({ type: "progress", id, bytes_done: s.bytes_done, bytes_total: s.bytes_total, files_done: s.files_done, files_total: s.files_total, speed_bps: 0, eta_s: null, path: s.path, rtt_ms: s.rtt_ms, streams: s.streams });
  },
  async cancel({ id }) {
    const s = transfers.get(id); if (!s) return;
    window.clearInterval(timers.get(id)); timers.delete(id);
    s.status = "cancelled"; s.speed_bps = 0; s.eta_s = null;
    emitTransfer({ type: "error", id, code: "cancelled", msg: "Đã huỷ", retry_in_s: null });
  },
  async clear_finished() {
    for (const [id, s] of [...transfers]) if (s.status === "done" || s.status === "error" || s.status === "cancelled") transfers.delete(id);
  },

  async set_admin_ticket({ ticket }) {
    if (ticket) {
      const info = decodeTicket(ticket);
      if (!info.admin && !ticket.startsWith("toss1mockadmin")) throw new Error("Đây không phải link admin");
    }
    adminTicket = ticket;
  },
  async remote_status() { if (!adminTicket) throw new Error("Chưa có máy tổng từ xa"); await sleep(200); return hostStatus(); },
  async remote_list_dir({ path }) { await sleep(150); return listFake(path); },
  async remote_create_ticket({ spec }) { await sleep(250); return doCreate(spec, true); },
  async remote_list_jobs() { await sleep(150); return structuredClone(jobs); },
  async remote_revoke({ id }) { const j = jobs.find((x) => x.id === id); if (j) j.revoked = true; },

  async share_folder({ path, days, put }) {
    await sleep(600);
    if (!path.trim()) throw new Error("Thiếu đường dẫn folder");
    if (/FAIL/.test(path)) throw new Error("Không đọc được folder này (không có quyền truy cập)");
    if (!state.host) state.host = { roots: [], return_root: `${state.dest_dir}/Tra-ve`, max_parallel_jobs: 2, bind_port: 41820 };
    const info = doCreate({ job: baseOf(path), root: "qs-x", path: "", ret: baseOf(path), get: true, put, days }, false);
    // same folder shared again: the old quick-share entry is replaced (its previous ticket stays valid until it expires)
    const at = quickShares.findIndex((q) => q.path === path);
    if (at >= 0) quickShares.splice(at, 1);
    quickShares.unshift({ alias: `qs-${hex(8)}`, path, job_id: info.id });
    try { await navigator.clipboard.writeText(info.web_url); } catch { /* the real shell copies natively */ }
    return info;
  },
  async list_quick_shares() { await sleep(60); return structuredClone(quickShares); },
  async remove_quick_share({ alias }) {
    const at = quickShares.findIndex((q) => q.alias === alias);
    if (at < 0) throw new Error("Không tìm thấy folder đang chia sẻ");
    const [q] = quickShares.splice(at, 1);
    const j = jobs.find((x) => x.id === q.job_id); if (j) j.revoked = true;
  },
  async set_context_menu({ on }) { state.context_menu = on; },
  async set_auto_receive({ on }) { state.auto_receive = on; },

  async qr_svg({ text }) { return fakeQr(text); },
  async copy_text({ text }) { try { await navigator.clipboard.writeText(text); } catch { /* ignore */ } },
  async ui_ready() {
    if (uiReadyDone) return; // React StrictMode runs the App effect twice in dev; the real shell flushes its queue once
    uiReadyDone = true;
    // URL switches to exercise shell-initiated events: ?share=1 (right-click menu), ?deeplink=1 (new job) / ?deeplink=exists (already downloaded)
    if (params.get("share") === "1") setTimeout(() => emit("share", { path: params.get("path") ?? "/Users/minh/Pictures/Giao hàng/12A3 - RAW" }), 150);
    if (params.get("shareerror") === "1") setTimeout(() => emit("share:error", { msg: "Không mở được folder để chia sẻ" }), 150);
    if (params.get("push") === "1" && hung()) {
      const h = hung()!;
      setTimeout(() => {
        const p: IncomingPush = { push_id: hex(32), from_id: h.id, from_name: h.name, job: "12A3 - Ảnh lớp (đã chọn)", files: 214, bytes: 3.4 * GB, at: now() };
        pushes.push(p);
        emit("push:incoming", p);
      }, 1000);
    }
    if (updateOn) setTimeout(() => emit("update:available", structuredClone(NEW_RELEASE)), 400);
    if (params.get("invite") === "1") {
      const inv = makeInvite("Mai – Laptop", 7);
      setTimeout(() => emit("invite", { invite: inv.invite }), 200);
    }
    const dl = params.get("deeplink");
    if (dl) {
      const job = dl === "exists" ? "11B2 - RAW" : "12B1 - RAW";
      const info = makeInfo({ id: hex(32), host: HOST_ID, job, root: "RAW", path: "2026/" + job, ret: job, get: true, put: false, admin: false, exp: now() + 6 * 86400 });
      issued.set(info.ticket, info);
      setTimeout(() => emit("deep-link", { ticket: info.deep_link }), 150);
    }
  },
  async has_admin_ticket() { return adminTicket !== null; },

  // ---- v0.5 ----------------------------------------------------------------
  async list_contacts() { await sleep(60); return structuredClone(contacts); },
  async create_invite({ days }) {
    await sleep(250);
    const info = makeInvite("", days);
    issuedInvites++;
    try { await navigator.clipboard.writeText(info.web_url); } catch { /* the real shell copies natively */ }
    return info;
  },
  async revoke_invites() { await sleep(150); issuedInvites = 0; },
  async expand_link({ text }) { await sleep(80); return text; },
  async parse_invite({ text }) { await sleep(120); return decodeInvite(text); },
  async accept_invite({ text }) {
    const info = decodeInvite(text);
    await sleep(700);
    if (params.get("invitefail") === "1") throw new Error("Máy kia không trả lời, hãy kiểm tra họ đang mở Toss");
    const c: Contact = { id: hex(64), name: info.name, their_name: info.name, platform: /win|pc/i.test(info.name) ? "windows" : "macos", added_at: now(), auto_accept: true, online: true, last_seen: now(), rtt_ms: 18 };
    contacts.push(c);
    emit("contacts:changed", {});
    return structuredClone(c);
  },
  async rename_contact({ id, name }) {
    const c = contacts.find((x) => x.id === id); if (!c) throw new Error("Không tìm thấy máy này");
    c.name = name.trim(); emit("contacts:changed", {});
  },
  async set_contact_auto_accept({ id, on }) {
    const c = contacts.find((x) => x.id === id); if (!c) throw new Error("Không tìm thấy máy này");
    c.auto_accept = on; emit("contacts:changed", {});
  },
  async remove_contact({ id }) {
    const at = contacts.findIndex((x) => x.id === id); if (at < 0) throw new Error("Không tìm thấy máy này");
    contacts.splice(at, 1); emit("contacts:changed", {});
  },
  async send_to_contact({ id, paths }) {
    const c = contacts.find((x) => x.id === id); if (!c) throw new Error("Không tìm thấy máy này");
    if (paths.length === 0) throw new Error("Chưa chọn file hoặc folder nào");
    const tid = `p-${hex(6)}`;
    const name = baseOf(paths[0]);
    const job = paths.length > 1 ? `${paths.length} mục` : name;
    const s = tfBase({ id: tid, job, op: "put", peer: c.name, contact: c.id });
    transfers.set(tid, s);
    emitTransfer({ type: "started", id: tid, job, op: "put", side: "client", peer: c.name, contact: c.id });
    if (c.online) {
      void (async () => {
        await sleep(700);
        if (s.status !== "connecting") return;
        s.bytes_total = Math.round((0.8 + Math.random() * 2) * GB); s.files_total = 60 + Math.floor(Math.random() * 120);
        s.status = "running"; s.path = "direct"; s.rtt_ms = c.rtt_ms ?? 10; s.streams = 8;
        emitTransfer({ type: "manifest", id: tid, files: s.files_total, bytes: s.bytes_total, to_do_files: s.files_total, to_do_bytes: s.bytes_total });
        runLoop(tid, 9 * MB);
      })();
    } // offline: stays "connecting" = "Chờ <tên> online" until presence sees them
    return tid;
  },
  async list_incoming_pushes() { return structuredClone(pushes); },
  async answer_push({ push_id, accept }) {
    const at = pushes.findIndex((p) => p.push_id === push_id); if (at < 0) throw new Error("Yêu cầu này đã hết hạn");
    const [p] = pushes.splice(at, 1);
    if (!accept) return;
    const tid = `in-${hex(6)}`;
    const s = tfBase({ id: tid, job: p.job, op: "put", side: "host", peer: p.from_name, contact: p.from_id });
    transfers.set(tid, s);
    emitTransfer({ type: "started", id: tid, job: p.job, op: "put", side: "host", peer: p.from_name, contact: p.from_id });
    void (async () => {
      await sleep(600);
      s.bytes_total = p.bytes; s.files_total = p.files; s.status = "running"; s.path = "direct"; s.rtt_ms = 31; s.streams = 8;
      s.dest = `${state.dest_dir}/${p.from_name}/${p.job}`;
      emitTransfer({ type: "manifest", id: tid, files: p.files, bytes: p.bytes, to_do_files: p.files, to_do_bytes: p.bytes });
      runLoop(tid, 9 * MB);
    })();
  },
  async open_local_network_settings() { console.info("[mock] open_local_network_settings"); },

  async get_diagnostics() { await sleep(120); return mockDiag(); },
  async add_firewall_rule() {
    await sleep(900);
    if (params.get("fwfail") === "1") throw new Error("Bạn đã từ chối quyền quản trị, firewall chưa được mở");
    firewall = "ok";
  },
  async test_connect({ ticket }) {
    if (!ticket.trim() && !adminTicket) throw new Error("Chưa có máy tổng — dán một link của máy tổng để thử");
    await sleep(params.get("fast") === "1" ? 200 : 1800);
    return mockReport();
  },
  async set_net({ net }) { await sleep(150); state.net = structuredClone(net); },
  async export_logs() { await sleep(400); return "/Users/minh/Desktop/toss-logs-2026-10-22-091500.zip"; },

  // ---- v0.5.1 ----------------------------------------------------------------
  async check_update() {
    await sleep(900);
    return updateOn ? structuredClone(NEW_RELEASE) : { available: false, current: state.version, version: null, notes: null, forced: false, date: null };
  },
  async install_update() {
    if (!updateOn || installing) throw new Error("Bạn đang dùng bản mới nhất (hoặc đang cập nhật)");
    installing = true;
    const running = [...transfers.values()].filter((t) => t.status === "running").length;
    if (running > 0 && params.get("updatewait") === "1") {
      emit("update:phase", { phase: "waiting", active: running });
      await sleep(2500);
    }
    const total = Math.round(14.6 * MB);
    emit("update:phase", { phase: "downloading", active: 0 });
    for (let d = 0; d < total; d += Math.round(total / 24)) {
      emit("update:progress", { downloaded: d, total });
      await sleep(160);
    }
    emit("update:progress", { downloaded: total, total });
    emit("update:phase", { phase: "installing", active: 0 });
    await sleep(1200); // the real shell restarts the app here
  },
  async set_auto_update({ on }) { state.auto_update = on; },
  async remote_update() {
    if (!adminTicket) throw new Error("Chưa có máy tổng từ xa");
    await sleep(500);
    if (remoteVersion === "0.5.0") throw new Error("Máy tổng đang chạy bản cũ, chưa hỗ trợ cập nhật từ xa — cập nhật tay lần này");
    setTimeout(() => { remoteVersion = "0.5.2"; }, 15_000);
  },
};

// deterministic pseudo-QR so the mock looks right (NOT scannable)
function fakeQr(text: string): string {
  const n = 29; let h = 2166136261;
  for (const c of text) { h ^= c.charCodeAt(0); h = Math.imul(h, 16777619); }
  const rnd = () => { h ^= h << 13; h ^= h >>> 17; h ^= h << 5; return (h >>> 0) / 4294967296; };
  let rects = "";
  const finder = (x: number, y: number) => x < 8 && y < 8 || x >= n - 8 && y < 8 || x < 8 && y >= n - 8;
  for (let y = 0; y < n; y++) for (let x = 0; x < n; x++) {
    let on = rnd() > 0.52;
    if (finder(x, y)) {
      const fx = x < 8 ? x : x - (n - 8), fy = y < 8 ? y : y - (n - 8);
      on = fx === 0 || fx === 6 || fy === 0 || fy === 6 || (fx >= 2 && fx <= 4 && fy >= 2 && fy <= 4);
      if (fx === 7 || fy === 7) on = false;
    }
    if (on) rects += `<rect x="${x + 2}" y="${y + 2}" width="1" height="1"/>`;
  }
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${n + 4} ${n + 4}" shape-rendering="crispEdges"><rect width="100%" height="100%" fill="#fff"/><g fill="#000">${rects}</g></svg>`;
}

// ---- seed + background simulation ------------------------------------------
seed();
seedJobs();
seedShares();
if (params.get("restored") === "1") seedRestored();
// screenshot / test hook (mock only): lets a script fire shell events such as drop:hover / drop:over
(window as unknown as { __tossMock?: unknown }).__tossMock = { emit };
