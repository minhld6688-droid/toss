// v0.5.1 in-app updates: Settings → Cập nhật, the Home banner, and "Cập nhật máy đó" for the remote host.
import { useEffect, useRef, useState } from "react";
import { motion, useReducedMotion } from "motion/react";
import type { HostStatus } from "@contracts/ipc";
import { api, errorMessage } from "../lib/ipc";
import { fmtBytes } from "../lib/format";
import { useStore } from "../lib/store";
import { cleanNotes, useUpdate } from "../lib/update";
import { AnimatePresence, Badge, EASE, Icon, Nest, Progress, Row, Section, Toggle } from "./ui";

/** Spinning update glyph (still with reduced motion). */
function Spinner({ on, className = "w-4 h-4" }: { on: boolean; className?: string }) {
  const reduce = useReducedMotion();
  return (
    <motion.span
      className="grid" animate={on && !reduce ? { rotate: 360 } : { rotate: 0 }}
      transition={on && !reduce ? { duration: 1.1, ease: "linear", repeat: Infinity } : { duration: 0.3 }}
    ><Icon name="update" className={className} /></motion.span>
  );
}

/** Progress line of a running install: waiting for transfers → download → install. */
function InstallProgress() {
  const { phase, active, downloaded, total } = useUpdate();
  if (!phase) return null;
  const frac = phase === "installing" ? 1 : phase === "downloading" && total ? downloaded / total : 0;
  const text = phase === "waiting"
    ? (active > 0 ? `Chờ ${active} lượt chuyển file xong…` : "Đang chuẩn bị…")
    : phase === "downloading"
      ? (total ? `Đang tải ${fmtBytes(downloaded)} / ${fmtBytes(total)}` : `Đang tải ${fmtBytes(downloaded)}`)
      : "Đang cài đặt — Toss sẽ tự mở lại";
  return (
    <div className="space-y-1.5" aria-live="polite">
      <Progress value={phase === "waiting" ? 0.04 : frac} live={phase !== "installing"} tone={phase === "installing" ? "ok" : "accent"} />
      <div className="flex items-center justify-between text-xs text-mute">
        <span>{text}</span>
        {phase === "downloading" && total ? <span className="mono">{Math.round(frac * 100)}%</span> : null}
      </div>
    </div>
  );
}

export function UpdateSection() {
  const app = useStore((s) => s.app)!;
  const patch = useStore((s) => s.patchApp);
  const toast = useStore((s) => s.toast);
  const { info, check, checkError, phase, error, runCheck, install } = useUpdate();
  const reduce = useReducedMotion();
  const avail = info?.available ? info : null;
  const notes = cleanNotes(avail?.notes);

  return (
    <Section title="Cập nhật">
      <div className="space-y-3">
        <div className="flex items-center justify-between gap-3">
          <div className="min-w-0">
            <div className="text-xs text-mute">Phiên bản đang dùng</div>
            <div className="title text-[22px] leading-tight">Toss <em>{app.version}</em></div>
          </div>
          <button className="btn shrink-0" disabled={check === "checking" || !!phase} onClick={() => void runCheck()}>
            <Spinner on={check === "checking"} />{check === "checking" ? "Đang kiểm tra…" : "Kiểm tra cập nhật"}
          </button>
        </div>

        <AnimatePresence initial={false} mode="popLayout">
          {check === "done" && !avail && (
            <motion.div
              key="latest" className="flex items-center gap-2 text-sm text-ok"
              initial={reduce ? { opacity: 0 } : { opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }}
              transition={{ duration: 0.45, ease: EASE }}
            ><Icon name="check" className="w-4 h-4" weight="bold" />Bạn đang dùng bản mới nhất</motion.div>
          )}
          {check === "error" && (
            <motion.div key="err" className="text-sm text-bad" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}>{checkError}</motion.div>
          )}
          {avail && (
            <motion.div
              key={`new-${avail.version}`}
              className="rounded-[16px] border border-accent/30 bg-accent/[0.07] p-3 space-y-2.5"
              initial={reduce ? { opacity: 0 } : { opacity: 0, y: 10, scale: 0.98 }} animate={{ opacity: 1, y: 0, scale: 1 }} exit={{ opacity: 0 }}
              transition={{ type: "spring", stiffness: 360, damping: 30 }}
            >
              <div className="flex items-center gap-2.5">
                <span className="grid place-items-center w-9 h-9 rounded-full bg-accent/15 text-accent shrink-0"><Icon name="update" className="w-[18px] h-[18px]" weight="bold" /></span>
                <div className="min-w-0 flex-1">
                  <div className="title text-[19px] leading-tight">Có bản <em>{avail.version}</em></div>
                  {avail.date && <div className="text-xs text-mute">Phát hành {fmtReleaseDate(avail.date)}</div>}
                </div>
                {avail.forced && <Badge tone="warn">Bắt buộc</Badge>}
              </div>
              {notes && <div className="text-sm whitespace-pre-line max-h-28 overflow-y-auto selectable pr-1">{notes}</div>}
              {phase ? <InstallProgress /> : (
                <button className="btn btn-primary w-full magnet" onClick={() => void install()}>
                  Cập nhật &amp; khởi động lại<Nest name="update" />
                </button>
              )}
              {!phase && <div className="text-xs text-mute flex gap-1.5"><Icon name="info" className="w-3.5 h-3.5 mt-px shrink-0" /><span>File đang chuyển sẽ tạm dừng và tự chạy tiếp sau khi Toss mở lại.</span></div>}
            </motion.div>
          )}
        </AnimatePresence>
        {phase && !avail && <InstallProgress />}
        {error && <div className="text-sm text-bad">{error}</div>}

        <Row title="Tự động cập nhật" hint="Tự cài bản mới lúc không có file đang chuyển — nên bật cho máy không có người ngồi">
          <Toggle on={app.auto_update} label="Tự động cập nhật" onChange={async (on) => {
            try { await api.set_auto_update({ on }); patch({ auto_update: on }); } catch (e) { toast(errorMessage(e), "error"); }
          }} />
        </Row>
      </div>
    </Section>
  );
}

function fmtReleaseDate(d: string): string {
  // the feed gives RFC 3339 ("2026-10-23T08:00:00Z") or the time crate's "2026-10-23 8:00:00.0 +00:00:00"
  const m = d.match(/^(\d{4})-(\d{2})-(\d{2})/);
  return m ? `${Number(m[3])}/${Number(m[2])}/${m[1]}` : d;
}

/** Home: slim banner while a newer version is known (closed for the session per version). */
export function UpdateBanner({ className = "" }: { className?: string }) {
  const { info, dismissed, phase, install, dismiss } = useUpdate();
  const reduce = useReducedMotion();
  const show = !!info?.available && dismissed !== info.version;
  return (
    <AnimatePresence initial={false}>
      {show && (
        <motion.div
          key="update-banner" role="status"
          className={`overflow-hidden ${className}`}
          initial={reduce ? { opacity: 0 } : { opacity: 0, height: 0, y: -8 }} animate={{ opacity: 1, height: "auto", y: 0 }}
          exit={reduce ? { opacity: 0 } : { opacity: 0, height: 0, y: -8 }} transition={{ duration: 0.5, ease: EASE }}
        >
          <div className="glass rounded-full pl-1.5 pr-1 py-1 flex items-center gap-2 mb-4">
            <span className="relative grid place-items-center w-8 h-8 rounded-full bg-accent/15 text-accent shrink-0">
              <Icon name="update" className="w-4 h-4" weight="bold" />
              {!phase && <span className="pulse-dot absolute top-0.5 right-0.5 w-2 h-2 rounded-full bg-accent ring-2 ring-bg" />}
            </span>
            <div className="min-w-0 flex-1 text-sm truncate">
              {phase ? <BannerPhase /> : <>Có bản <span className="font-semibold">{info!.version}</span>{info!.forced ? " · bắt buộc" : ""}</>}
            </div>
            {!phase && (
              <>
                <button className="btn btn-primary !h-8 !px-3 !text-xs shrink-0" onClick={() => void install()}>Cập nhật</button>
                <button className="btn btn-ghost btn-icon !w-8 !h-8 shrink-0 text-mute" aria-label="Ẩn" title="Ẩn đến lần mở sau" onClick={dismiss}><Icon name="x" className="w-3.5 h-3.5" /></button>
              </>
            )}
          </div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}

function BannerPhase() {
  const { phase, active, downloaded, total } = useUpdate();
  if (phase === "waiting") return <span className="text-mute">{active > 0 ? `Chờ ${active} lượt chuyển xong…` : "Đang chuẩn bị…"}</span>;
  if (phase === "installing") return <span className="text-mute">Đang cài đặt…</span>;
  return <span className="text-mute">Đang tải bản mới{total ? ` · ${Math.round((downloaded / total) * 100)}%` : "…"}</span>;
}

/** Remote machine card: ask the controlled machine to update itself, then watch its version change. */
export function RemoteUpdate({ status, onStatus }: { status: HostStatus; onStatus: (s: HostStatus) => void }) {
  const toast = useStore((s) => s.toast);
  const [state, setState] = useState<"idle" | "sending" | "sent" | "updated" | "timeout">("idle");
  const [err, setErr] = useState<string | null>(null);
  const [newVersion, setNewVersion] = useState<string | null>(null);
  const timer = useRef<number | null>(null);
  useEffect(() => () => { if (timer.current) window.clearInterval(timer.current); }, []);

  async function send() {
    setState("sending"); setErr(null);
    const before = status.version;
    try {
      await api.remote_update();
    } catch (e) {
      setState("idle"); setErr(errorMessage(e)); return;
    }
    setState("sent");
    const t0 = Date.now();
    timer.current = window.setInterval(async () => {
      if (Date.now() - t0 > 5 * 60_000) {
        window.clearInterval(timer.current!); timer.current = null; setState("timeout"); return;
      }
      try {
        const s = await api.remote_status();
        if (s.version !== before) {
          window.clearInterval(timer.current!); timer.current = null;
          setNewVersion(s.version); setState("updated"); onStatus(s);
          toast(`Máy kia đã lên bản ${s.version}`);
        }
      } catch { /* restarting: offline for a moment */ }
    }, 10_000);
  }

  return (
    <div className="space-y-1.5">
      <button className="btn w-full" disabled={state === "sending" || state === "sent"} onClick={() => void send()}>
        <Spinner on={state === "sending" || state === "sent"} />
        {state === "sending" ? "Đang gửi lệnh…" : "Cập nhật máy đó"}
      </button>
      {state === "sent" && <div className="text-xs text-mute flex gap-1.5" aria-live="polite"><Icon name="clock" className="w-3.5 h-3.5 mt-px shrink-0" /><span>Đã gửi lệnh — máy đó sẽ tự cập nhật và khởi động lại</span></div>}
      {state === "updated" && <div className="text-xs text-ok flex gap-1.5"><Icon name="check" className="w-3.5 h-3.5 mt-px shrink-0" weight="bold" /><span>Máy đó đã lên bản {newVersion}</span></div>}
      {state === "timeout" && <div className="text-xs text-mute">Chưa thấy bản mới sau 5 phút — máy đó có thể đang chờ file chuyển xong. Bấm làm mới để xem lại.</div>}
      {err && <div className="text-xs text-bad">{err}</div>}
    </div>
  );
}
