// v0.5.1 in-app updates: UI state shared by Settings → Cập nhật and the Home banner.
import { create } from "zustand";
import type { UpdateInfo, UpdatePhase } from "@contracts/ipc";
import { api, errorMessage, on } from "./ipc";

export type CheckState = "idle" | "checking" | "done" | "error";

interface UpdateStore {
  /** last answer of the feed (check_update or update:available) */
  info: UpdateInfo | null;
  check: CheckState;
  checkError: string | null;
  /** an install is running (button, banner, background or remote request) */
  phase: UpdatePhase | null;
  /** transfers still running while phase = "waiting" */
  active: number;
  downloaded: number;
  total: number | null;
  error: string | null;
  /** banner closed for this session (per version) */
  dismissed: string | null;

  runCheck(): Promise<void>;
  install(): Promise<void>;
  dismiss(): void;
}

export const useUpdate = create<UpdateStore>((set, get) => ({
  info: null,
  check: "idle",
  checkError: null,
  phase: null,
  active: 0,
  downloaded: 0,
  total: null,
  error: null,
  dismissed: null,

  async runCheck() {
    set({ check: "checking", checkError: null });
    try {
      const info = await api.check_update();
      set({ info, check: "done" });
    } catch (e) {
      set({ check: "error", checkError: errorMessage(e) });
    }
  },
  async install() {
    if (get().phase) return;
    set({ phase: "waiting", active: 0, downloaded: 0, total: null, error: null });
    try {
      // resolves only if the app did not restart (it normally does)
      await api.install_update();
    } catch (e) {
      set({ phase: null, error: errorMessage(e) });
    }
  },
  dismiss: () => set((s) => ({ dismissed: s.info?.version ?? null })),
}));

/** Shell events → store. Returns the unsubscribe functions. */
export function listenUpdates(): Promise<() => void>[] {
  return [
    on("update:available", (info) => useUpdate.setState({ info, check: "done", checkError: null })),
    on("update:phase", ({ phase, active }) => useUpdate.setState({ phase, active, error: null })),
    on("update:progress", ({ downloaded, total }) => useUpdate.setState((s) => ({ downloaded, total, phase: s.phase ?? "downloading" }))),
    on("update:error", ({ msg }) => useUpdate.setState({ phase: null, error: msg })),
  ];
}

/** Release notes without the "#force" marker. */
export function cleanNotes(notes: string | null | undefined): string {
  return (notes ?? "").replace(/#force\b/gi, "").replace(/\n{3,}/g, "\n\n").trim();
}
