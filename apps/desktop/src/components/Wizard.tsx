import { useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { api, errorMessage } from "../lib/ipc";
import { useStore } from "../lib/store";
import { EASE, Icon, Nest, Words } from "./ui";
import { TossMark } from "./TossMark";

export function Wizard() {
  const app = useStore((s) => s.app)!;
  const setApp = useStore((s) => s.setApp);
  const toast = useStore((s) => s.toast);
  const [step, setStep] = useState(0);
  const [name, setName] = useState(app.device_name);
  const [dest, setDest] = useState(app.dest_dir);
  const [busy, setBusy] = useState(false);
  const reduce = useReducedMotion();
  const prev = useRef(0);
  const dir = step >= prev.current ? 1 : -1;
  prev.current = step;

  async function finish() {
    setBusy(true);
    try { setApp(await api.finish_setup({ device_name: name.trim() || app.device_name, dest_dir: dest })); }
    catch (e) { toast(errorMessage(e), "error"); setBusy(false); }
  }

  return (
    <div className="fixed inset-0 z-40 bg-bg flex flex-col p-6">
      <div className="aura" /><div className="grain" />
      <div className="relative z-10 flex flex-col h-full">
      <div className="flex gap-1.5 mb-8">
        {[0, 1, 2].map((i) => (
          <div key={i} className="h-1 flex-1 rounded-full bg-line/15 overflow-hidden">
            <motion.div
              className="h-full rounded-full bg-accent origin-left" initial={false}
              animate={{ scaleX: i <= step ? 1 : 0 }} transition={{ duration: 0.7, ease: EASE, delay: i === step ? 0.15 : 0 }}
              style={{ boxShadow: "0 0 10px rgb(var(--accent) / 0.7)" }}
            />
          </div>
        ))}
      </div>

      <AnimatePresence mode="wait" initial={false} custom={dir}>
      <motion.div
        key={step} custom={dir} className="flex-1 flex flex-col"
        variants={{
          enter: (d: number) => (reduce ? { opacity: 0 } : { opacity: 0, x: 40 * d, filter: "blur(8px)" }),
          center: { opacity: 1, x: 0, filter: "blur(0px)" },
          exit: (d: number) => (reduce ? { opacity: 0 } : { opacity: 0, x: -30 * d, filter: "blur(6px)" }),
        }}
        initial="enter" animate="center" exit="exit" transition={{ duration: 0.45, ease: EASE }}
      >
        {step === 0 && (
          <>
            <TossMark className="w-32 -ml-2 mb-2" />
            <div className="text-xs text-mute mb-1">Bước 1/3</div>
            <Words text="Tên máy của bạn" accent={["máy"]} className="text-[30px] mb-2" />
            <p className="text-sm text-mute mb-4">Tên này hiện ở phía máy tổng khi bạn nhận hoặc trả file.</p>
            <input className="input h-12 text-base" autoFocus value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === "Enter" && name.trim() && setStep(1)} />
          </>
        )}
        {step === 1 && (
          <>
            <div className="text-xs text-mute mb-1">Bước 2/3</div>
            <Words text="Thư mục nhận file" accent={["nhận"]} className="text-[30px] mb-2" />
            <p className="text-sm text-mute mb-4">File nhận về sẽ nằm trong thư mục này, mỗi job một folder riêng.</p>
            <div className="card p-3 flex items-center gap-2">
              <Icon name="folder" className="w-5 h-5 text-accent shrink-0" />
              <div className="mono text-xs break-all flex-1 selectable">{dest}</div>
              <button className="btn" onClick={async () => { try { const p = await api.pick_folder({ title: "Chọn thư mục nhận file" }); if (p) setDest(p); } catch (e) { toast(errorMessage(e), "error"); } }}>Đổi…</button>
            </div>
          </>
        )}
        {step === 2 && (
          <>
            <div className="text-xs text-mute mb-1">Bước 3/3</div>
            <motion.div
              className="w-14 h-14 rounded-full bg-ok/15 text-ok grid place-items-center mb-3" style={{ boxShadow: "0 0 0 8px rgb(var(--ok) / 0.06)" }}
              initial={reduce ? false : { scale: 0, rotate: -60 }} animate={{ scale: 1, rotate: 0 }} transition={{ type: "spring", stiffness: 420, damping: 14, delay: 0.2 }}
            ><Icon name="check" className="w-7 h-7" weight="bold" /></motion.div>
            <Words text="Xong rồi" accent={["rồi"]} className="text-[30px] mb-2" />
            <p className="text-sm text-mute">Dán link từ máy tổng vào Trang chính để nhận file. Muốn dùng máy này làm máy tổng, bật “Chế độ máy tổng” trong Cài đặt.</p>
            <div className="card p-3 mt-4 text-sm space-y-1">
              <div><span className="text-mute">Tên máy: </span>{name}</div>
              <div className="truncate"><span className="text-mute">Nhận về: </span><span className="mono text-xs">{dest}</span></div>
            </div>
          </>
        )}
      </motion.div>
      </AnimatePresence>

      <div className="flex gap-2">
        {step > 0 && <button className="btn btn-lg" onClick={() => setStep(step - 1)}>Quay lại</button>}
        {step < 2
          ? <button className="btn btn-primary btn-lg flex-1 magnet" disabled={step === 0 && !name.trim()} onClick={() => setStep(step + 1)}>Tiếp tục<Nest /></button>
          : <button className="btn btn-primary btn-lg flex-1 magnet" disabled={busy} onClick={finish}>Bắt đầu dùng Toss<Nest /></button>}
      </div>
      </div>
    </div>
  );
}
