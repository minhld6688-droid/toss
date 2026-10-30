import { create } from "zustand";
import type { AppState, Contact, FirewallState, Incoming, Nearby, Received, TextMsg, TicketInfo, TransferEvent, TransferEventV05, TransferState, WebShare } from "@contracts/ipc";
import { api, errorMessage } from "./ipc";

export type Screen = "home" | "transfers" | "settings";

export interface Recent {
  ticket: string; job: string; get: boolean; put: boolean; exp: number; at: number;
}

/** Why the confirm sheet is shown instead of the transfer starting by itself (auto-receive). */
export interface SheetNote { kind: "exists" | "error"; text: string }

/** Heuristic: does a start_get rejection mean "this job is already downloaded"? (shell wording unknown) */
const EXISTS_RE = /already|exist|complete|đã (có|xong|nhận|tồn tại|đầy đủ)|hoàn tất/i;

interface Toast { id: number; text: string; kind: "info" | "error" }

/** "Thêm máy" sheet: which tab, and an invite to prefill (deep link / paste). */
export interface AddDevice { tab: "invite" | "enter"; text: string }

/** "Gửi tới ai?": files/folders (paths) or a piece of text waiting for a recipient. */
export interface SendRequest { paths: string[]; text?: string; nonce: number }

/** Everything a new sheet closes first, so two sheets never stack by accident. */
const NO_SHEETS = { sheet: null, sheetNote: null, share: null, send: null, contactSheet: null, peerSheet: null, addDevice: null } as const;

interface Store {
  screen: Screen;
  app: AppState | null;
  transfers: Record<string, TransferState>;
  /** ticket currently shown in the confirm sheet */
  sheet: TicketInfo | null;
  sheetNote: SheetNote | null;
  /** Share view ("Tạo link"): file/folder being shared; nonce remounts the view for a new request */
  share: { path: string; nonce: number } | null;
  send: SendRequest | null;
  /** bumped after every share / remove so the settings list reloads */
  sharesRev: number;
  recents: Recent[];
  toasts: Toast[];
  dropHover: boolean;
  /** comets in flight toward the "Chuyển file" tab (pure decoration) */
  flights: { id: number; x: number; y: number }[];
  /** bumped when a comet lands, the tab answers with a pulse */
  navPulse: number;
  /** "Vì sao?" on a relay transfer: timestamp of the last request to open the Chẩn đoán section (0 = none) */
  diagFocus: number;
  contacts: Contact[];
  contactsLoaded: boolean;
  /** v0.8 devices on this network (Toss + LocalSend), live */
  nearby: Nearby[];
  nearbyLoaded: boolean;
  scanning: boolean;
  /** asks waiting for the user's answer (Toss pushes and LocalSend requests alike) */
  incoming: Incoming[];
  incomingSheet: boolean;
  /** text messages received, oldest first; the first one is shown */
  texts: TextMsg[];
  /** received history ("Đã nhận"), newest first */
  received: Received[];
  /** browser share running on the LAN (Chia sẻ qua trình duyệt) */
  webShare: WebShare | null;
  /** macOS blocks Toss from the local network (NetDiag.lan_blocked / net:lan_blocked) */
  lanBlocked: boolean;
  /** OS firewall rule for Toss (Windows): "missing" → Home offers a one-click fix */
  firewall: FirewallState;
  addDevice: AddDevice | null;
  /** contact whose detail sheet is open */
  contactSheet: string | null;
  /** nearby device (Nearby.key) whose sheet is open */
  peerSheet: string | null;
  /** row under a dragged file (Nearby.key or contact id): highlighted drop target */
  hoverKey: string | null;

  refreshContacts(): Promise<void>;
  refreshNearby(): Promise<void>;
  rescanNearby(): Promise<void>;
  refreshIncoming(): Promise<void>;
  refreshReceived(): Promise<void>;
  setLanBlocked(b: boolean): void;
  setFirewall(f: FirewallState): void;
  presence(id: string, online: boolean, rtt: number | null): void;
  addIncoming(p: Incoming): void;
  removeIncoming(id: string): void;
  setIncomingSheet(open: boolean): void;
  pushText(m: TextMsg): void;
  dismissText(): void;
  addReceived(r: Received): void;
  setWebShare(w: WebShare | null): void;
  openAddDevice(tab?: AddDevice["tab"], text?: string): void;
  closeAddDevice(): void;
  openContact(id: string): void;
  closeContact(): void;
  openPeer(key: string): void;
  closePeer(): void;
  setHoverKey(key: string | null): void;
  /** display name for a Nearby.key or a contact id */
  nameOf(key: string): string | null;
  /** send_to + comet + jump to Chuyển file. Resolves false (after a toast) on failure. */
  sendTo(key: string, paths: string[], origin?: { x: number; y: number }): Promise<boolean>;
  /** send_text + toast. Resolves false (after a toast) on failure. */
  sendText(key: string, text: string): Promise<boolean>;
  /** `invite` deep link / pasted invite: open "Thêm máy" on the paste tab with the confirm */
  receiveInvite(invite: string): void;

  setScreen(s: Screen): void;
  /** Settings → Chẩn đoán & nâng cao → Kết nối, scrolled into view */
  openDiagnostics(): void;
  setApp(a: AppState): void;
  patchApp(p: Partial<AppState>): void;
  openTicket(text: string): Promise<boolean>;
  /** deep-link entry: honours app.auto_receive */
  receiveLink(text: string): Promise<void>;
  closeSheet(): void;
  openShare(path: string): void;
  closeShare(): void;
  /** "Gửi tới ai?" for these paths (window drop, menu bar / Dock icon, right-click, tray, pickers) */
  openSend(paths: string[]): void;
  /** "Gửi chữ tới ai?" */
  openSendText(text?: string): void;
  closeSend(): void;
  /** pick files or a folder, then "Gửi tới ai?" */
  pickAndSend(kind?: "files" | "folder"): Promise<void>;
  bumpShares(): void;
  toast(text: string, kind?: "info" | "error"): void;
  refreshTransfers(): Promise<void>;
  applyEvent(ev: TransferEvent | TransferEventV05): void;
  setDropHover(on: boolean): void;
  fly(from?: { x: number; y: number }): void;
  land(id: number): void;
}

const RECENT_KEY = "toss.recents.v1";
function loadRecents(): Recent[] {
  try { return JSON.parse(localStorage.getItem(RECENT_KEY) || "[]"); } catch { return []; }
}
function saveRecents(r: Recent[]) {
  try { localStorage.setItem(RECENT_KEY, JSON.stringify(r)); } catch { /* storage may be unavailable */ }
}

let toastSeq = 1;
let flightSeq = 1;

/** parse_ticket + remember in "Link đã mở". Toasts and returns null on a bad link. */
async function parseAndRemember(text: string): Promise<TicketInfo | null> {
  try {
    const info = await api.parse_ticket({ text });
    if (!info.admin) {
      const rec: Recent = { ticket: info.ticket, job: info.job, get: info.get, put: info.put, exp: info.exp, at: Date.now() };
      const recents = [rec, ...useStore.getState().recents.filter((r) => r.ticket !== info.ticket)].slice(0, 12);
      saveRecents(recents);
      useStore.setState({ recents });
    }
    return info;
  } catch (e) {
    useStore.getState().toast(`Link không hợp lệ: ${errorMessage(e)}`, "error");
    return null;
  }
}

/** A "Link điều khiển" is not something to receive: point at the one place that takes it. */
function adminNotice() {
  useStore.getState().toast("Đây là link điều khiển — dán ở Cài đặt → Chẩn đoán & nâng cao → Điều khiển máy khác");
}

let nearbyTimer = 0;

export const useStore = create<Store>((set, get) => ({
  screen: "home",
  app: null,
  transfers: {},
  sheet: null,
  sheetNote: null,
  share: null,
  send: null,
  sharesRev: 0,
  recents: loadRecents(),
  toasts: [],
  dropHover: false,
  flights: [],
  navPulse: 0,
  diagFocus: 0,
  contacts: [],
  contactsLoaded: false,
  nearby: [],
  nearbyLoaded: false,
  scanning: false,
  incoming: [],
  incomingSheet: false,
  texts: [],
  received: [],
  webShare: null,
  lanBlocked: false,
  firewall: "unknown",
  addDevice: null,
  contactSheet: null,
  peerSheet: null,
  hoverKey: null,

  async refreshContacts() {
    try {
      const contacts = await api.list_contacts();
      set({ contacts, contactsLoaded: true });
    } catch {
      // older core / not supported yet: an empty book is the right fallback
      set({ contactsLoaded: true });
    }
  },
  async refreshNearby() {
    try {
      const nearby = await api.list_nearby();
      set({ nearby, nearbyLoaded: true });
    } catch { set({ nearbyLoaded: true }); }
  },
  async rescanNearby() {
    if (get().scanning) return;
    set({ scanning: true });
    try { await api.rescan_nearby(); } catch (e) { get().toast(errorMessage(e), "error"); }
    await get().refreshNearby();
    set({ scanning: false });
  },
  async refreshIncoming() {
    try {
      const incoming = await api.list_incoming();
      set((s) => ({ incoming, incomingSheet: incoming.length > 0 && (s.incomingSheet || incoming.some((p) => !s.incoming.some((o) => o.id === p.id))) }));
    } catch { /* older shell */ }
  },
  async refreshReceived() {
    try { set({ received: await api.list_received() }); } catch { /* older shell */ }
  },
  setLanBlocked: (lanBlocked) => set((s) => (s.lanBlocked === lanBlocked ? s : { lanBlocked })),
  setFirewall: (firewall) => set({ firewall }),
  presence(id, online, rtt) {
    set((s) => ({
      contacts: s.contacts.map((c) => c.id !== id ? c : {
        ...c, online, rtt_ms: online ? rtt : null,
        last_seen: online || !c.online ? c.last_seen : Math.floor(Date.now() / 1000),
      }),
    }));
  },
  addIncoming(p) {
    set((s) => (s.incoming.some((x) => x.id === p.id) ? s : { incoming: [...s.incoming, p], incomingSheet: true, addDevice: null, contactSheet: null, peerSheet: null }));
  },
  removeIncoming: (id) => set((s) => {
    const incoming = s.incoming.filter((p) => p.id !== id);
    return { incoming, incomingSheet: incoming.length > 0 && s.incomingSheet };
  }),
  setIncomingSheet: (incomingSheet) => set({ incomingSheet }),
  pushText: (m) => set((s) => ({ texts: [...s.texts, m].slice(-10) })),
  dismissText: () => set((s) => ({ texts: s.texts.slice(1) })),
  addReceived: (r) => set((s) => ({ received: [r, ...s.received.filter((x) => x.id !== r.id)].slice(0, 30) })),
  setWebShare: (webShare) => set({ webShare }),
  openAddDevice: (tab = "invite", text = "") => set({ ...NO_SHEETS, addDevice: { tab, text }, screen: "home" }),
  closeAddDevice: () => set({ addDevice: null }),
  openContact: (id) => set({ contactSheet: id, peerSheet: null }),
  closeContact: () => set({ contactSheet: null }),
  openPeer: (key) => set({ peerSheet: key, contactSheet: null }),
  closePeer: () => set({ peerSheet: null }),
  setHoverKey: (hoverKey) => set((s) => (s.hoverKey === hoverKey ? s : { hoverKey })),
  receiveInvite: (invite) => set({ ...NO_SHEETS, addDevice: { tab: "enter", text: invite }, screen: "home" }),

  nameOf(key) {
    const s = get();
    return s.contacts.find((c) => c.id === key)?.name ?? s.nearby.find((n) => n.key === key)?.name ?? null;
  },

  async sendTo(key, paths, origin) {
    const s = get();
    const c = s.contacts.find((x) => x.id === key);
    const name = s.nameOf(key) ?? "máy kia";
    const seen = s.nearby.some((n) => n.key === key || (c && n.contact_id === c.id));
    try {
      await api.send_to({ key, paths });
      await get().refreshTransfers();
      get().fly(origin);
      set({ screen: "transfers", contactSheet: null, peerSheet: null, send: null });
      get().toast(c && !c.online && !seen ? `${name} đang offline, sẽ gửi khi máy online` : `Đang gửi cho ${name}…`);
      return true;
    } catch (e) {
      get().toast(errorMessage(e), "error");
      return false;
    }
  },

  async sendText(key, text) {
    try {
      await api.send_text({ key, text });
      set({ send: null, contactSheet: null, peerSheet: null });
      get().toast(`Đã gửi chữ cho ${get().nameOf(key) ?? "máy kia"}`);
      return true;
    } catch (e) {
      get().toast(errorMessage(e), "error");
      return false;
    }
  },

  setScreen: (screen) => set({ screen }),
  openDiagnostics: () => set({ screen: "settings", sheet: null, sheetNote: null, diagFocus: Date.now() }),
  setApp: (app) => set({ app }),
  patchApp: (p) => set((s) => (s.app ? { app: { ...s.app, ...p } } : s)),
  setDropHover: (dropHover) => set({ dropHover }),
  fly(from) {
    const p = from ?? { x: window.innerWidth / 2, y: window.innerHeight * 0.32 };
    set((s) => ({ flights: [...s.flights, { id: flightSeq++, ...p }].slice(-4) }));
  },
  land: (id) => set((s) => ({ flights: s.flights.filter((f) => f.id !== id), navPulse: s.navPulse + 1 })),

  async openTicket(text) {
    const info = await parseAndRemember(text);
    if (!info) return false;
    if (info.admin) { adminNotice(); return true; }
    set({ ...NO_SHEETS, sheet: info, screen: "home" });
    return true;
  },

  async receiveLink(text) {
    const info = await parseAndRemember(text);
    if (!info) return;
    if (info.admin) { adminNotice(); return; }
    const exp = info.exp && info.exp <= Date.now() / 1000;
    if (!get().app?.auto_receive || !info.get || exp) {
      set({ ...NO_SHEETS, sheet: info, screen: "home" });
      return;
    }
    try {
      await api.start_get({ ticket: info.ticket });
      await get().refreshTransfers();
      get().fly();
      set({ ...NO_SHEETS, screen: "transfers" });
      get().toast(`Đang nhận ${info.job}…`);
    } catch (e) {
      const msg = errorMessage(e);
      if (EXISTS_RE.test(msg)) {
        set({ ...NO_SHEETS, sheet: info, sheetNote: { kind: "exists", text: "Job này đã nhận xong trước đó. Bấm Tải lại nếu muốn nhận lại." }, screen: "home" });
      } else {
        // unknown rejection: show the sheet so the user can retry by hand, and say why
        set({ ...NO_SHEETS, sheet: info, sheetNote: { kind: "error", text: msg }, screen: "home" });
      }
    }
  },
  closeSheet: () => set({ sheet: null, sheetNote: null }),
  openShare: (path) => set({ ...NO_SHEETS, share: { path, nonce: Date.now() } }),
  closeShare: () => set({ share: null, screen: "home" }),
  openSend: (paths) => set({ ...NO_SHEETS, send: { paths, nonce: Date.now() }, screen: "home" }),
  openSendText: (text = "") => set({ ...NO_SHEETS, send: { paths: [], text, nonce: Date.now() }, screen: "home" }),
  closeSend: () => set({ send: null }),
  async pickAndSend(kind = "folder") {
    try {
      if (kind === "files") {
        const p = await api.pick_files({ title: "Chọn file để gửi" });
        if (p.length > 0) get().openSend(p);
      } else {
        const p = await api.pick_folder({ title: "Chọn folder để gửi" });
        if (p) get().openSend([p]);
      }
    } catch (e) { get().toast(errorMessage(e), "error"); }
  },
  bumpShares: () => set((s) => ({ sharesRev: s.sharesRev + 1 })),

  toast(text, kind = "info") {
    const id = toastSeq++;
    set((s) => ({ toasts: [...s.toasts, { id, text, kind }] }));
    setTimeout(() => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })), 4000);
  },

  async refreshTransfers() {
    try {
      const list = await api.list_transfers();
      const map: Record<string, TransferState> = {};
      for (const t of list) map[t.id] = t;
      set({ transfers: map });
    } catch (e) {
      get().toast(errorMessage(e), "error");
    }
  },

  applyEvent(ev) {
    // events that have their own listeners (contacts:changed, nearby:changed, text …) also come through here
    if (ev.type === "contacts_changed" || ev.type === "contact_presence" || ev.type === "push_incoming" || ev.type === "lan_blocked") return;
    const kind: string = ev.type;
    if (kind === "host_status" || kind === "nearby_changed" || kind === "text_incoming") {
      if (kind === "nearby_changed") scheduleNearbyRefresh();
      return;
    }
    const e = ev as TransferEvent & { id: string };
    const cur = get().transfers;
    const t = cur[e.id];
    let next: TransferState | undefined = t;
    switch (e.type) {
      case "started":
        next = {
          id: e.id, job: e.job, op: e.op, side: e.side, peer: e.peer, status: "connecting",
          bytes_done: 0, bytes_total: 0, files_done: 0, files_total: 0, speed_bps: 0, eta_s: null,
          path: "unknown", rtt_ms: null, streams: 0, dest: null, error: null, started_at: Math.floor(Date.now() / 1000),
          contact: e.contact ?? null,
        };
        break;
      case "manifest":
        if (t) next = { ...t, status: "running", bytes_total: e.bytes, files_total: e.files };
        break;
      case "progress":
        if (t) next = {
          ...t, status: t.status === "paused" ? "paused" : "running", bytes_done: e.bytes_done, bytes_total: e.bytes_total,
          files_done: e.files_done, files_total: e.files_total, speed_bps: e.speed_bps, eta_s: e.eta_s, path: e.path, rtt_ms: e.rtt_ms ?? null, streams: e.streams ?? 0, error: null,
        };
        break;
      case "done":
        if (t) next = { ...t, status: "done", bytes_done: e.bytes, bytes_total: e.bytes, files_done: e.files, files_total: e.files, speed_bps: 0, eta_s: 0, dest: e.dest ?? t.dest, error: null };
        break;
      case "error":
        if (t) {
          if (e.code === "cancelled") next = { ...t, status: "cancelled", speed_bps: 0, eta_s: null, error: null };
          else if (e.retry_in_s != null) next = { ...t, status: "connecting", speed_bps: 0, eta_s: null, error: retryText(e.code, e.retry_in_s) };
          else next = { ...t, status: "error", speed_bps: 0, eta_s: null, error: errorText(e.code, e.msg) };
        }
        break;
      case "paused":
        if (t) next = { ...t, status: "paused", speed_bps: 0, eta_s: null };
        break;
    }
    if (!next) {
      // event for a transfer we have not seen (e.g. a receive started by the shell): resync
      void get().refreshTransfers();
      return;
    }
    set({ transfers: { ...cur, [e.id]: next } });
  },
}));

/** nearby:changed can fire in bursts while devices announce themselves: one refresh per burst. */
export function scheduleNearbyRefresh() {
  window.clearTimeout(nearbyTimer);
  nearbyTimer = window.setTimeout(() => { void useStore.getState().refreshNearby(); }, 200);
}

export function retryText(code: string, secs: number): string {
  if (code === "busy") return `Máy gửi đang bận, thử lại sau ${secs} giây`;
  if (code === "host_offline") return `Máy gửi đang offline, thử lại sau ${secs} giây`;
  return `Lỗi, thử lại sau ${secs} giây`;
}

export function errorText(code: string, msg: string): string {
  switch (code) {
    case "denied": return msg || "Link không có quyền truy cập";
    case "expired": return "Link đã hết hạn";
    case "revoked": return "Link đã bị thu hồi";
    case "host_offline": return "Máy gửi đang offline";
    case "busy": return "Máy gửi đang bận";
    case "hash_mismatch": return "File bị lỗi khi truyền (sai mã kiểm tra)";
    case "io": return `Lỗi ghi/đọc file: ${msg}`;
    default: return msg || "Có lỗi xảy ra";
  }
}
