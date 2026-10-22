// UI-only additions on top of the frozen contract (ipc-types.ts is a verbatim copy of contracts/ipc.ts).
// The Rust shell implements these too; they are additive and optional for any other shell.

/** Extra commands: the UI calls `ui_ready` once its listeners are registered so
 *  the shell can flush deep-links that arrived during startup. */
export interface ExtraCommands {
  ui_ready(): Promise<void>;
  /** True when an admin ticket (remote host) is stored in settings. Cheap: never dials the host. */
  has_admin_ticket(): Promise<boolean>;
}

/** Extra events. */
export type ExtraEvents = {
  /** tray menu "Trạng thái" asks the UI to switch screen. */
  navigate: { screen: "home" | "transfers" | "settings" };
  /** OS drag is hovering over the window (true) or left it (false). */
  "drop:hover": { on: boolean };
};
