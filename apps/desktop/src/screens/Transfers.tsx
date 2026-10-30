import { useMemo } from "react";
import type { ConnPath, Received, TransferState } from "@contracts/ipc";
import { api, errorMessage } from "../lib/ipc";
import { fmtAgo, fmtBytes, fmtEta, fmtItems, fmtSpeed } from "../lib/format";
import { useStore } from "../lib/store";
import { Badge, EASE, Icon, Progress, Words, motion } from "../components/ui";
import { TossMark } from "../components/TossMark";
import { LanBlockedCard } from "../components/LanBlocked";
import type React from "react";

const pathLabel: Record<ConnPath, string | null> = { direct: "Trực tiếp", relay: "Relay", mixed: "Hỗn hợp", unknown: null };

/** Direction of the bytes. Pushes (contacts, nearby Toss, LocalSend) are a "put": sent by the client side, received by the host side. A "get" is a download. */
const isIncoming = (t: TransferState) => (t.side === "client" && t.op === "get") || (t.side === "host" && t.op === "put");

function statusLine(t: TransferState, contactName: string | null, contactOnline: boolean | null): string {
  switch (t.status) {
    case "connecting":
      if (t.error) return t.error;
      // queued push: the contact is offline, it starts by itself when they come online
      if (t.contact && !isIncoming(t) && contactOnline === false) return `Chờ ${contactName ?? t.peer} online`;
      return "Đang kết nối…";
    case "running": return [fmtSpeed(t.speed_bps), fmtEta(t.eta_s)].filter(Boolean).join(" · ");
    case "paused": return t.bytes_done > 0 ? "Tạm dừng · bấm Tiếp tục để gửi nốt" : "Tạm dừng";
    case "done": return `Xong · ${t.files_total} file`;
    case "cancelled": return "Đã huỷ";
    case "error": return t.error ?? "Lỗi";
  }
}

function Item({ t, i }: { t: TransferState; i: number }) {
  const toast = useStore((s) => s.toast);
  const refresh = useStore((s) => s.refreshTransfers);
  const contact = useStore((s) => (t.contact ? s.contacts.find((c) => c.id === t.contact) ?? null : null));
  const openDiagnostics = useStore((s) => s.openDiagnostics);
  const call = async (fn: () => Promise<unknown>) => { try { await fn(); } catch (e) { toast(errorMessage(e), "error"); } };
  const frac = t.bytes_total > 0 ? t.bytes_done / t.bytes_total : t.status === "done" ? 1 : 0;
  const active = t.status === "connecting" || t.status === "running" || t.status === "paused";
  const finished = t.status === "done" || t.status === "error" || t.status === "cancelled";
  const tone = t.status === "done" ? "ok" : t.status === "error" ? "bad" : t.status === "paused" || t.status === "cancelled" ? "mute" : "accent";
  const pl = pathLabel[t.path];
  const incoming = isIncoming(t);
  const peerName = contact?.name ?? t.peer;
  return (
    <motion.li
      layout="position" className="shell rise" style={{ "--i": i } as React.CSSProperties}
      transition={{ layout: { duration: 0.6, ease: EASE } }}
    ><div className="core p-3.5">
      <div className="flex items-start gap-2.5">
        <div className={`relative grid place-items-center w-8 h-8 rounded-full shrink-0 ${t.status === "done" ? "bg-ok/15 text-ok" : incoming ? "bg-accent/15 text-accent" : "bg-ok/15 text-ok"} ${t.status === "running" ? `drip ${incoming ? "" : "up"}` : ""}`}>
          {t.status === "done" ? (
            <motion.span
              key="done" className="grid place-items-center"
              initial={{ scale: 0, rotate: -40 }} animate={{ scale: 1, rotate: 0 }} transition={{ type: "spring", stiffness: 520, damping: 16 }}
            ><Icon name="check" className="w-4 h-4" weight="bold" /></motion.span>
          ) : <Icon name={t.status === "connecting" && !incoming && contact?.online === false ? "clock" : incoming ? "down" : "up"} className="w-4 h-4" weight="bold" />}
          {t.status === "done" && (
            <motion.span
              className="absolute inset-0 rounded-full border-2 border-ok pointer-events-none"
              initial={{ scale: 1, opacity: 0.8 }} animate={{ scale: 2.2, opacity: 0 }} transition={{ duration: 0.8, ease: EASE }}
            />
          )}
        </div>
        <div className="min-w-0 flex-1">
          <div className="truncate text-base font-semibold tracking-tight" title={t.job}>{t.job}</div>
          <div className="text-xs text-mute truncate">{incoming ? "Nhận từ" : "Gửi tới"} {peerName}</div>
        </div>
        <div className="flex flex-col items-end gap-0.5 shrink-0">
          <div className="flex items-center gap-1">
            {pl && <Badge tone={t.path === "direct" ? "ok" : "mute"}>{pl}</Badge>}
            {pl && t.rtt_ms != null && <span className="mono text-[11px] text-mute" title="Độ trễ (RTT) của đường đang dùng">{t.rtt_ms} ms</span>}
          </div>
          {t.path === "relay" && (
            <button className="text-[11px] leading-4 text-accent underline underline-offset-2 decoration-accent/40 hover:decoration-accent rounded-sm" onClick={openDiagnostics}>Vì sao?</button>
          )}
        </div>
      </div>

      {(active || t.status === "error" || t.status === "done") && (
        <div className="mt-3"><Progress value={frac} tone={tone} live={t.status === "running"} /></div>
      )}

      <div className="mt-2 flex items-center justify-between gap-2 text-xs">
        <div className={`min-w-0 truncate ${t.status === "error" ? "text-bad" : "text-mute"}`}>{statusLine(t, contact?.name ?? null, contact ? contact.online : null)}</div>
        <div className="text-mute shrink-0 mono">
          {t.bytes_total > 0 ? `${fmtBytes(t.bytes_done)} / ${fmtBytes(t.bytes_total)}` : ""}
        </div>
      </div>

      <div className="mt-3 flex flex-wrap gap-2 justify-end">
        {t.status === "running" && <button className="btn" onClick={() => call(() => api.pause({ id: t.id }))}><Icon name="pause" />Tạm dừng</button>}
        {t.status === "paused" && <button className="btn" onClick={() => call(() => api.resume({ id: t.id }))}><Icon name="play" />Tiếp tục</button>}
        {t.status === "error" && incoming && <button className="btn" onClick={() => call(() => api.resume({ id: t.id }))}><Icon name="refresh" />Thử lại</button>}
        {finished && !incoming && (
          <button className="btn" onClick={() => call(async () => { await api.resend({ id: t.id }); await refresh(); toast(`Đang gửi lại ${t.job}…`); })}><Icon name="refresh" />Gửi lại</button>
        )}
        {active && <button className="btn btn-danger" onClick={() => call(() => api.cancel({ id: t.id }))}><Icon name="x" />Huỷ</button>}
        {t.status === "done" && t.dest && <button className="btn" onClick={() => call(() => api.open_path({ path: t.dest! }))}><Icon name="folder" />Mở folder</button>}
      </div>
    </div></motion.li>
  );
}

/** "Đã nhận": what arrived before (survives restarts); items still listed above as finished transfers are not repeated. */
function ReceivedList({ skipDest }: { skipDest: Set<string> }) {
  const received = useStore((s) => s.received);
  const toast = useStore((s) => s.toast);
  const list = received.filter((r) => !skipDest.has(r.dest));
  if (list.length === 0) return null;
  return (
    <section className="mt-6 rise" style={{ "--i": 2 } as React.CSSProperties}>
      <h2 className="h-section">Đã nhận</h2>
      <ul className="card divide-y divide-line/10 overflow-hidden">
        {list.map((r: Received) => (
          <li key={r.id} className="flex items-center gap-2.5 px-3.5 py-2.5">
            <span className="grid place-items-center w-8 h-8 rounded-full bg-ok/15 text-ok shrink-0"><Icon name="down" className="w-4 h-4" weight="bold" /></span>
            <div className="min-w-0 flex-1">
              <div className="truncate text-base font-medium" title={r.job}>{r.job}</div>
              <div className="text-xs text-mute truncate">{r.from_name} · {fmtItems(r.files, r.bytes)} · {fmtAgo(r.at)}</div>
            </div>
            <button className="btn btn-icon shrink-0" title="Mở folder" aria-label={`Mở folder ${r.job}`} onClick={() => api.open_path({ path: r.dest }).catch((e) => toast(errorMessage(e), "error"))}><Icon name="folder" /></button>
          </li>
        ))}
      </ul>
    </section>
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
  const doneDest = useMemo(() => new Set(list.filter((t) => t.status === "done" && t.dest).map((t) => t.dest as string)), [list]);
  const receivedShown = useStore((s) => s.received.some((r) => !doneDest.has(r.dest)));

  return (
    <div className="p-4">
      <LanBlockedCard className="mb-4" />
      <div className="flex items-center justify-between mb-3">
        <Words text="Chuyển file" accent={["file"]} className="text-[30px]" />
        <button
          className="btn" disabled={!hasFinished}
          onClick={async () => { try { await api.clear_finished(); await refresh(); } catch (e) { toast(errorMessage(e), "error"); } }}
        ><Icon name="trash" />Xoá đã xong</button>
      </div>
      {list.length === 0 ? (
        <div className={`text-center text-mute rise ${receivedShown ? "py-8" : "py-16"}`}>
          <TossMark className={`mx-auto mb-2 opacity-80 ${receivedShown ? "w-28" : "w-40"}`} speed={0.6} />
          <div className="title text-xl text-ink">Chưa có lượt <em>chuyển</em> nào</div>
          <div className="text-sm">Kéo file hoặc folder vào Trang chính để gửi.</div>
        </div>
      ) : (
        <ul className="space-y-3">{list.map((t, i) => <Item key={t.id} t={t} i={i} />)}</ul>
      )}
      <ReceivedList skipDest={doneDest} />
    </div>
  );
}
