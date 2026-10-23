// Single entry point for UI <-> shell calls.
//  - real mode: @tauri-apps/api invoke()/listen()
//  - mock mode (VITE_MOCK=1, or no Tauri runtime present): in-memory simulation (./mock)
import type { Commands, CommandsV05, CommandsV051, Events, EventsV05, EventsV051 } from "./ipc-types";
import type { ExtraCommands, ExtraEvents } from "./ipc-extra";

export type AllCommands = Commands & CommandsV05 & CommandsV051 & ExtraCommands;
export type AllEvents = Events & EventsV05 & EventsV051 & ExtraEvents;

const hasTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
export const isMock: boolean = import.meta.env.VITE_MOCK === "1" || !hasTauri;

const COMMAND_NAMES: (keyof AllCommands)[] = [
  "get_state", "finish_setup", "set_device_name", "set_dest_dir", "set_autostart", "pick_folder", "open_path",
  "set_host", "host_status", "list_dir", "create_ticket", "list_jobs", "revoke", "create_admin_ticket",
  "parse_ticket", "start_get", "start_put", "list_transfers", "pause", "resume", "cancel", "clear_finished",
  "set_admin_ticket", "remote_status", "remote_list_dir", "remote_create_ticket", "remote_list_jobs", "remote_revoke",
  "qr_svg", "copy_text", "ui_ready", "has_admin_ticket",
  "share_folder", "list_quick_shares", "remove_quick_share", "set_context_menu", "set_auto_receive",
  "get_diagnostics", "add_firewall_rule", "test_connect", "set_net", "export_logs",
  "list_contacts", "create_invite", "revoke_invites", "accept_invite", "rename_contact", "set_contact_auto_accept",
  "remove_contact", "send_to_contact", "list_incoming_pushes", "answer_push", "open_local_network_settings", "parse_invite", "expand_link",
  "check_update", "install_update", "set_auto_update", "remote_update",
];

function errText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}

/** Normalised error thrown by every api call (Rust errors arrive as plain strings). */
export class IpcError extends Error {}

function makeReal(): AllCommands {
  const out: Record<string, unknown> = {};
  for (const name of COMMAND_NAMES) {
    out[name] = async (args?: Record<string, unknown>) => {
      const { invoke } = await import("@tauri-apps/api/core");
      try {
        return await invoke(name, args ?? {});
      } catch (e) {
        throw new IpcError(errText(e));
      }
    };
  }
  return out as unknown as AllCommands;
}

let mockModule: Promise<typeof import("./mock")> | null = null;
const loadMock = () => (mockModule ??= import("./mock"));

function makeMock(): AllCommands {
  const out: Record<string, unknown> = {};
  for (const name of COMMAND_NAMES) {
    out[name] = async (args?: unknown) => {
      const m = await loadMock();
      try {
        return await (m.mockApi as unknown as Record<string, (a?: unknown) => Promise<unknown>>)[name](args);
      } catch (e) {
        throw new IpcError(errText(e));
      }
    };
  }
  return out as unknown as AllCommands;
}

export const api: AllCommands = isMock ? makeMock() : makeReal();

/** Subscribe to a shell event. Returns an unsubscribe function (async because the real listen() is). */
export async function on<K extends keyof AllEvents>(name: K, handler: (payload: AllEvents[K]) => void): Promise<() => void> {
  if (isMock) {
    const m = await loadMock();
    return m.mockOn(name, handler as (p: unknown) => void);
  }
  const { listen } = await import("@tauri-apps/api/event");
  return listen<AllEvents[K]>(name, (e) => handler(e.payload));
}

export const errorMessage = errText;
