import { create } from "zustand";
import type { AppState, HostStatus, TicketInfo, TransferEvent, TransferState } from "./ipc-types";
import { api, errorMessage } from "./ipc";

export type Screen = "home" | "transfers" | "settings";

export interface Recent {
  ticket: string; job: string; get: boolean; put: boolean; admin: boolean; exp: number; at: number;
}

interface Toast { id: number; text: string; kind: "info" | "error" }

interface Store {
  screen: Screen;
  app: AppState | null;
  transfers: Record<string, TransferState>;
  /** ticket currently shown in the confirm sheet */
  sheet: TicketInfo | null;
  recents: Recent[];
  toasts: Toast[];
  hostStatus: HostStatus | null;
  dropHover: boolean;

  setScreen(s: Screen): void;
  setApp(a: AppState): void;
  patchApp(p: Partial<AppState>): void;
  openTicket(text: string): Promise<boolean>;
  closeSheet(): void;
  toast(text: string, kind?: "info" | "error"): void;
  refreshTransfers(): Promise<void>;
  applyEvent(ev: TransferEvent): void;
  setDropHover(on: boolean): void;
  setHostStatus(h: HostStatus | null): void;
}

const RECENT_KEY = "toss.recents.v1";
function loadRecents(): Recent[] {
  try { return JSON.parse(localStorage.getItem(RECENT_KEY) || "[]"); } catch { return []; }
}
function saveRecents(r: Recent[]) {
  try { localStorage.setItem(RECENT_KEY, JSON.stringify(r)); } catch { /* storage may be unavailable */ }
}

let toastSeq = 1;

export const useStore = create<Store>((set, get) => ({
  screen: "home",
  app: null,
  transfers: {},
  sheet: null,
  recents: loadRecents(),
  toasts: [],
  hostStatus: null,
  dropHover: false,

  setScreen: (screen) => set({ screen }),
  setApp: (app) => set({ app }),
  patchApp: (p) => set((s) => (s.app ? { app: { ...s.app, ...p } } : s)),
  setDropHover: (dropHover) => set({ dropHover }),
  setHostStatus: (hostStatus) => set({ hostStatus }),

  async openTicket(text) {
    try {
      const info = await api.parse_ticket({ text });
      const rec: Recent = { ticket: info.ticket, job: info.job, get: info.get, put: info.put, admin: info.admin, exp: info.exp, at: Date.now() };
      if (!info.admin) {
        const recents = [rec, ...get().recents.filter((r) => r.ticket !== info.ticket)].slice(0, 12);
        saveRecents(recents);
        set({ recents });
      }
      set({ sheet: info, screen: "home" });
      return true;
    } catch (e) {
      get().toast(`Link không hợp lệ: ${errorMessage(e)}`, "error");
      return false;
    }
  },
  closeSheet: () => set({ sheet: null }),

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
          path: "unknown", dest: null, error: null, started_at: Math.floor(Date.now() / 1000),
        };
        break;
      case "manifest":
        if (t) next = { ...t, status: "running", bytes_total: ev.bytes, files_total: ev.files };
        break;
      case "progress":
        if (t) next = {
          ...t, status: t.status === "paused" ? "paused" : "running", bytes_done: ev.bytes_done, bytes_total: ev.bytes_total,
          files_done: ev.files_done, files_total: ev.files_total, speed_bps: ev.speed_bps, eta_s: ev.eta_s, path: ev.path, error: null,
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
