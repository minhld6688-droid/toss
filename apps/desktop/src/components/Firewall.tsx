import { useState } from "react";
import { api, errorMessage } from "../lib/ipc";
import { useStore } from "../lib/store";
import { Icon, Nest, Sheet } from "./ui";

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
