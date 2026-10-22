import { useMemo } from "react";
import type { ConnPath, TransferState } from "../lib/ipc-types";
import { api, errorMessage } from "../lib/ipc";
import { fmtBytes, fmtEta, fmtSpeed } from "../lib/format";
import { useStore } from "../lib/store";
import { Badge, EASE, Icon, Progress, Words, motion } from "../components/ui";
import { TossMark } from "../components/TossMark";
import type React from "react";

const pathLabel: Record<ConnPath, string | null> = { direct: "Trực tiếp", relay: "Relay", mixed: "Hỗn hợp", unknown: null };

function statusLine(t: TransferState): string {
  switch (t.status) {
    case "connecting": return t.error ?? "Đang kết nối…";
    case "running": return [fmtSpeed(t.speed_bps), fmtEta(t.eta_s)].filter(Boolean).join(" · ");
    case "paused": return "Tạm dừng";
    case "done": return `Xong · ${t.files_total} file`;
    case "cancelled": return "Đã huỷ";
    case "error": return t.error ?? "Lỗi";
  }
}

function Item({ t, i }: { t: TransferState; i: number }) {
  const toast = useStore((s) => s.toast);
  const call = async (fn: () => Promise<void>) => { try { await fn(); } catch (e) { toast(errorMessage(e), "error"); } };
  const frac = t.bytes_total > 0 ? t.bytes_done / t.bytes_total : t.status === "done" ? 1 : 0;
  const active = t.status === "connecting" || t.status === "running" || t.status === "paused";
  const tone = t.status === "done" ? "ok" : t.status === "error" ? "bad" : t.status === "paused" || t.status === "cancelled" ? "mute" : "accent";
  const pl = pathLabel[t.path];
  const isHostSide = t.side === "host";
  return (
    <motion.li
      layout="position" className="shell rise" style={{ "--i": i } as React.CSSProperties}
      transition={{ layout: { duration: 0.6, ease: EASE } }}
    ><div className="core p-3.5">
      <div className="flex items-start gap-2.5">
        <div className={`relative grid place-items-center w-8 h-8 rounded-full shrink-0 ${t.status === "done" ? "bg-ok/15 text-ok" : t.op === "get" ? "bg-accent/15 text-accent" : "bg-ok/15 text-ok"} ${t.status === "running" ? `drip ${t.op === "put" ? "up" : ""}` : ""}`}>
          {t.status === "done" ? (
            <motion.span
              key="done" className="grid place-items-center"
              initial={{ scale: 0, rotate: -40 }} animate={{ scale: 1, rotate: 0 }} transition={{ type: "spring", stiffness: 520, damping: 16 }}
            ><Icon name="check" className="w-4 h-4" weight="bold" /></motion.span>
          ) : <Icon name={t.op === "get" ? "down" : "up"} className="w-4 h-4" weight="bold" />}
          {t.status === "done" && (
            <motion.span
              className="absolute inset-0 rounded-full border-2 border-ok pointer-events-none"
              initial={{ scale: 1, opacity: 0.8 }} animate={{ scale: 2.2, opacity: 0 }} transition={{ duration: 0.8, ease: EASE }}
            />
          )}
        </div>
        <div className="min-w-0 flex-1">
          <div className="truncate text-base font-semibold tracking-tight" title={t.job}>{t.job}</div>
          <div className="text-xs text-mute truncate">
            {isHostSide ? `${t.op === "get" ? "Gửi cho" : "Nhận từ"} ${t.peer}` : t.op === "get" ? `Nhận từ ${t.peer}` : `Trả cho ${t.peer}`}
          </div>
        </div>
        <div className="flex gap-1 shrink-0">
          {isHostSide && <Badge tone="warn">Máy tổng</Badge>}
          {pl && <Badge tone={t.path === "direct" ? "ok" : "mute"}>{pl}</Badge>}
        </div>
      </div>

      {(active || t.status === "error" || t.status === "done") && (
        <div className="mt-3"><Progress value={frac} tone={tone} live={t.status === "running"} /></div>
      )}

      <div className="mt-2 flex items-center justify-between gap-2 text-xs">
        <div className={`min-w-0 truncate ${t.status === "error" ? "text-bad" : "text-mute"}`}>{statusLine(t)}</div>
        <div className="text-mute shrink-0 mono">
          {t.bytes_total > 0 ? `${fmtBytes(t.bytes_done)} / ${fmtBytes(t.bytes_total)}` : ""}
        </div>
      </div>

      <div className="mt-3 flex gap-2 justify-end">
        {t.status === "running" && <button className="btn" onClick={() => call(() => api.pause({ id: t.id }))}><Icon name="pause" />Tạm dừng</button>}
        {(t.status === "paused" || t.status === "error") && <button className="btn" onClick={() => call(() => api.resume({ id: t.id }))}><Icon name={t.status === "error" ? "refresh" : "play"} />{t.status === "error" ? "Thử lại" : "Tiếp tục"}</button>}
        {active && <button className="btn btn-danger" onClick={() => call(() => api.cancel({ id: t.id }))}><Icon name="x" />Huỷ</button>}
        {t.status === "done" && t.dest && <button className="btn" onClick={() => call(() => api.open_path({ path: t.dest! }))}><Icon name="folder" />Mở folder</button>}
      </div>
    </div></motion.li>
  );
}

export function Transfers() {
  const map = useStore((s) => s.transfers);
  const refresh = useStore((s) => s.refreshTransfers);
  const toast = useStore((s) => s.toast);
  const list = useMemo(() => {
    const rank = (t: TransferState) => (t.status === "running" ? 0 : t.status === "connecting" ? 1 : t.status === "paused" ? 2 : t.status === "error" ? 3 : 4);
    return Object.values(map).sort((a, b) => rank(a) - rank(b) || b.started_at - a.started_at);
  }, [map]);
  const hasFinished = list.some((t) => t.status === "done" || t.status === "error" || t.status === "cancelled");

  return (
    <div className="p-4">
      <div className="flex items-center justify-between mb-3">
        <Words text="Chuyển file" accent={["file"]} className="text-[30px]" />
        <button
          className="btn" disabled={!hasFinished}
          onClick={async () => { try { await api.clear_finished(); await refresh(); } catch (e) { toast(errorMessage(e), "error"); } }}
        ><Icon name="trash" />Xoá đã xong</button>
      </div>
      {list.length === 0 ? (
        <div className="text-center text-mute py-16 rise">
          <TossMark className="mx-auto w-40 mb-2 opacity-80" speed={0.6} />
          <div className="title text-xl text-ink">Chưa có lượt <em>chuyển</em> nào</div>
          <div className="text-sm">Dán link ở Trang chính để bắt đầu.</div>
        </div>
      ) : (
        <ul className="space-y-3">{list.map((t, i) => <Item key={t.id} t={t} i={i} />)}</ul>
      )}
    </div>
  );
}
