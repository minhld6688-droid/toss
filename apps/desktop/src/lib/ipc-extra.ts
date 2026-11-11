// UI-only additions on top of the frozen contract (the frozen contract itself lives in contracts/ipc.ts, imported as @contracts/ipc).
// The Rust shell implements these too; they are additive and optional for any other shell.

/** Extra commands: the UI calls `ui_ready` once its listeners are registered so
 *  the shell can flush deep-links that arrived during startup. */
export interface ExtraCommands {
  ui_ready(): Promise<void>;
  /** True when an admin ticket (remote host) is stored in settings. Cheap: never dials the host. */
  has_admin_ticket(): Promise<boolean>;
  /** v0.5: decode an invite (tossinv1…, toss://i/…, web link) without pairing, for the "Kết nối với <name>?" confirm. */
  parse_invite(args: { text: string }): Promise<import("@contracts/ipc").InviteInfo>;
  /** v0.5.2: short link (https://host/<9 chars>) → the toss1… / tossinv1… it stands for; other text unchanged. */
  expand_link(args: { text: string }): Promise<string>;
  /** 0.9 studio machine: the Links screen. */
  studio_view(): Promise<import("@contracts/ipc").StudioView>;
  studio_set(args: { on: boolean; root: string; days: number }): Promise<import("@contracts/ipc").StudioView>;
  /** Look at the jobs folder now (otherwise every 30 s). */
  studio_scan(): Promise<import("@contracts/ipc").StudioView>;
  /** Link for a folder (also copied to the clipboard). */
  studio_link_folder(args: { path: string }): Promise<import("@contracts/ipc").StudioLink>;
  /** The editor's standing backup link (put-only, never expires). */
  studio_backup_link(args: { editor: string }): Promise<import("@contracts/ipc").StudioLink>;
  /** Revoke a link and drop it from the list. */
  studio_remove(args: { id: string }): Promise<import("@contracts/ipc").StudioView>;
  /** 0.9: delete the local copies a backup receipt covers (unchanged files only). */
  free_up(args: { id: string }): Promise<import("@contracts/ipc").Freed>;
}

/** Extra events. */
export type ExtraEvents = {
  /** tray menu "Đang chuyển" asks the UI to switch screen. */
  navigate: { screen: "home" | "transfers" | "settings" };
  /** OS drag is hovering over the window (true) or left it (false). */
  "drop:hover": { on: boolean; x?: number; y?: number };
  /** v0.5: the drag moved; window-relative CSS px, so the UI can highlight the contact row under it. */
  "drop:over": { x: number; y: number };
  /** the drop itself also carries the position (x, y) */
  "drop": { paths: string[]; x?: number; y?: number };
  /** the shell could not start a right-click share (bad path, no access…): show `msg` as a toast. */
  "share:error": { msg: string };
  /** v0.6: files dropped on the menu bar / Dock icon → "Gửi tới ai?" */
  "send": { paths: string[] };
  /** v0.6: tray "Gửi folder…" → pick a folder, then "Gửi tới ai?" */
  "send:pick": Record<string, never>;
  /** v0.6 (macOS): a drag entered the menu bar icon; the window popped up — show the contact book */
  "tray:drag": Record<string, never>;
  /** v0.6: tray "Kiểm tra cập nhật" */
  "update:check": Record<string, never>;
  /** 0.9: the studio's link list changed (new shooter folder, right-click link). */
  "studio:links": Record<string, never>;
};
