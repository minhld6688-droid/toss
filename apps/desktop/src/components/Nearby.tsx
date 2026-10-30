// v0.8 "Gần đây": every device the shell can see on this network (Toss and LocalSend), live.
import type React from "react";
import { useEffect, useState } from "react";
import { useReducedMotion } from "motion/react";
import type { Nearby } from "@contracts/ipc";
import { api, errorMessage, isMock } from "../lib/ipc";
import { PLATFORM_LABEL, utf8Bytes } from "../lib/format";
import { useStore } from "../lib/store";
import { centerOf } from "./Comet";
import { HowItWorks } from "./HowItWorks";
import { Badge, Dot, Icon, Nest, Sheet, motion } from "./ui";

/** Text messages are limited to 60 KiB by the core. */
export const MAX_TEXT_BYTES = 60 * 1024;

export function nearbyIcon(n: Pick<Nearby, "platform" | "kind">): string {
  if (n.platform === "ios" || n.platform === "android") return "phone";
  if (n.platform === "web") return "globe";
  if (n.platform === "macos") return "laptop";
  return "desktop";
}

/** Second line of a row: which app it is, and what kind of device. */
export function nearbySub(n: Nearby): string {
  const dev = n.model && n.model !== "Toss" ? n.model : PLATFORM_LABEL[n.platform] ?? "";
  const app = n.kind === "localsend" ? "LocalSend" : "Toss";
  return [app, dev].filter(Boolean).join(" · ");
}

/** Round device icon with the green "is here" light on its edge. */
export function DeviceAvatar({ icon, online = true, size = 40 }: { icon: string; online?: boolean; size?: number }) {
  return (
    <div className="relative shrink-0" style={{ width: size, height: size }}>
      <div className={`grid place-items-center w-full h-full rounded-full transition-colors duration-500 ease-aura ${online ? "bg-accent/15 text-accent" : "bg-line/10 text-mute"}`}>
        <Icon name={icon} className="w-[52%] h-[52%]" weight="bold" />
      </div>
      <span className="absolute -right-0.5 -bottom-0.5 grid place-items-center w-3.5 h-3.5 rounded-full bg-bg">
        <Dot tone={online ? "ok" : "mute"} />
      </span>
    </div>
  );
}

/** HTML5 drag & drop onto a row, mock mode only (the real shell reports native drops with a position). */
export function useRowDrop(key: string): React.HTMLAttributes<HTMLElement> {
  const setHover = useStore((s) => s.setHoverKey);
  const sendTo = useStore((s) => s.sendTo);
  if (!isMock) return {};
  return {
    onDragOver: (e) => { e.preventDefault(); setHover(key); },
    onDragLeave: () => setHover(null),
    onDrop: (e) => {
      e.preventDefault(); setHover(null);
      const names = Array.from(e.dataTransfer.files).map((f) => f.name);
      void sendTo(key, names.length ? names : ["Folder demo"], centerOf(e.currentTarget));
    },
  };
}

function NearbyRow({ n, i }: { n: Nearby; i: number }) {
  const dropHover = useStore((s) => s.dropHover);
  const hovered = useStore((s) => s.hoverKey === n.key);
  const openPeer = useStore((s) => s.openPeer);
  const drop = useRowDrop(n.key);
  const reduce = useReducedMotion();
  const target = dropHover && !hovered;
  return (
    <li className="rise" style={{ "--i": 1 + i } as React.CSSProperties}>
      <div
        role="button" tabIndex={0} data-send-key={n.key} aria-label={`${n.name}, ${nearbySub(n)}`}
        onClick={() => openPeer(n.key)}
        onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openPeer(n.key); } }}
        {...drop}
        className={`group relative flex items-center gap-3 px-3.5 py-2.5 cursor-pointer outline-none transition-[background-color,box-shadow] duration-500 ease-aura focus-visible:bg-line/[0.06] ${hovered ? "bg-accent/15 shadow-[inset_0_0_0_2px_rgb(var(--accent))]" : target ? "bg-accent/[0.06]" : "hover:bg-line/[0.05]"}`}
      >
        <motion.div animate={{ scale: hovered && !reduce ? 1.12 : 1 }} transition={reduce ? { duration: 0 } : { type: "spring", stiffness: 420, damping: 18 }}>
          <DeviceAvatar icon={nearbyIcon(n)} />
        </motion.div>
        <div className="min-w-0 flex-1">
          <div className="truncate text-base font-medium transition-transform duration-500 ease-aura group-hover:translate-x-0.5">{n.name}</div>
          <div className="text-xs text-mute truncate">{nearbySub(n)}</div>
        </div>
        {hovered ? (
          <span className="inline-flex items-center gap-1 text-xs font-semibold text-accent shrink-0"><Icon name="up" className="w-4 h-4 drip up" weight="bold" />Thả để gửi</span>
        ) : n.trusted ? (
          <Badge tone="ok">Tin cậy</Badge>
        ) : (
          <Icon name="chevron" className="w-4 h-4 text-mute -rotate-90 opacity-60 transition-all duration-500 ease-aura group-hover:opacity-100 group-hover:translate-x-0.5" />
        )}
      </div>
    </li>
  );
}

/** Contacts that are on this network show up in Danh bạ (online); this list is for everyone else. */
export function strangersNearby(nearby: Nearby[]): Nearby[] {
  return nearby
    .filter((n) => !n.contact_id)
    .sort((a, b) => Number(b.trusted) - Number(a.trusted) || a.name.localeCompare(b.name, "vi"));
}

export function NearbySection() {
  const nearby = useStore((s) => s.nearby);
  const loaded = useStore((s) => s.nearbyLoaded);
  const scanning = useStore((s) => s.scanning);
  const rescan = useStore((s) => s.rescanNearby);
  const dropHover = useStore((s) => s.dropHover);
  const reduce = useReducedMotion();
  const list = strangersNearby(nearby);

  return (
    <section className="mb-4 rise" style={{ "--i": 1 } as React.CSSProperties}>
      <div className="flex items-center justify-between mb-2">
        <h2 className="h-section !mb-0">Gần đây</h2>
        <button className="btn btn-ghost !h-8 !px-2.5 !text-xs" onClick={() => void rescan()} disabled={scanning} aria-label="Quét lại mạng" title="Quét lại mạng">
          <motion.span className="grid" animate={scanning && !reduce ? { rotate: 360 } : { rotate: 0 }} transition={scanning && !reduce ? { duration: 0.9, ease: "linear", repeat: Infinity } : { duration: 0.3 }}>
            <Icon name="refresh" className="w-4 h-4" />
          </motion.span>
          {scanning ? "Đang quét…" : "Quét lại"}
        </button>
      </div>
      {list.length === 0 ? (
        loaded && (
          <div className="rounded-[20px] border border-dashed border-line/25 bg-line/[0.04] px-4 py-4 text-center">
            <span className="mx-auto mb-2 grid place-items-center w-10 h-10 rounded-full bg-accent/15 text-accent"><Icon name="signal" className="w-5 h-5" weight="bold" /></span>
            <div className="text-base font-medium">Chưa thấy máy nào cùng mạng</div>
            <HowItWorks variant="line" className="mt-1.5 text-left" />
          </div>
        )
      ) : (
        <ul className={`card divide-y divide-line/10 overflow-hidden transition-shadow duration-500 ease-aura ${dropHover ? "shadow-[0_0_0_1.5px_rgb(var(--accent)/0.55)]" : ""}`}>
          {list.map((n, i) => <NearbyRow key={n.key} n={n} i={i} />)}
        </ul>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------------------------
// Text box shared by "Gửi chữ" everywhere
// ---------------------------------------------------------------------------------------------
export function TextBox({ value, onChange, autoFocus = true, rows = 4 }: { value: string; onChange: (v: string) => void; autoFocus?: boolean; rows?: number }) {
  const bytes = utf8Bytes(value);
  const over = bytes > MAX_TEXT_BYTES;
  return (
    <div>
      <textarea
        className={`input resize-none py-2 h-auto ${over ? "border-bad" : ""}`} rows={rows} autoFocus={autoFocus} spellCheck={false}
        placeholder="Gõ hoặc dán chữ cần gửi (link, mật khẩu wifi, ghi chú…)" value={value} onChange={(e) => onChange(e.target.value)}
        aria-label="Nội dung cần gửi"
      />
      {over && <div className="text-xs text-bad mt-1">Dài quá 60 KB — chia nhỏ hoặc gửi bằng file.</div>}
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// What you can do with one known device: file, folder or text. Used by both device sheets.
// ---------------------------------------------------------------------------------------------
export function SendActions({ targetKey, name }: { targetKey: string; name: string }) {
  const sendTo = useStore((s) => s.sendTo);
  const sendText = useStore((s) => s.sendText);
  const toast = useStore((s) => s.toast);
  const [mode, setMode] = useState<"menu" | "text">("menu");
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);

  async function pick(kind: "files" | "folder", e: React.MouseEvent) {
    const from = centerOf(e.currentTarget);
    try {
      if (kind === "files") {
        const p = await api.pick_files({ title: `Chọn file để gửi cho ${name}` });
        if (p.length > 0) await sendTo(targetKey, p, from);
      } else {
        const p = await api.pick_folder({ title: `Chọn folder để gửi cho ${name}` });
        if (p) await sendTo(targetKey, [p], from);
      }
    } catch (err) { toast(errorMessage(err), "error"); }
  }

  if (mode === "text") {
    const bad = !text.trim() || utf8Bytes(text) > MAX_TEXT_BYTES;
    return (
      <div className="space-y-2">
        <TextBox value={text} onChange={setText} />
        <div className="flex gap-2">
          <button className="btn btn-ghost" onClick={() => setMode("menu")}>Quay lại</button>
          <button
            className="btn btn-primary flex-1 magnet" disabled={bad || busy}
            onClick={async () => { setBusy(true); await sendText(targetKey, text); setBusy(false); }}
          >Gửi chữ<Nest /></button>
        </div>
      </div>
    );
  }
  return (
    <div className="space-y-2">
      <button className="btn btn-primary btn-lg w-full magnet" onClick={(e) => void pick("files", e)}>
        <Icon name="up" weight="bold" />Gửi file…<Nest />
      </button>
      <div className="flex gap-2">
        <button className="btn flex-1" onClick={(e) => void pick("folder", e)}><Icon name="folder" />Gửi folder…</button>
        <button className="btn flex-1" onClick={() => setMode("text")}><Icon name="chat" />Gửi chữ</button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Sheet for a nearby device that is not in the contact book
// ---------------------------------------------------------------------------------------------
export function PeerSheet({ k }: { k: string }) {
  const n = useStore((s) => s.nearby.find((x) => x.key === k));
  const close = useStore((s) => s.closePeer);
  useEffect(() => { if (!n) close(); }, [n, close]);
  if (!n) return null;
  return (
    <Sheet onClose={close} title={n.name}>
      <div className="flex items-center gap-3 mb-3">
        <DeviceAvatar icon={nearbyIcon(n)} size={52} />
        <div className="min-w-0">
          <div className="title text-[26px] leading-tight truncate">{n.name}</div>
          <div className="text-xs text-mute">{nearbySub(n)} · <span className="text-ok font-medium">cùng mạng</span></div>
        </div>
      </div>
      <SendActions targetKey={n.key} name={n.name} />
      <div className="text-xs text-mute text-center mt-2.5 mb-1">
        {n.trusted ? "Máy này được tin cậy: gửi tới là nhận luôn." : "Hoặc kéo file thả thẳng vào tên máy ở Trang chính."}
      </div>
      <button className="btn btn-ghost w-full mt-1" onClick={close}>Đóng</button>
    </Sheet>
  );
}
