// IPC contract between the React UI and the Tauri (Rust) shell.
// Rust side: apps/desktop/src-tauri/src/commands.rs implements every command
// with exactly these names/arg shapes; events are emitted on the main window.
// UI side: apps/desktop/src/lib/ipc.ts wraps invoke()/listen() with these types
// and also provides a mock implementation (VITE_MOCK=1) that drives the UI
// without a Rust backend.

export type Op = "get" | "put";
export type Side = "client" | "host";
export type ConnPath = "direct" | "relay" | "mixed" | "unknown";
export type Status = "connecting" | "running" | "paused" | "done" | "error" | "cancelled";
export type ErrorCode = "denied" | "expired" | "revoked" | "host_offline" | "busy" | "io" | "hash_mismatch" | "cancelled" | "other";

export interface AppState {
  version: string;
  endpoint_id: string;          // base32 iroh EndpointId
  device_name: string;
  dest_dir: string;             // where "get" lands, default ~/Toss
  host: HostConfig | null;      // null = host mode off
  page_host: string;            // e.g. "toss-link.vercel.app" (links use https://<page_host>/#<ticket>)
  autostart: boolean;
  first_run: boolean;
  context_menu: boolean;        // "Tạo link Toss" registered in the OS right-click menu (v0.3)
  auto_receive: boolean;        // v0.3: a clicked link with `get` starts downloading immediately (default true)           // true until setup finished
}

export interface Root { alias: string; path: string }
export interface HostConfig { roots: Root[]; return_root: string; max_parallel_jobs: number; bind_port?: number | null }  // bind_port: fixed UDP port for port-forwarding, null/absent = random (CHANGES.md)

export interface TicketInfo {        // decoded, never verified on the client
  ticket: string;                    // "toss1..."
  id: string;                        // hex
  host: string;                      // endpoint id base32
  job: string; root: string; path: string; ret: string;
  get: boolean; put: boolean; admin: boolean;
  exp: number;                       // unix s, 0 = never
  web_url: string; deep_link: string;
}

export interface TicketSpec { job: string; root: string; path: string; ret: string; get: boolean; put: boolean; days: number }

export interface TransferState {
  id: string; job: string; op: Op; side: Side; peer: string;
  status: Status; bytes_done: number; bytes_total: number; files_done: number; files_total: number;
  speed_bps: number; eta_s: number | null; path: ConnPath; dest: string | null; error: string | null;
  started_at: number;
}

export interface JobRecord {
  id: string; job: string; root: string; path: string; get: boolean; put: boolean;
  exp: number; created: number; revoked: boolean;
  last: { at: number; device: string; op: Op; files: number; bytes: number; ok: boolean; msg: string } | null;
  ticket: string;                     // re-signed string for display/copy (host side only)
}

export interface DirEntry { name: string; is_dir: boolean; size: number; mtime: number }

export interface HostStatus { device: string; version: string; roots: string[]; active: number; queued: number; uptime_s: number }

// ---- commands (invoke names) -------------------------------------------
export interface Commands {
  get_state(): Promise<AppState>;
  finish_setup(args: { device_name: string; dest_dir: string }): Promise<AppState>;
  set_device_name(args: { name: string }): Promise<void>;
  set_dest_dir(args: { path: string }): Promise<void>;
  set_autostart(args: { on: boolean }): Promise<void>;
  // v0.3 quick share (right-click → "Tạo link Toss"). Any machine can share: the folder becomes a
  // quick-share root (alias "qs-<8hex>", persisted), host mode is turned on if it was off (return root
  // defaults to <dest_dir>/Tra-ve), and a get ticket (job = folder name, `days`, optional put) is created
  // and COPIED TO THE CLIPBOARD by the shell. The UI shows the result (link + QR) in the Share view.
  share_folder(args: { path: string; days: number; put: boolean }): Promise<TicketInfo>;
  list_quick_shares(): Promise<{ alias: string; path: string; job_id: string | null }[]>;
  remove_quick_share(args: { alias: string }): Promise<void>;    // revokes its ticket(s) and drops the root
  set_context_menu(args: { on: boolean }): Promise<void>;        // Windows: HKCU\Software\Classes\Directory\shell\Toss (+ Background); macOS: ~/Library/Services/Tạo link Toss.workflow
  set_auto_receive(args: { on: boolean }): Promise<void>;
  pick_folder(args: { title: string }): Promise<string | null>;   // native dialog
  open_path(args: { path: string }): Promise<void>;                // reveal in Finder/Explorer

  // host mode
  set_host(args: { config: HostConfig | null }): Promise<void>;
  host_status(): Promise<HostStatus | null>;
  list_dir(args: { root: string; path: string }): Promise<DirEntry[]>;       // local host
  create_ticket(args: { spec: TicketSpec }): Promise<TicketInfo>;            // local host
  list_jobs(): Promise<JobRecord[]>;                                         // local host
  revoke(args: { id: string }): Promise<void>;
  create_admin_ticket(args: { days: number }): Promise<TicketInfo>;

  // tickets / transfers (client)
  parse_ticket(args: { text: string }): Promise<TicketInfo>;                 // throws on bad input
  start_get(args: { ticket: string }): Promise<string>;                      // returns transfer id
  start_put(args: { ticket: string; paths: string[] }): Promise<string>;
  list_transfers(): Promise<TransferState[]>;
  pause(args: { id: string }): Promise<void>;
  resume(args: { id: string }): Promise<void>;
  cancel(args: { id: string }): Promise<void>;
  clear_finished(): Promise<void>;

  // remote host control via admin ticket (stored in app settings, at most one)
  set_admin_ticket(args: { ticket: string | null }): Promise<void>;
  remote_status(): Promise<HostStatus>;
  remote_list_dir(args: { root: string; path: string }): Promise<DirEntry[]>;
  remote_create_ticket(args: { spec: TicketSpec }): Promise<TicketInfo>;
  remote_list_jobs(): Promise<JobRecord[]>;
  remote_revoke(args: { id: string }): Promise<void>;

  // misc
  qr_svg(args: { text: string }): Promise<string>;                           // SVG markup
  copy_text(args: { text: string }): Promise<void>;
}

// ---- events (listen names) ---------------------------------------------
export type Events = {
  "transfer:event": TransferEvent;        // every core event, see below
  "deep-link": { ticket: string };        // app opened via toss://t/<ticket> or file drop of a .toss file
  "drop": { paths: string[] };            // OS drag & drop onto the window
  "share": { path: string };               // v0.3: app launched / forwarded with `--share <folder>` (right-click menu) → UI opens the Share view and calls share_folder
  "host:status": HostStatus;
};

export type TransferEvent =
  | { type: "started"; id: string; job: string; op: Op; side: Side; peer: string }
  | { type: "manifest"; id: string; files: number; bytes: number; to_do_files: number; to_do_bytes: number }
  | { type: "progress"; id: string; bytes_done: number; bytes_total: number; files_done: number; files_total: number; speed_bps: number; eta_s: number | null; path: ConnPath }
  | { type: "done"; id: string; files: number; bytes: number; elapsed_s: number; dest: string | null }
  | { type: "error"; id: string; code: ErrorCode; msg: string; retry_in_s: number | null }
  | { type: "paused"; id: string }
  | { type: "host_status"; device: string; version: string; roots: string[]; active: number; queued: number; uptime_s: number };

// ---- UI behaviour the shell relies on ------------------------------------
// - v0.3: On "deep-link" with ticket.get and auto_receive=true → call start_get at once, show the window,
//   switch to the Chuyển file screen (the transfer card is the "popup"); a ticket whose job folder is already
//   complete shows the sheet instead (Mở folder / Tải lại). auto_receive=false → old behaviour below.
// - On "deep-link": if ticket.get → show a confirm sheet (job name, host, size unknown)
//   with one button "Nhận" → start_get. If only ticket.put → open the drop target for that job.
// - On "drop" while a put-capable ticket is selected → start_put.
// - Window: 400 x 600, not resizable below 360 x 480, hides to tray on close.
// - Host mode machines: window may stay hidden; tray menu = Mở Toss · Trạng thái · Thoát.
