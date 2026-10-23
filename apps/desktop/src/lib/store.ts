import { create } from "zustand";
import type { AppState, Contact, HostStatus, IncomingPush, TicketInfo, TransferEvent, TransferEventV05, TransferState } from "./ipc-types";
import { api, errorMessage } from "./ipc";

export type Screen = "home" | "transfers" | "settings";

export interface Recent {
  ticket: string; job: string; get: boolean; put: boolean; admin: boolean; exp: number; at: number;
}

/** Why the confirm sheet is shown instead of the transfer starting by itself (auto-receive). */
export interface SheetNote { kind: "exists" | "error"; text: string }

/** Heuristic: does a start_get rejection mean "this job is already downloaded"? (shell wording unknown) */
const EXISTS_RE = /already|exist|complete|đã (có|xong|nhận|tồn tại|đầy đủ)|hoàn tất/i;

interface Toast { id: number; text: string; kind: "info" | "error" }

/** "Thêm máy" sheet: which tab, and an invite to prefill (deep link / paste). */
export interface AddDevice { tab: "invite" | "enter"; text: string }

interface Store {
  screen: Screen;
  app: AppState | null;
  transfers: Record<string, TransferState>;
  /** ticket currently shown in the confirm sheet */
  sheet: TicketInfo | null;
  sheetNote: SheetNote | null;
  /** Share view ("Tạo link"): folder being shared; nonce remounts the view for a new request */
  share: { path: string; nonce: number } | null;
  /** folder dropped on the window, waiting for "Tạo link chia sẻ folder này?" */
  shareAsk: string | null;
  /** bumped after every share / remove so the settings list reloads */
  sharesRev: number;
  recents: Recent[];
  toasts: Toast[];
  hostStatus: HostStatus | null;
  dropHover: boolean;
  /** comets in flight toward the "Chuyển file" tab (pure decoration) */
  flights: { id: number; x: number; y: number }[];
  /** bumped when a comet lands, the tab answers with a pulse */
  navPulse: number;
  /** "Vì sao?" on a relay transfer: timestamp of the last request to open the Kết nối section (0 = none) */
  diagFocus: number;
  /** v0.5 Danh bạ máy */
  contacts: Contact[];
  contactsLoaded: boolean;
  /** pushes waiting for the user's answer (not auto-accepted) */
  pushes: IncomingPush[];
  /** the "X muốn gửi…" sheet is open */
  pushSheet: boolean;
  /** macOS blocks Toss from the local network (NetDiag.lan_blocked / net:lan_blocked) */
  lanBlocked: boolean;
  addDevice: AddDevice | null;
  /** contact whose detail sheet is open */
  contactSheet: string | null;
  /** contact row under a dragged file (highlighted drop target) */
  hoverContact: string | null;

  refreshContacts(): Promise<void>;
  refreshPushes(): Promise<void>;
  setLanBlocked(b: boolean): void;
  presence(id: string, online: boolean, rtt: number | null): void;
  addPush(p: IncomingPush): void;
  removePush(pushId: string): void;
  setPushSheet(open: boolean): void;
  openAddDevice(tab?: AddDevice["tab"], text?: string): void;
  closeAddDevice(): void;
  openContact(id: string): void;
  closeContact(): void;
  setHoverContact(id: string | null): void;
  /** send_to_contact + comet + jump to Chuyển file. Resolves false (after a toast) on failure. */
  sendToContact(id: string, paths: string[], origin?: { x: number; y: number }): Promise<boolean>;
  /** `invite` deep link / pasted invite: open "Thêm máy" on the paste tab with the confirm */
  receiveInvite(invite: string): void;

  setScreen(s: Screen): void;
  /** Settings → Kết nối, scrolled into view */
  openDiagnostics(): void;
  setApp(a: AppState): void;
  patchApp(p: Partial<AppState>): void;
  openTicket(text: string): Promise<boolean>;
  /** deep-link entry: honours app.auto_receive */
  receiveLink(text: string): Promise<void>;
  closeSheet(): void;
  openShare(path: string): void;
  closeShare(): void;
  askShare(path: string | null): void;
  bumpShares(): void;
  toast(text: string, kind?: "info" | "error"): void;
  refreshTransfers(): Promise<void>;
  applyEvent(ev: TransferEvent | TransferEventV05): void;
  setDropHover(on: boolean): void;
  setHostStatus(h: HostStatus | null): void;
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

/** parse_ticket + remember in "Gần đây". Toasts and returns null on a bad link. */
async function parseAndRemember(text: string): Promise<TicketInfo | null> {
  try {
    const info = await api.parse_ticket({ text });
    if (!info.admin) {
      const rec: Recent = { ticket: info.ticket, job: info.job, get: info.get, put: info.put, admin: info.admin, exp: info.exp, at: Date.now() };
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

export const useStore = create<Store>((set, get) => ({
  screen: "home",
  app: null,
  transfers: {},
  sheet: null,
  sheetNote: null,
  share: null,
  shareAsk: null,
  sharesRev: 0,
  recents: loadRecents(),
  toasts: [],
  hostStatus: null,
  dropHover: false,
  flights: [],
  navPulse: 0,
  diagFocus: 0,
  contacts: [],
  contactsLoaded: false,
  pushes: [],
  pushSheet: false,
  lanBlocked: false,
  addDevice: null,
  contactSheet: null,
  hoverContact: null,

  async refreshContacts() {
    try {
      const contacts = await api.list_contacts();
      set({ contacts, contactsLoaded: true });
    } catch {
      // older core / not supported yet: an empty book is the right fallback
      set({ contactsLoaded: true });
    }
  },
  async refreshPushes() {
    try {
      const pushes = await api.list_incoming_pushes();
      set((s) => ({ pushes, pushSheet: pushes.length > 0 && (s.pushSheet || pushes.some((p) => !s.pushes.some((o) => o.push_id === p.push_id))) }));
    } catch { /* older core */ }
  },
  setLanBlocked: (lanBlocked) => set((s) => (s.lanBlocked === lanBlocked ? s : { lanBlocked })),
  presence(id, online, rtt) {
    set((s) => ({
      contacts: s.contacts.map((c) => c.id !== id ? c : {
        ...c, online, rtt_ms: online ? rtt : null,
        last_seen: online || !c.online ? c.last_seen : Math.floor(Date.now() / 1000),
      }),
    }));
  },
  addPush(p) {
    set((s) => (s.pushes.some((x) => x.push_id === p.push_id) ? s : { pushes: [...s.pushes, p], pushSheet: true, addDevice: null, contactSheet: null }));
  },
  removePush: (pushId) => set((s) => {
    const pushes = s.pushes.filter((p) => p.push_id !== pushId);
    return { pushes, pushSheet: pushes.length > 0 && s.pushSheet };
  }),
  setPushSheet: (pushSheet) => set({ pushSheet }),
  openAddDevice: (tab = "invite", text = "") => set({ addDevice: { tab, text }, contactSheet: null, sheet: null, sheetNote: null, screen: "home" }),
  closeAddDevice: () => set({ addDevice: null }),
  openContact: (id) => set({ contactSheet: id }),
  closeContact: () => set({ contactSheet: null }),
  setHoverContact: (hoverContact) => set((s) => (s.hoverContact === hoverContact ? s : { hoverContact })),
  receiveInvite: (invite) => set({ addDevice: { tab: "enter", text: invite }, contactSheet: null, sheet: null, sheetNote: null, share: null, shareAsk: null, screen: "home" }),

  async sendToContact(id, paths, origin) {
    const c = get().contacts.find((x) => x.id === id);
    try {
      await api.send_to_contact({ id, paths });
      await get().refreshTransfers();
      get().fly(origin);
      set({ screen: "transfers", contactSheet: null });
      get().toast(c && !c.online ? `${c.name} đang offline, sẽ gửi khi máy online` : `Đang gửi cho ${c?.name ?? "máy kia"}…`);
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
  setHostStatus: (hostStatus) => set({ hostStatus }),
  fly(from) {
    const p = from ?? { x: window.innerWidth / 2, y: window.innerHeight * 0.32 };
    set((s) => ({ flights: [...s.flights, { id: flightSeq++, ...p }].slice(-4) }));
  },
  land: (id) => set((s) => ({ flights: s.flights.filter((f) => f.id !== id), navPulse: s.navPulse + 1 })),

  async openTicket(text) {
    const info = await parseAndRemember(text);
    if (!info) return false;
    set({ sheet: info, sheetNote: null, share: null, shareAsk: null, screen: "home" });
    return true;
  },

  async receiveLink(text) {
    const info = await parseAndRemember(text);
    if (!info) return;
    const exp = info.exp && info.exp <= Date.now() / 1000;
    if (!get().app?.auto_receive || !info.get || info.admin || exp) {
      set({ sheet: info, sheetNote: null, share: null, shareAsk: null, screen: "home" });
      return;
    }
    try {
      await api.start_get({ ticket: info.ticket });
      await get().refreshTransfers();
      get().fly();
      set({ sheet: null, sheetNote: null, share: null, shareAsk: null, screen: "transfers" });
      get().toast(`Đang nhận ${info.job}…`);
    } catch (e) {
      const msg = errorMessage(e);
      if (EXISTS_RE.test(msg)) {
        set({ share: null, shareAsk: null, sheet: info, sheetNote: { kind: "exists", text: "Job này đã nhận xong trước đó. Bấm Tải lại nếu muốn nhận lại." }, screen: "home" });
      } else {
        // unknown rejection: show the sheet so the user can retry by hand, and say why
        set({ share: null, shareAsk: null, sheet: info, sheetNote: { kind: "error", text: msg }, screen: "home" });
      }
    }
  },
  closeSheet: () => set({ sheet: null, sheetNote: null }),
  openShare: (path) => set({ share: { path, nonce: Date.now() }, shareAsk: null, sheet: null, sheetNote: null }),
  closeShare: () => set({ share: null, screen: "home" }),
  askShare: (shareAsk) => set({ shareAsk }),
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
    // v0.5 core events also come through here; they have their own listeners (contacts:changed …)
    if (ev.type === "contacts_changed" || ev.type === "contact_presence" || ev.type === "push_incoming" || ev.type === "lan_blocked") return;
    if (ev.type === "host_status") {
      const { type: _t, ...rest } = ev;
      void _t;
      set({ hostStatus: rest });
      return;
    }
    const cur = get().transfers;
    const t = cur[ev.id];
    let next: TransferState | undefined = t;
    switch (ev.type) {
      case "started":
        next = {
          id: ev.id, job: ev.job, op: ev.op, side: ev.side, peer: ev.peer, status: "connecting",
          bytes_done: 0, bytes_total: 0, files_done: 0, files_total: 0, speed_bps: 0, eta_s: null,
          path: "unknown", rtt_ms: null, streams: 0, dest: null, error: null, started_at: Math.floor(Date.now() / 1000),
          contact: ev.contact ?? null,
        };
        break;
      case "manifest":
        if (t) next = { ...t, status: "running", bytes_total: ev.bytes, files_total: ev.files };
        break;
      case "progress":
        if (t) next = {
          ...t, status: t.status === "paused" ? "paused" : "running", bytes_done: ev.bytes_done, bytes_total: ev.bytes_total,
          files_done: ev.files_done, files_total: ev.files_total, speed_bps: ev.speed_bps, eta_s: ev.eta_s, path: ev.path, rtt_ms: ev.rtt_ms ?? null, streams: ev.streams ?? 0, error: null,
        };
        break;
      case "done":
        if (t) next = { ...t, status: "done", bytes_done: ev.bytes, bytes_total: ev.bytes, files_done: ev.files, files_total: ev.files, speed_bps: 0, eta_s: 0, dest: ev.dest ?? t.dest, error: null };
        break;
      case "error":
        if (t) {
          if (ev.code === "cancelled") next = { ...t, status: "cancelled", speed_bps: 0, eta_s: null, error: null };
          else if (ev.retry_in_s != null) next = { ...t, status: "connecting", speed_bps: 0, eta_s: null, error: retryText(ev.code, ev.retry_in_s) };
          else next = { ...t, status: "error", speed_bps: 0, eta_s: null, error: errorText(ev.code, ev.msg) };
        }
        break;
      case "paused":
        if (t) next = { ...t, status: "paused", speed_bps: 0, eta_s: null };
        break;
    }
    if (!next) {
      // event for a transfer we have not seen (e.g. host-side job): resync
      void get().refreshTransfers();
      return;
    }
    set({ transfers: { ...cur, [ev.id]: next } });
  },
}));

export function retryText(code: string, secs: number): string {
  if (code === "busy") return `Máy tổng đang bận, thử lại sau ${secs} giây`;
  if (code === "host_offline") return `Máy tổng đang offline, thử lại sau ${secs} giây`;
  return `Lỗi, thử lại sau ${secs} giây`;
}

export function errorText(code: string, msg: string): string {
  switch (code) {
    case "denied": return "Link không có quyền truy cập";
    case "expired": return "Link đã hết hạn";
    case "revoked": return "Link đã bị thu hồi";
    case "host_offline": return "Máy tổng đang offline";
    case "busy": return "Máy tổng đang bận";
    case "hash_mismatch": return "File bị lỗi khi truyền (sai mã kiểm tra)";
    case "io": return `Lỗi ghi/đọc file: ${msg}`;
    default: return msg || "Có lỗi xảy ra";
  }
}
