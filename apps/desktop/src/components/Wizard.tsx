// v0.8 first run: ONE screen — name, receive folder, "Mở cùng máy" — and go. Firewall / Local Network are handled
// afterwards by banners on Home, only when they are actually needed.
import { useState } from "react";
import { motion, useReducedMotion } from "motion/react";
import { api, errorMessage } from "../lib/ipc";
import { useStore } from "../lib/store";
import { EASE, Icon, Nest, Row, Toggle, Words } from "./ui";
import { TossMark } from "./TossMark";
import { HowItWorks } from "./HowItWorks";

export function Wizard() {
  const app = useStore((s) => s.app)!;
  const setApp = useStore((s) => s.setApp);
  const toast = useStore((s) => s.toast);
  const [name, setName] = useState(app.device_name);
  const [dest, setDest] = useState(app.dest_dir);
  const [autostart, setAutostart] = useState(true);
  const [busy, setBusy] = useState(false);
  const reduce = useReducedMotion();

  async function finish() {
    setBusy(true);
    try {
      let next = await api.finish_setup({ device_name: name.trim() || app.device_name, dest_dir: dest });
      if (next.autostart !== autostart) { await api.set_autostart({ on: autostart }); next = { ...next, autostart }; }
      // macOS asks for "Mạng cục bộ" the first time Toss looks for devices: better now than in the middle of a transfer
      api.request_local_network().catch(() => undefined);
      setApp(next);
    } catch (e) { toast(errorMessage(e), "error"); setBusy(false); }
  }

  return (
    <div className="fixed inset-0 z-40 bg-bg flex flex-col p-6">
      <div className="aura" /><div className="grain" />
      <motion.div
        className="relative z-10 flex flex-col h-full overflow-y-auto px-1 -mx-1"
        initial={reduce ? false : { opacity: 0, y: 16, filter: "blur(8px)" }} animate={{ opacity: 1, y: 0, filter: "blur(0px)" }} transition={{ duration: 0.6, ease: EASE }}
      >
        <TossMark className="w-24 -ml-1 -mb-1 shrink-0" />
        <Words text="Chào mừng đến Toss" accent={["Toss"]} className="text-[30px] mb-1" />
        <p className="text-sm text-mute mb-4">Đặt tên máy, chọn nơi nhận file rồi bắt đầu. Có gì cần chỉnh thêm, Toss sẽ nhắc sau.</p>

        <div className="space-y-3 flex-1">
          <div>
            <label className="label" htmlFor="wz-name">Tên máy (máy khác thấy tên này)</label>
            <input id="wz-name" className="input h-11 text-base" autoFocus value={name} maxLength={64} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === "Enter" && name.trim() && void finish()} placeholder="Vd. Minh – MacBook" />
          </div>
          <div>
            <div className="label">Thư mục nhận file</div>
            <div className="card p-3 flex items-center gap-2">
              <Icon name="folder" className="w-5 h-5 text-accent shrink-0" />
              <div className="mono text-xs break-all flex-1 selectable">{dest}</div>
              <button className="btn" onClick={async () => { try { const p = await api.pick_folder({ title: "Chọn thư mục nhận file" }); if (p) setDest(p); } catch (e) { toast(errorMessage(e), "error"); } }}>Đổi…</button>
            </div>
          </div>
          <div className="card px-3.5 py-1">
            <Row title="Mở cùng máy" hint="Chạy ẩn khi bật máy để luôn nhận được file">
              <Toggle on={autostart} label="Mở cùng máy" onChange={setAutostart} />
            </Row>
          </div>
          <HowItWorks variant="line" />
        </div>

        <button className="btn btn-primary btn-lg w-full magnet mt-4 shrink-0" disabled={busy || !name.trim()} onClick={() => void finish()}>
          {busy ? "Đang chuẩn bị…" : "Bắt đầu"}<Nest />
        </button>
      </motion.div>
    </div>
  );
}
