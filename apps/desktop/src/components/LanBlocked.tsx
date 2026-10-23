import { useState } from "react";
import { motion, useReducedMotion } from "motion/react";
import { api, errorMessage } from "../lib/ipc";
import { useStore } from "../lib/store";
import { EASE, Icon, Nest } from "./ui";

/** Shown (Home, Chuyển file, Kết nối) while macOS denies Toss the local network: LAN sends fail with
 *  EHOSTUNREACH and fall back to the relay, ~1 MB/s instead of ~60 MB/s. */
export function LanBlockedCard({ className = "" }: { className?: string }) {
  const blocked = useStore((s) => s.lanBlocked);
  const toast = useStore((s) => s.toast);
  const reduce = useReducedMotion();
  const [busy, setBusy] = useState(false);
  if (!blocked) return null;

  async function open() {
    setBusy(true);
    try { await api.open_local_network_settings(); }
    catch (e) { toast(errorMessage(e), "error"); }
    setBusy(false);
  }

  return (
    <motion.div
      role="alert"
      className={`rounded-[20px] border border-warn/40 bg-warn/10 p-3.5 ${className}`}
      initial={reduce ? false : { opacity: 0, y: -8, scale: 0.98 }} animate={{ opacity: 1, y: 0, scale: 1 }}
      transition={{ duration: 0.6, ease: EASE }}
    >
      <div className="flex items-start gap-3">
        <span className="relative grid place-items-center w-10 h-10 rounded-full bg-warn/20 text-warn shrink-0">
          <Icon name="offline" className="w-5 h-5" weight="bold" />
          {!reduce && (
            <motion.span
              className="absolute inset-0 rounded-full border border-warn pointer-events-none"
              initial={{ scale: 1, opacity: 0.7 }} animate={{ scale: 1.7, opacity: 0 }}
              transition={{ duration: 2.2, ease: "easeOut", repeat: Infinity }}
            />
          )}
        </span>
        <div className="min-w-0">
          <div className="title text-[19px] leading-[1.15] [&_em]:text-warn">macOS đang chặn Toss trong <em>mạng nội bộ</em></div>
          <div className="text-sm mt-1">Chuyển file sẽ rất chậm: khoảng 1 MB/s thay vì 60 MB/s.</div>
        </div>
      </div>
      <button className="btn btn-primary w-full mt-3 magnet" disabled={busy} onClick={open}>
        <Icon name="settings" />Mở cài đặt Mạng cục bộ<Nest />
      </button>
      <div className="text-xs text-mute mt-2 flex gap-1.5">
        <Icon name="info" className="w-3.5 h-3.5 mt-px shrink-0" />
        <span>Bật công tắc Toss trong danh sách, rồi thoát và mở lại Toss.</span>
      </div>
    </motion.div>
  );
}
