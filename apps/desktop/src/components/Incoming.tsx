// v0.8 incoming: one ask sheet for Toss pushes and LocalSend requests alike, and the text message sheet.
import { useEffect, useState } from "react";
import { useReducedMotion } from "motion/react";
import type { Incoming } from "@contracts/ipc";
import { api, errorMessage } from "../lib/ipc";
import { fmtAgo, fmtItems } from "../lib/format";
import { useStore } from "../lib/store";
import { centerOf } from "./Comet";
import { DeviceAvatar } from "./Nearby";
import { Badge, Icon, Nest, Sheet, motion } from "./ui";

/** Icon for an ask: the contact's platform, the nearby entry's platform, or a generic machine. */
function useAskIcon(p: Incoming): string {
  return useStore((s) => {
    const c = s.contacts.find((x) => x.id === p.from_key || `t:${x.id}` === p.from_key);
    if (c) return c.platform === "macos" ? "laptop" : "desktop";
    const n = s.nearby.find((x) => x.key === p.from_key);
    if (n) return n.platform === "ios" || n.platform === "android" ? "phone" : n.platform === "macos" ? "laptop" : "desktop";
    return p.source === "localsend" ? "phone" : "desktop";
  });
}

function askLine(p: Incoming) {
  return fmtItems(p.files, p.bytes);
}

/** Home banner while asks wait and the sheet is closed ("Để sau"). */
export function IncomingBanner({ className = "" }: { className?: string }) {
  const incoming = useStore((s) => s.incoming);
  const open = useStore((s) => s.incomingSheet);
  const setOpen = useStore((s) => s.setIncomingSheet);
  const first = incoming[0];
  if (!first || open) return null;
  return (
    <button
      className={`w-full rounded-[16px] bg-accent/10 hair border-accent/40 px-3.5 py-2.5 text-left flex items-center gap-2.5 active:scale-[0.99] transition-transform duration-300 ${className}`}
      onClick={() => setOpen(true)}
    >
      <span className="grid place-items-center w-8 h-8 rounded-full bg-accent/20 text-accent shrink-0"><Icon name="down" className="w-4 h-4" weight="bold" /></span>
      <span className="min-w-0 flex-1 text-sm truncate"><b className="font-semibold">{first.from_name}</b> muốn gửi {first.job}{incoming.length > 1 ? ` (+${incoming.length - 1})` : ""}</span>
      <span className="text-xs font-semibold text-accent shrink-0">Xem</span>
    </button>
  );
}

export function IncomingSheet() {
  const incoming = useStore((s) => s.incoming);
  const setOpen = useStore((s) => s.setIncomingSheet);
  const remove = useStore((s) => s.removeIncoming);
  const refresh = useStore((s) => s.refreshTransfers);
  const fly = useStore((s) => s.fly);
  const setScreen = useStore((s) => s.setScreen);
  const toast = useStore((s) => s.toast);
  const reduce = useReducedMotion();
  const [busy, setBusy] = useState<string | null>(null);

  const p = incoming[0];
  const icon = useAskIcon(p ?? ({ from_key: "", source: "toss" } as Incoming));
  useEffect(() => { if (!p) setOpen(false); }, [p, setOpen]);
  if (!p) return null;
  const rest = incoming.slice(1);

  async function answer(x: Incoming, accept: boolean, always: boolean, el?: Element | null) {
    setBusy(x.id);
    try {
      await api.answer_incoming({ id: x.id, accept, always });
      remove(x.id);
      if (accept) {
        await refresh();
        fly(el ? centerOf(el) : undefined);
        setScreen("transfers");
        toast(always ? `Đang nhận ${x.job} từ ${x.from_name} — lần sau tự nhận` : `Đang nhận ${x.job} từ ${x.from_name}…`);
      } else toast(`Đã từ chối ${x.job}`);
    } catch (e) { toast(errorMessage(e), "error"); }
    setBusy(null);
  }

  const preview = p.names.slice(0, 3);
  const more = Math.max(0, p.files - preview.length);

  return (
    <Sheet onClose={() => setOpen(false)} title={`${p.from_name} muốn gửi ${p.job}`}>
      <div className="flex items-center gap-3 mb-3">
        <div className="relative shrink-0">
          <DeviceAvatar icon={icon} size={52} />
          {!reduce && (
            <motion.span
              className="absolute inset-0 rounded-full border-2 border-accent pointer-events-none"
              initial={{ scale: 1, opacity: 0.8 }} animate={{ scale: 1.6, opacity: 0 }}
              transition={{ duration: 1.8, ease: "easeOut", repeat: Infinity }}
            />
          )}
        </div>
        <div className="min-w-0">
          <div className="flex items-center gap-1.5 mb-0.5">
            <span className="text-[11px] font-semibold uppercase tracking-[0.2em] text-mute">Yêu cầu gửi</span>
            {p.stranger && <Badge tone="warn">Máy lạ</Badge>}
            {p.source === "localsend" && <Badge>LocalSend</Badge>}
          </div>
          <div className="title text-[22px] leading-tight"><em className="block truncate">{p.from_name}</em>muốn gửi cho bạn</div>
        </div>
      </div>

      <div className="rounded-[16px] hair bg-line/[0.04] px-3.5 py-3 mb-3">
        <div className="flex items-center gap-3">
          <span className="grid place-items-center w-10 h-10 rounded-full bg-accent/15 text-accent shrink-0"><Icon name={p.files > 1 ? "folder" : "file"} className="w-5 h-5" /></span>
          <div className="min-w-0 flex-1">
            <div className="truncate text-base font-semibold tracking-tight" title={p.job}>{p.job}</div>
            <div className="text-sm text-mute mono">{askLine(p)}</div>
          </div>
        </div>
        {p.files > 1 && preview.length > 0 && (
          <div className="mt-2.5 pt-2.5 border-t border-line/10 text-xs text-mute mono truncate" title={p.names.join("\n")}>
            {preview.join(", ")}{more > 0 ? ` … +${more}` : ""}
          </div>
        )}
      </div>

      <div className="flex gap-2 mb-2">
        <button className="btn btn-primary btn-lg flex-1 magnet" disabled={busy !== null} onClick={(e) => answer(p, true, false, e.currentTarget)}>Nhận<Nest name="down" /></button>
        <button className="btn btn-lg btn-danger" disabled={busy !== null} onClick={() => answer(p, false, false)}>Từ chối</button>
      </div>
      <button className="btn w-full" disabled={busy !== null} onClick={(e) => answer(p, true, true, e.currentTarget)}>
        <Icon name="shield" />Luôn nhận từ máy này
      </button>
      {p.stranger && <div className="text-xs text-mute text-center mt-2">Máy lạ cùng mạng. “Luôn nhận” nhớ máy này: lần sau nhận luôn, không hỏi.</div>}

      {rest.length > 0 && (
        <div className="mt-3">
          <div className="text-xs text-mute px-1 mb-1.5">Còn {rest.length} yêu cầu đang chờ</div>
          <ul className="card divide-y divide-line/10 overflow-hidden">
            {rest.map((x) => (
              <li key={x.id} className="flex items-center gap-2.5 px-3 py-2">
                <div className="min-w-0 flex-1">
                  <div className="truncate text-sm font-medium">{x.job}</div>
                  <div className="truncate text-xs text-mute">{x.from_name} · {askLine(x)}</div>
                </div>
                <button className="btn btn-icon" aria-label={`Nhận ${x.job}`} disabled={busy !== null} onClick={(e) => answer(x, true, false, e.currentTarget)}><Icon name="check" weight="bold" /></button>
                <button className="btn btn-icon btn-danger" aria-label={`Từ chối ${x.job}`} disabled={busy !== null} onClick={() => answer(x, false, false)}><Icon name="x" weight="bold" /></button>
              </li>
            ))}
          </ul>
        </div>
      )}
      <button className="btn btn-ghost w-full mt-2" onClick={() => setOpen(false)}>Để sau</button>
    </Sheet>
  );
}

/** A text message from another device: read it, copy it. */
export function TextSheet() {
  const texts = useStore((s) => s.texts);
  const dismiss = useStore((s) => s.dismissText);
  const toast = useStore((s) => s.toast);
  const [copied, setCopied] = useState(false);
  const m = texts[0];
  useEffect(() => { setCopied(false); }, [m]);
  if (!m) return null;

  async function copy() {
    try {
      await api.copy_text({ text: m!.text });
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1800);
    } catch (e) { toast(errorMessage(e), "error"); }
  }

  return (
    <Sheet onClose={dismiss} title={`${m.from_name} gửi chữ`}>
      <div className="flex items-center gap-2.5 mb-3">
        <span className="grid place-items-center w-9 h-9 rounded-full bg-accent/15 text-accent shrink-0"><Icon name="chat" className="w-5 h-5" weight="bold" /></span>
        <div className="min-w-0">
          <div className="title text-[24px] leading-tight truncate"><em>{m.from_name}</em> gửi chữ</div>
          <div className="text-xs text-mute">{fmtAgo(m.at)}</div>
        </div>
      </div>
      <div className="selectable rounded-[16px] hair bg-line/[0.04] px-3.5 py-3 mb-3 max-h-56 overflow-y-auto whitespace-pre-wrap break-words text-base">{m.text}</div>
      <button className="btn btn-primary btn-lg w-full magnet" onClick={() => void copy()}>
        {copied ? "Đã chép" : "Sao chép"}<Nest name={copied ? "check" : "copy"} />
      </button>
      {texts.length > 1 && <div className="text-xs text-mute text-center mt-2">Còn {texts.length - 1} tin chữ nữa</div>}
      <button className="btn btn-ghost w-full mt-2" onClick={dismiss}>Đóng</button>
    </Sheet>
  );
}
