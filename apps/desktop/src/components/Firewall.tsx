import { useState } from "react";
import { useReducedMotion } from "motion/react";
import { api, errorMessage } from "../lib/ipc";
import { useStore } from "../lib/store";
import { EASE, Icon, Nest, Sheet, motion } from "./ui";

/** "Cho phép Toss qua firewall": one button, one OS prompt (UAC / admin password). Shared by the
 *  setup wizard, the host-mode prompt and the Kết nối section. */
export function FirewallAllow({ onDone, compact = false }: { onDone?: () => void; compact?: boolean }) {
  const toast = useStore((s) => s.toast);
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(false);

  async function allow() {
    setBusy(true);
    try {
      await api.add_firewall_rule();
      setDone(true);
      toast("Đã cho phép Toss qua firewall");
      onDone?.();
    } catch (e) { toast(errorMessage(e), "error"); }
    setBusy(false);
  }

  if (done) {
    return (
      <div className="flex items-center gap-2 text-sm text-ok">
        <Icon name="check" className="w-4 h-4" weight="bold" />Toss đã được phép nhận kết nối
      </div>
    );
  }
  return (
    <button className={`btn btn-primary ${compact ? "w-full" : "btn-lg w-full"} magnet`} disabled={busy} onClick={allow}>
      <Icon name="shield" />{busy ? "Đang chờ bạn đồng ý…" : "Cho phép qua firewall"}<Nest />
    </button>
  );
}

export const FIREWALL_WHY =
  "Firewall của máy đang chặn các kết nối UDP vào Toss, nên máy ở xa chỉ nối được qua relay (chậm). Bấm nút rồi đồng ý một lần ở hộp thoại quyền quản trị.";

/** Offered once when serving starts and the firewall would block other machines. */
export function FirewallSheet({ onClose }: { onClose: () => void }) {
  return (
    <Sheet onClose={onClose} title="Cho phép Toss qua firewall">
      <div className="flex items-center gap-3 mb-2">
        <span className="grid place-items-center w-10 h-10 rounded-full bg-accent/15 text-accent shrink-0"><Icon name="shield" className="w-5 h-5" weight="bold" /></span>
        <div className="title text-xl leading-tight">Cho phép Toss <em>qua firewall</em></div>
      </div>
      <p className="text-sm text-mute mb-4">{FIREWALL_WHY}</p>
      <div className="space-y-2">
        <FirewallAllow compact onDone={() => window.setTimeout(onClose, 900)} />
        <button className="btn btn-ghost w-full" onClick={onClose}>Để sau</button>
      </div>
    </Sheet>
  );
}

const FW_DISMISS = "toss.fw.dismissed";
function fwDismissed(): boolean {
  try { return localStorage.getItem(FW_DISMISS) === "1"; } catch { return false; }
}

/** Non-blocking banner on Home (v0.8: replaces the setup-wizard step): the firewall would stop other machines from connecting. */
export function FirewallNotice({ className = "" }: { className?: string }) {
  const fw = useStore((s) => s.firewall);
  const setFirewall = useStore((s) => s.setFirewall);
  const reduce = useReducedMotion();
  const [hidden, setHidden] = useState(fwDismissed);
  if (fw !== "missing" || hidden) return null;
  return (
    <motion.div
      className={`rounded-[20px] border border-warn/40 bg-warn/10 p-3.5 ${className}`}
      initial={reduce ? false : { opacity: 0, y: -8, scale: 0.98 }} animate={{ opacity: 1, y: 0, scale: 1 }} transition={{ duration: 0.6, ease: EASE }}
    >
      <div className="flex items-start gap-3">
        <span className="grid place-items-center w-9 h-9 rounded-full bg-warn/20 text-warn shrink-0"><Icon name="shield" className="w-[18px] h-[18px]" weight="bold" /></span>
        <div className="min-w-0">
          <div className="text-base font-medium">Cho phép Toss qua firewall</div>
          <div className="text-xs text-mute mt-0.5">Firewall đang chặn máy khác nối vào Toss, nên chỉ máy ở xa nối được và sẽ chậm. Bấm một lần và đồng ý.</div>
        </div>
      </div>
      <div className="flex gap-2 mt-3">
        <div className="flex-1"><FirewallAllow compact onDone={() => window.setTimeout(() => setFirewall("ok"), 900)} /></div>
        <button className="btn btn-ghost" onClick={() => { try { localStorage.setItem(FW_DISMISS, "1"); } catch { /* ignore */ } setHidden(true); }}>Để sau</button>
      </div>
    </motion.div>
  );
}
