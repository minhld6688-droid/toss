// v0.6 "Gửi tới ai?": one place to send files, whatever brought them here — a drop on the
// window, the menu bar / Dock icon, right-click "Gửi bằng Toss", or the "Gửi folder…" button.
// Every machine is the same: pick a person from the contact book, or make a link.
import type React from "react";
import { useReducedMotion } from "motion/react";
import { baseName, midTruncate } from "../lib/format";
import { isMock } from "../lib/ipc";
import { useStore } from "../lib/store";
import { Avatar } from "./Contacts";
import { centerOf } from "./Comet";
import { Badge, Icon, Sheet, motion } from "./ui";
import { Hint } from "./Hint";

export function SendPicker({ paths }: { paths: string[] }) {
  const contacts = useStore((s) => s.contacts);
  const hover = useStore((s) => s.hoverContact);
  const close = useStore((s) => s.closeSend);
  const sendTo = useStore((s) => s.sendToContact);
  const openShare = useStore((s) => s.openShare);
  const openAdd = useStore((s) => s.openAddDevice);
  const setHover = useStore((s) => s.setHoverContact);
  const reduce = useReducedMotion();

  const first = paths[0] ?? "";
  const label = paths.length > 1 ? `${baseName(first)} và ${paths.length - 1} mục khác` : baseName(first);
  // online first, then by name
  const list = [...contacts].sort((a, b) => Number(b.online) - Number(a.online) || a.name.localeCompare(b.name, "vi"));

  return (
    <Sheet onClose={close} title="Gửi tới ai?">
      <div className="flex items-center gap-2.5 mb-3">
        <span className="grid place-items-center w-9 h-9 rounded-full bg-accent/15 text-accent shrink-0"><Icon name="up" className="w-5 h-5" weight="bold" /></span>
        <div className="min-w-0">
          <div className="title text-[24px] leading-tight">Gửi tới <em>ai?</em></div>
          <div className="text-sm text-mute truncate" title={paths.join("\n")}>{label}</div>
        </div>
      </div>
      {paths.length === 1 && (
        <div className="mono text-[11px] text-mute selectable whitespace-nowrap overflow-hidden -mt-1.5 mb-3 pl-[46px]" title={first}>{midTruncate(first, 22, 22)}</div>
      )}

      {list.length > 0 ? (
        <ul className="card divide-y divide-line/10 overflow-hidden mb-2.5">
          {list.map((c, i) => {
            const on = hover === c.id;
            return (
              <li key={c.id} className="rise" style={{ "--i": i } as React.CSSProperties}>
                <button
                  type="button" data-contact-id={c.id}
                  onClick={(e) => void sendTo(c.id, paths, centerOf(e.currentTarget))}
                  onDragOver={isMock ? (e) => { e.preventDefault(); setHover(c.id); } : undefined}
                  onDragLeave={isMock ? () => setHover(null) : undefined}
                  className={`group w-full text-left flex items-center gap-3 px-3.5 py-2.5 transition-[background-color,box-shadow] duration-500 ease-aura active:scale-[0.99] ${on ? "bg-accent/15 shadow-[inset_0_0_0_2px_rgb(var(--accent))]" : "hover:bg-line/[0.05]"}`}
                >
                  <motion.div animate={{ scale: on && !reduce ? 1.12 : 1 }} transition={reduce ? { duration: 0 } : { type: "spring", stiffness: 420, damping: 18 }}>
                    <Avatar c={c} size={36} />
                  </motion.div>
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-base font-medium">{c.name}</div>
                    <div className="text-xs text-mute truncate">{c.online ? <span className="text-ok">Online — nhận ngay</span> : "Offline — tự gửi khi máy online"}</div>
                  </div>
                  <span className={`text-xs font-semibold shrink-0 transition-opacity duration-500 ease-aura ${on ? "text-accent opacity-100" : "text-accent opacity-0 group-hover:opacity-100"}`}>Gửi</span>
                </button>
              </li>
            );
          })}
        </ul>
      ) : (
        <div className="rounded-[16px] border border-dashed border-line/25 bg-line/[0.04] px-3.5 py-3 mb-2.5 flex items-center gap-3">
          <span className="grid place-items-center w-9 h-9 rounded-full bg-line/10 text-mute shrink-0"><Icon name="contact" className="w-[18px] h-[18px]" /></span>
          <div className="min-w-0 flex-1 text-sm">
            <div className="font-medium">Danh bạ đang trống</div>
            <div className="text-xs text-mute">Thêm máy một lần, lần sau chỉ cần thả vào tên.</div>
          </div>
          <button className="btn !h-8 !px-3 shrink-0" onClick={() => openAdd("invite")}><Icon name="plus" weight="bold" />Thêm</button>
        </div>
      )}

      <button
        type="button" onClick={() => openShare(first)}
        className="group w-full text-left rounded-[16px] hair bg-line/[0.04] hover:bg-line/[0.07] px-3.5 py-2.5 flex items-center gap-3 transition-colors duration-500 ease-aura active:scale-[0.99]"
      >
        <span className="grid place-items-center w-9 h-9 rounded-full bg-accent/15 text-accent shrink-0"><Icon name="link" className="w-[18px] h-[18px]" weight="bold" /></span>
        <div className="min-w-0 flex-1">
          <div className="text-base font-medium">Tạo link</div>
          <div className="text-xs text-mute">Cho người chưa có trong danh bạ — gửi link qua Zalo, Messenger…</div>
        </div>
        {paths.length > 1 && <Badge>Chỉ mục đầu</Badge>}
        <Icon name="go" className="w-4 h-4 text-mute shrink-0 transition-transform duration-500 ease-aura group-hover:translate-x-0.5" />
      </button>

      <Hint className="mt-3">Bấm vào tên để gửi, hoặc kéo file thả thẳng vào tên. Máy offline thì file chờ sẵn và tự đi khi máy đó mở Toss.</Hint>
      <button className="btn btn-ghost w-full mt-2" onClick={close}>Huỷ</button>
    </Sheet>
  );
}
