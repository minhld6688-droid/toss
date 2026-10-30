// "Chia sẻ qua trình duyệt": the shell serves the chosen items on the LAN; anyone on the same wifi opens the link
// (or scans the QR) in a browser and downloads. Shown inside "Gửi tới ai?" right after starting, and as a card on
// Home for as long as the share runs.
import { useState } from "react";
import type { WebShare } from "@contracts/ipc";
import { api, errorMessage } from "../lib/ipc";
import { useStore } from "../lib/store";
import { Icon } from "./ui";

export async function stopWebShare() {
  const { setWebShare, toast } = useStore.getState();
  try { await api.web_share_stop(); setWebShare(null); }
  catch (e) { toast(errorMessage(e), "error"); }
}

export function WebSharePanel({ w, onStop, compact = false }: { w: WebShare; onStop: () => void; compact?: boolean }) {
  const toast = useStore((s) => s.toast);
  const [copied, setCopied] = useState<string | null>(null);

  async function copy(url: string) {
    try {
      await api.copy_text({ text: url });
      setCopied(url);
      window.setTimeout(() => setCopied((c) => (c === url ? null : c)), 1800);
    } catch (e) { toast(errorMessage(e), "error"); }
  }

  return (
    <div className="rounded-[16px] hair bg-accent/[0.06] p-3">
      <div className="flex gap-3 items-start">
        <div className={`${compact ? "w-[84px] h-[84px]" : "w-[104px] h-[104px]"} shrink-0 rounded-[12px] bg-white p-1.5 overflow-hidden`} dangerouslySetInnerHTML={{ __html: w.qr_svg }} role="img" aria-label="Mã QR để mở trên điện thoại" />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5 text-sm font-medium mb-1">
            <span className="pulse-dot w-1.5 h-1.5 rounded-full bg-ok shrink-0" />Đang chia sẻ
          </div>
          <div className="text-xs text-mute mb-1.5">Máy cùng wifi mở link này (hoặc quét mã) là tải về, không cần cài app.</div>
          <ul className="space-y-1">
            {w.urls.map((u) => (
              <li key={u} className="flex items-center gap-1.5">
                <span className="mono text-xs selectable truncate min-w-0 flex-1" title={u}>{u}</span>
                <button className="btn btn-icon !h-7 !w-7 shrink-0" aria-label={`Sao chép ${u}`} onClick={() => void copy(u)}><Icon name={copied === u ? "check" : "copy"} className="w-3.5 h-3.5" weight="bold" /></button>
              </li>
            ))}
          </ul>
        </div>
      </div>
      <button className="btn btn-danger w-full mt-3" onClick={onStop}><Icon name="x" />Dừng chia sẻ</button>
    </div>
  );
}

/** Home card while a browser share runs. */
export function WebShareCard({ className = "" }: { className?: string }) {
  const w = useStore((s) => s.webShare);
  if (!w) return null;
  return <div className={className}><WebSharePanel w={w} compact onStop={() => void stopWebShare()} /></div>;
}
