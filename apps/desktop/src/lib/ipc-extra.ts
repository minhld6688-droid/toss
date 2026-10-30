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
};
