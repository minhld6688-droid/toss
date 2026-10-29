import type React from "react";
import { useCallback, useEffect, useRef, useState } from "react";
import type { TicketInfo } from "../lib/ipc-types";
import { api, errorMessage } from "../lib/ipc";
import { baseName, midTruncate } from "../lib/format";
import { useStore } from "../lib/store";
import { Badge, FullSheet, Icon, Nest, Sheet, Words } from "./ui";

const DAYS = [
  { v: 1, label: "1 ngày" }, { v: 3, label: "3 ngày" }, { v: 7, label: "7 ngày" }, { v: 30, label: "30 ngày" }, { v: 0, label: "Không hết hạn" },
];

type Phase = "creating" | "ready" | "error";

/**
 * Full-height "Tạo link" sheet for a folder (right-click menu, dropped folder, or the "Chia sẻ folder…" button).
 * The link is created IMMEDIATELY with the defaults (7 ngày, không nhận file ngược) — the shell has already put it in
 * the clipboard by the time the user sees it. Changing hạn / put offers "Tạo lại với tuỳ chọn khác".
 */
export function ShareView({ path }: { path: string }) {
  const close = useStore((s) => s.closeShare);
  const toast = useStore((s) => s.toast);
  const setApp = useStore((s) => s.setApp);
  const bumpShares = useStore((s) => s.bumpShares);

  const [days, setDays] = useState(7);
  const [put, setPut] = useState(false);
  const [phase, setPhase] = useState<Phase>("creating");
  const [info, setInfo] = useState<TicketInfo | null>(null);
  const [used, setUsed] = useState<{ days: number; put: boolean } | null>(null);
  const [err, setErr] = useState("");
  const [svg, setSvg] = useState("");
  const [copied, setCopied] = useState<null | "auto" | "again" | "raw">(null);

  const seq = useRef(0);
  const alive = useRef(false);
  const started = useRef(false);
  const flash = useRef<number>(0);

  const create = useCallback(async (d: number, p: boolean) => {
    const my = ++seq.current;
    setPhase("creating"); setErr("");
    try {
      const t = await api.share_folder({ path, days: d, put: p });
      if (!alive.current || my !== seq.current) return;
      setInfo(t); setUsed({ days: d, put: p }); setPhase("ready");
      setCopied("auto"); // the shell copied the link itself
      bumpShares();
      api.get_state().then(setApp).catch(() => { /* host mode may have just turned on; settings re-read on open */ });
    } catch (e) {
      if (!alive.current || my !== seq.current) return;
      setErr(errorMessage(e)); setPhase("error"); setCopied(null);
    }
  }, [path, bumpShares, setApp]);

  useEffect(() => {
    alive.current = true;
    if (!started.current) { started.current = true; void create(7, false); }
    return () => { alive.current = false; window.clearTimeout(flash.current); };
  }, [create]);

  useEffect(() => {
    if (!info) return;
    let live = true;
    api.qr_svg({ text: info.web_url }).then((s) => live && setSvg(s)).catch((e) => toast(errorMessage(e), "error"));
    return () => { live = false; };
  }, [info, toast]);

  async function copy(what: "again" | "raw", text: string) {
    try {
      await api.copy_text({ text });
      setCopied(what);
      window.clearTimeout(flash.current);
      flash.current = window.setTimeout(() => setCopied("auto"), 1600);
    } catch (e) { toast(errorMessage(e), "error"); }
  }

  const creating = phase === "creating";
  const changed = !!used && (days !== used.days || put !== used.put);
  const canRedo = !creating && (changed || phase === "error");

  return (
    <FullSheet onClose={close} title="Tạo link">
      <div className="relative z-10 flex-1 min-h-0 overflow-y-auto px-4 pt-4 pb-2">
        <div className="flex items-center justify-between mb-2.5">
          <Words text="Tạo link" accent={["link"]} className="text-[28px]" />
          {creating ? <Badge>Đang tạo…</Badge> : phase === "ready" ? <Badge tone="ok">Sẵn sàng</Badge> : <Badge tone="bad">Lỗi</Badge>}
        </div>

        {/* folder */}
        <div className="card mb-2.5 rise px-3.5 py-2.5 flex items-center gap-3">
          <span className="grid place-items-center w-9 h-9 rounded-full shrink-0 bg-accent/15 text-accent"><Icon name="folder" className="w-[18px] h-[18px]" weight="bold" /></span>
          <div className="min-w-0 flex-1">
            <div className="truncate text-base font-semibold tracking-tight" title={baseName(path)}>{baseName(path)}</div>
            <div className="mono text-xs text-mute selectable whitespace-nowrap overflow-hidden" title={path}>{midTruncate(path, 12, 22)}</div>
          </div>
        </div>

        {/* result */}
        <div className="shell mb-2.5 rise" style={{ "--i": 1 } as React.CSSProperties}><div className="core p-3.5">
          {phase === "error" ? (
            <div className="py-6 text-center">
              <span className="mx-auto mb-2 grid place-items-center w-10 h-10 rounded-full bg-bad/15 text-bad"><Icon name="x" className="w-5 h-5" weight="bold" /></span>
              <div className="text-base font-medium">Không tạo được link</div>
              <div className="text-sm text-bad mt-1 break-words">{err}</div>
            </div>
          ) : (
            <>
              <div className="flex justify-center">
                <div
                  className={`relative w-[132px] h-[132px] rounded-[16px] bg-white p-2 overflow-hidden shadow-[0_10px_30px_-14px_rgb(var(--accent)/0.6)] transition-[opacity,filter] duration-500 ease-aura ${creating ? "opacity-60 blur-[2px]" : ""} ${creating && !svg ? "sheen" : ""}`}
                  aria-label="Mã QR"
                  dangerouslySetInnerHTML={{ __html: svg }}
                />
              </div>
              <div className="mono text-xs text-center text-ink selectable mt-3 whitespace-nowrap overflow-hidden min-h-[16px]" title={info?.web_url}>
                {info ? midTruncate(info.web_url, 30, 12) : ""}
              </div>
              <button
                className="btn btn-primary btn-lg w-full mt-2.5"
                disabled={creating || !info}
                onClick={() => info && copy("again", info.web_url)}
              >
                {creating ? "Đang tạo link…" : <><Icon name={copied ? "check" : "copy"} weight="bold" />{copied === "again" ? "Đã copy lại" : copied === "raw" || copied === "auto" ? "Đã copy vào clipboard" : "Copy link"}</>}
              </button>
              <div className="flex items-center gap-2 mt-2.5 text-xs">
                <span className="text-mute shrink-0">Mã</span>
                <span className="mono text-mute selectable flex-1 min-w-0 whitespace-nowrap overflow-hidden" title={info?.ticket}>{info ? midTruncate(info.ticket, 18, 8) : ""}</span>
                <button className="btn btn-ghost !h-7 !px-2.5 !text-xs shrink-0" disabled={!info} onClick={() => info && copy("raw", info.ticket)}>
                  <Icon name={copied === "raw" ? "check" : "copy"} className="w-3.5 h-3.5" />{copied === "raw" ? "Đã copy" : "Copy mã"}
                </button>
              </div>
            </>
          )}
        </div></div>

        {/* options */}
        <div className="card mb-1 rise px-3.5" style={{ "--i": 2 } as React.CSSProperties}>
          <div className="flex items-center justify-between gap-3 min-h-[42px]">
            <label htmlFor="share-days" className="text-base">Hạn</label>
            <select id="share-days" className="input !w-40" value={days} disabled={creating} onChange={(e) => setDays(Number(e.target.value))}>
              {DAYS.map((d) => <option key={d.v} value={d.v}>{d.label}</option>)}
            </select>
          </div>
          <label className="flex items-center gap-2.5 min-h-[42px] cursor-pointer border-t border-line/10" title="Người nhận có thể trả file về máy này">
            <input type="checkbox" checked={put} disabled={creating} onChange={(e) => setPut(e.target.checked)} />
            <span className="text-base">Cho phép gửi file ngược lại</span>
          </label>
        </div>
      </div>

      <div className="relative z-10 px-4 pb-4 pt-2 flex gap-2">
        <button className={`btn btn-lg ${canRedo ? "" : "btn-primary flex-1"}`} onClick={close}>Xong</button>
        {canRedo && (
          <button className="btn btn-primary btn-lg flex-1 rise" onClick={() => void create(days, put)}>
            {changed ? "Tạo lại với tuỳ chọn khác" : "Thử lại"}<Nest name="refresh" />
          </button>
        )}
      </div>
    </FullSheet>
  );
}
