// "Gửi tới ai?": one place to send, whatever brought it here — a drop on the window, the menu bar / Dock icon,
// right-click "Gửi bằng Toss", the file/folder buttons, or the text button / a paste ("Gửi chữ tới ai?").
// Sections: Gần đây (devices on this network) · Danh bạ · Tạo link (exactly one file or folder) · Chia sẻ qua trình duyệt.
import type React from "react";
import { useState } from "react";
import { useReducedMotion } from "motion/react";
import type { Contact } from "@contracts/ipc";
import { api, errorMessage } from "../lib/ipc";
import { baseName, midTruncate, utf8Bytes } from "../lib/format";
import { useStore } from "../lib/store";
import { Avatar } from "./Contacts";
import { centerOf } from "./Comet";
import { HowItWorks } from "./HowItWorks";
import { DeviceAvatar, MAX_TEXT_BYTES, nearbyIcon, nearbySub, strangersNearby, TextBox, useRowDrop } from "./Nearby";
import { stopWebShare, WebSharePanel } from "./WebShare";
import { Badge, Icon, Sheet, motion } from "./ui";

interface Choice { key: string; name: string; sub: React.ReactNode; avatar: React.ReactNode }

function ChoiceRow({ c, i, onPick, label = "Gửi" }: { c: Choice; i: number; onPick: (key: string, el: Element) => void; label?: string }) {
  const on = useStore((s) => s.hoverKey === c.key);
  const drop = useRowDrop(c.key);
  const reduce = useReducedMotion();
  return (
    <li className="rise" style={{ "--i": i } as React.CSSProperties}>
      <button
        type="button" data-send-key={c.key}
        onClick={(e) => onPick(c.key, e.currentTarget)}
        {...drop}
        className={`group w-full text-left flex items-center gap-3 px-3.5 py-2 transition-[background-color,box-shadow] duration-500 ease-aura active:scale-[0.99] ${on ? "bg-accent/15 shadow-[inset_0_0_0_2px_rgb(var(--accent))]" : "hover:bg-line/[0.05]"}`}
      >
        <motion.div animate={{ scale: on && !reduce ? 1.12 : 1 }} transition={reduce ? { duration: 0 } : { type: "spring", stiffness: 420, damping: 18 }}>{c.avatar}</motion.div>
        <div className="min-w-0 flex-1">
          <div className="truncate text-base font-medium">{c.name}</div>
          <div className="text-xs text-mute truncate">{c.sub}</div>
        </div>
        <span className={`text-xs font-semibold shrink-0 transition-opacity duration-500 ease-aura ${on ? "text-accent opacity-100" : "text-accent opacity-0 group-hover:opacity-100"}`}>{label}</span>
      </button>
    </li>
  );
}

function contactChoice(c: Contact): Choice {
  return { key: c.id, name: c.name, avatar: <Avatar c={c} size={34} />, sub: c.online ? <span className="text-ok">Online — nhận ngay</span> : "Offline — tự gửi khi máy online" };
}

/** Square-ish option under the lists ("Tạo link", "Chia sẻ qua trình duyệt"). */
function Tile({ icon, title, sub, onClick, disabled, badge }: { icon: string; title: string; sub: string; onClick: () => void; disabled?: boolean; badge?: React.ReactNode }) {
  return (
    <button
      type="button" onClick={onClick} disabled={disabled}
      className="group text-left rounded-[16px] hair bg-line/[0.04] hover:bg-line/[0.07] px-3 py-2.5 flex flex-col gap-1.5 transition-colors duration-500 ease-aura active:scale-[0.99] disabled:opacity-60"
    >
      <span className="flex items-center justify-between">
        <span className="grid place-items-center w-8 h-8 rounded-full bg-accent/15 text-accent"><Icon name={icon} className="w-4 h-4" weight="bold" /></span>
        {badge}
      </span>
      <span className="text-base font-medium leading-tight">{title}</span>
      <span className="text-xs text-mute leading-snug">{sub}</span>
    </button>
  );
}

export function SendPicker({ paths, text: initialText }: { paths: string[]; text?: string }) {
  const contacts = useStore((s) => s.contacts);
  const nearby = useStore((s) => s.nearby);
  const close = useStore((s) => s.closeSend);
  const sendTo = useStore((s) => s.sendTo);
  const sendText = useStore((s) => s.sendText);
  const openShare = useStore((s) => s.openShare);
  const openAdd = useStore((s) => s.openAddDevice);
  const toast = useStore((s) => s.toast);
  const setWebShare = useStore((s) => s.setWebShare);
  const webShare = useStore((s) => s.webShare);

  const textMode = initialText !== undefined;
  const [text, setText] = useState(initialText ?? "");
  const [starting, setStarting] = useState(false);
  const [shared, setShared] = useState(false);
  const textBad = textMode && (!text.trim() || utf8Bytes(text) > MAX_TEXT_BYTES);

  const first = paths[0] ?? "";
  const label = paths.length > 1 ? `${baseName(first)} và ${paths.length - 1} mục khác` : baseName(first);
  const near: Choice[] = strangersNearby(nearby).map((n) => ({
    key: n.key, name: n.name, avatar: <DeviceAvatar icon={nearbyIcon(n)} size={34} />, sub: nearbySub(n),
  }));
  // online first, then by name
  const book: Choice[] = [...contacts].sort((a, b) => Number(b.online) - Number(a.online) || a.name.localeCompare(b.name, "vi")).map(contactChoice);

  function pick(key: string, el: Element) {
    if (textMode) { if (!textBad) void sendText(key, text); return; }
    void sendTo(key, paths, centerOf(el));
  }

  async function startWebShare() {
    setStarting(true);
    try {
      const w = await api.web_share_start({ paths });
      setWebShare(w);
      setShared(true);
    } catch (e) { toast(errorMessage(e), "error"); }
    setStarting(false);
  }

  const list = (title: string, items: Choice[], from: number) => items.length > 0 && (
    <div className="mb-2.5">
      <h3 className="h-section !mb-1.5">{title}</h3>
      <ul className="card divide-y divide-line/10 overflow-hidden">{items.map((c, i) => <ChoiceRow key={c.key} c={c} i={from + i} onPick={pick} label={textMode ? "Gửi chữ" : "Gửi"} />)}</ul>
    </div>
  );

  return (
    <Sheet onClose={close} title={textMode ? "Gửi chữ tới ai?" : "Gửi tới ai?"}>
      <div className="flex items-center gap-2.5 mb-3">
        <span className="grid place-items-center w-9 h-9 rounded-full bg-accent/15 text-accent shrink-0"><Icon name={textMode ? "chat" : "up"} className="w-5 h-5" weight="bold" /></span>
        <div className="min-w-0">
          <div className="title text-[24px] leading-tight">{textMode ? "Gửi chữ tới " : "Gửi tới "}<em>ai?</em></div>
          {!textMode && <div className="text-sm text-mute truncate" title={paths.join("\n")}>{label}</div>}
        </div>
      </div>
      {!textMode && paths.length === 1 && (
        <div className="mono text-[11px] text-mute selectable whitespace-nowrap overflow-hidden -mt-1.5 mb-3 pl-[46px]" title={first}>{midTruncate(first, 22, 22)}</div>
      )}
      {textMode && <div className="mb-3"><TextBox value={text} onChange={setText} rows={3} /></div>}

      {list("Gần đây", near, 0)}
      {list("Danh bạ", book, near.length)}
      {near.length + book.length === 0 && (
        <div className="rounded-[16px] border border-dashed border-line/25 bg-line/[0.04] px-3.5 py-3 mb-2.5">
          <div className="flex items-center gap-3">
            <span className="grid place-items-center w-9 h-9 rounded-full bg-line/10 text-mute shrink-0"><Icon name="signal" className="w-[18px] h-[18px]" /></span>
            <div className="min-w-0 flex-1 text-sm">
              <div className="font-medium">Chưa thấy máy nào</div>
              <div className="text-xs text-mute">Máy ở xa thì mời một lần để ghép.</div>
            </div>
            <button className="btn !h-8 !px-3 shrink-0" onClick={() => openAdd("invite")}><Icon name="invite" />Mời</button>
          </div>
          <HowItWorks variant="line" className="mt-2" />
        </div>
      )}

      {!textMode && (shared && webShare ? (
        <WebSharePanel w={webShare} onStop={() => { setShared(false); void stopWebShare(); }} />
      ) : (
        <div className={`grid gap-2 ${paths.length === 1 ? "grid-cols-2" : "grid-cols-1"}`}>
          {paths.length === 1 && (
            <Tile icon="link" title="Tạo link" sub="Gửi qua Zalo, Messenger… cho người chưa có trong danh bạ" onClick={() => openShare(first)} />
          )}
          <Tile
            icon="globe" title="Chia sẻ qua trình duyệt" sub={starting ? "Đang bật…" : "Máy cùng wifi mở link hoặc quét mã là tải, không cần app"}
            onClick={() => void startWebShare()} disabled={starting} badge={webShare ? <Badge tone="ok">Đang chạy</Badge> : undefined}
          />
        </div>
      ))}

      <button className="btn btn-ghost w-full mt-3" onClick={close}>{shared ? "Xong" : "Huỷ"}</button>
    </Sheet>
  );
}
