// In-memory mock of the shell (VITE_MOCK=1). Simulates tickets, host mode, remote
// admin control and transfers with progress so the UI can be built and screenshotted
// without Rust. Implements the same surface as Commands (+ ExtraCommands).
import type {
  AppState, Commands, DirEntry, HostConfig, HostStatus, JobRecord,
  TicketInfo, TicketSpec, TransferEvent, TransferState,
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

function makeInfo(body: Omit<TicketInfo, "ticket" | "web_url" | "deep_link">): TicketInfo {
  const json = JSON.stringify(body);
  const ticket = "toss1" + b32enc(new TextEncoder().encode(json)) + b32enc(crypto.getRandomValues(new Uint8Array(24)));
  return withUrls({ ...body, ticket });
}
function withUrls(t: Omit<TicketInfo, "web_url" | "deep_link">): TicketInfo {
  return { ...t, web_url: `https://${state.page_host}/#${t.ticket}`, deep_link: `toss://t/${t.ticket}` };
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
};

const state: AppState = {
  version: "0.1.0",
  endpoint_id: HOST_ID,
  device_name: "MacBook của Minh",
  dest_dir: "/Users/minh/Toss",
  host: hostOn ? structuredClone(demoHost) : null,
  page_host: "minhld6688-droid.github.io/toss",
  autostart: true,
  first_run: wizard,
};
let adminTicket: string | null = params.get("remote") === "1" ? "toss1mockadmin" + "abcdefghijklmnopqrstuvwxyz234567" : null;

const transfers = new Map<string, TransferState>();
const timers = new Map<string, number>();
const bytesPerTick = new Map<string, number>();
const startedAt = Date.now();

function tfBase(p: Partial<TransferState> & { id: string; job: string }): TransferState {
  return {
    op: "get", side: "client", peer: "Máy tổng", status: "connecting", bytes_done: 0, bytes_total: 0,
    files_done: 0, files_total: 0, speed_bps: 0, eta_s: null, path: "unknown", dest: null, error: null,
    started_at: now(), ...p,
  };
}

function seed() {
  const dest = `${state.dest_dir}/11B2 - RAW`;
  transfers.set("a1-0001", tfBase({
    id: "a1-0001", job: "11B2 - RAW", status: "done", bytes_done: 4.2 * GB, bytes_total: 4.2 * GB,
    files_done: 318, files_total: 318, path: "direct", dest, started_at: now() - 3600,
  }));
  transfers.set("a2-0002", tfBase({
    id: "a2-0002", job: "12A3 - Retouch (trả)", op: "put", status: "done", bytes_done: 1.1 * GB, bytes_total: 1.1 * GB,
    files_done: 96, files_total: 96, path: "relay", started_at: now() - 7200,
  }));
  transfers.set("a3-0003", tfBase({
    id: "a3-0003", job: "12A3 - RAW", status: "running", bytes_done: 1.7 * GB, bytes_total: 5.6 * GB,
    files_done: 121, files_total: 402, speed_bps: 38 * MB, eta_s: 103, path: "direct",
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
      emitTransfer({ type: "progress", id, bytes_done: s.bytes_total, bytes_total: s.bytes_total, files_done: s.files_total, files_total: s.files_total, speed_bps: 0, eta_s: 0, path: s.path });
      emitTransfer({ type: "done", id, files: s.files_total, bytes: s.bytes_total, elapsed_s: Math.max(1, now() - s.started_at), dest: s.dest });
      return;
    }
    emitTransfer({ type: "progress", id, bytes_done: Math.round(s.bytes_done), bytes_total: s.bytes_total, files_done: s.files_done, files_total: s.files_total, speed_bps: s.speed_bps, eta_s: s.eta_s, path: s.path });
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
    jobs.push({ id: info.id, job, root, path, get, put, exp: info.exp, created: now() - 86400 * 2, revoked, last, ticket: info.ticket });
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
  jobs.unshift({ id: info.id, job: info.job, root: info.root, path: info.path, get: info.get, put: info.put, exp: info.exp, created: now(), revoked: false, last: null, ticket: remote ? "" : info.ticket });
  return info;
}

function hostStatus(): HostStatus {
  const cfg = state.host ?? demoHost;
  const active = [...transfers.values()].filter((t) => t.status === "running").length;
  return { device: remoteDevice(), version: "0.1.0", roots: cfg.roots.map((r) => r.alias), active, queued: 0, uptime_s: Math.floor((Date.now() - startedAt) / 1000) + 5400 };
}
const remoteDevice = () => (adminTicket ? "Máy tổng - Studio" : state.device_name);

let pickN = 0;

// ---- api -----------------------------------------------------------------
export const mockApi: Commands & ExtraCommands = {
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
  async start_get({ ticket }) { return launch(decodeTicket(ticket), "get"); },
  async start_put({ ticket, paths }) { return launch(decodeTicket(ticket), "put", paths); },
  async list_transfers() { return structuredClone([...transfers.values()]).sort((a, b) => b.started_at - a.started_at); },
  async pause({ id }) {
    const s = transfers.get(id); if (!s || s.status !== "running") return;
    s.status = "paused"; s.speed_bps = 0; s.eta_s = null; emitTransfer({ type: "paused", id });
  },
  async resume({ id }) {
    const s = transfers.get(id); if (!s || s.status !== "paused") return;
    s.status = "running"; runLoop(id, bytesPerTick.get(id) ?? 6 * MB);
    emitTransfer({ type: "progress", id, bytes_done: s.bytes_done, bytes_total: s.bytes_total, files_done: s.files_done, files_total: s.files_total, speed_bps: 0, eta_s: null, path: s.path });
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

  async qr_svg({ text }) { return fakeQr(text); },
  async copy_text({ text }) { try { await navigator.clipboard.writeText(text); } catch { /* ignore */ } },
  async ui_ready() { /* nothing queued in mock */ },
  async has_admin_ticket() { return adminTicket !== null; },
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
