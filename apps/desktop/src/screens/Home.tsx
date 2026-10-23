import type React from "react";
import { useEffect, useRef, useState } from "react";
import { extractInvite, extractShort, extractTicket } from "../lib/ticket";
import { fmtExpiry } from "../lib/format";
import { useStore } from "../lib/store";
import { Badge, Icon, Nest, Words } from "../components/ui";
import { TossMark } from "../components/TossMark";
import { ContactsSection } from "../components/Contacts";
import { LanBlockedCard } from "../components/LanBlocked";
import { UpdateBanner } from "../components/Update";
import { api, errorMessage, isMock } from "../lib/ipc";

export function Home() {
  const openTicket = useStore((s) => s.openTicket);
  const recents = useStore((s) => s.recents);
  const dropHover = useStore((s) => s.dropHover);
  const toast = useStore((s) => s.toast);
  const openShare = useStore((s) => s.openShare);
  const askShare = useStore((s) => s.askShare);
  const receiveInvite = useStore((s) => s.receiveInvite);
  const deviceName = useStore((s) => s.app?.device_name ?? "");
  const hostOn = useStore((s) => !!s.app?.host);
  const [text, setText] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  async function handle(t: string): Promise<boolean> {
    const inv = extractInvite(t);
    if (inv) { receiveInvite(inv); setText(""); return true; }
    const found = extractTicket(t);
    if (!found) {
      const short = extractShort(t);
      if (!short) return false;
      try {
        const long = await api.expand_link({ text: short });
        if (extractTicket(long) || extractInvite(long)) return handle(long);
      } catch (e) {
        toast(errorMessage(e), "error");
      }
      return true;
    }
    const ok = await openTicket(t);
    if (ok) setText("");
    return ok;
  }

  // Paste anywhere on the page (when no text field is focused).
  useEffect(() => {
    const h = (e: ClipboardEvent) => {
      if (document.activeElement === inputRef.current) return;
      const t = e.clipboardData?.getData("text") ?? "";
      if (t) void handle(t).then((ok) => { if (!ok) toast("Không thấy link Toss trong nội dung vừa dán", "error"); });
    };
    window.addEventListener("paste", h);
    return () => window.removeEventListener("paste", h);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function pasteFromClipboard() {
    try {
      const t = await navigator.clipboard.readText();
      if (!(await handle(t))) toast("Clipboard không có link Toss", "error");
    } catch {
      inputRef.current?.focus();
      toast("Nhấn Ctrl/⌘+V để dán link", "info");
    }
  }

  async function shareFolder() {
    try {
      const p = await api.pick_folder({ title: "Chọn folder để chia sẻ" });
      if (p) openShare(p);
    } catch (e) { toast(errorMessage(e), "error"); }
  }

  return (
    <div className="p-4 pb-2">
      <UpdateBanner />
      <LanBlockedCard className="mb-4" />
      <ContactsSection />
      <div className={`shell rise transition-colors duration-500 ease-aura ${dropHover ? "bg-accent/10" : ""}`}>
        <div
          className={`core relative overflow-hidden px-4 pt-5 pb-6 text-center transition-colors duration-500 ease-aura ${dropHover ? "border-accent/60" : ""}`}
          onDragOver={isMock ? (e) => e.preventDefault() : undefined}
          onDrop={isMock ? (e) => {
            e.preventDefault();
            const f = e.dataTransfer.files[0];
            if (f) askShare(f.name); else toast("Dán link trước, rồi kéo folder vào để trả");
          } : undefined}
        >
          <div className="pointer-events-none absolute -top-24 left-1/2 -translate-x-1/2 w-72 h-48 rounded-full bg-accent/20 blur-3xl" />
          <div className="relative inline-flex items-center gap-2 h-6 pl-2 pr-2.5 rounded-full text-[10px] font-semibold uppercase tracking-[0.2em] text-mute bg-line/[0.06] hair max-w-full">
            <span className={`pulse-dot w-1.5 h-1.5 rounded-full shrink-0 ${hostOn ? "bg-warn" : "bg-ok"}`} />
            <span className="truncate">{hostOn ? "Máy tổng đang mở" : deviceName || "Sẵn sàng"}</span>
          </div>
          <TossMark className={`relative mx-auto w-[180px] -mt-1 -mb-3 transition-transform duration-700 ease-aura ${dropHover ? "scale-110" : ""}`} speed={dropHover ? 1.8 : 1} />
          <Words as="div" text="Dán link hoặc kéo folder vào đây" accent={["link", "folder"]} className="relative text-[24px] px-2" delay={0.1} />
          <div className="relative text-sm text-mute mt-2 mb-5">Link để nhận/trả file · folder để tạo link chia sẻ</div>
          <div className="relative flex flex-wrap justify-center gap-2">
            <button className="btn btn-primary btn-lg magnet" onClick={pasteFromClipboard}>Dán từ clipboard<Nest name="copy" /></button>
            <button className="btn btn-lg" onClick={shareFolder}><Icon name="share" />Chia sẻ folder…</button>
          </div>
        </div>
      </div>

      <div className="mt-3 rise" style={{ "--i": 1 } as React.CSSProperties}>
        <input
          ref={inputRef}
          className="input"
          placeholder="…hoặc dán link / mã toss1… / lời mời vào đây"
          value={text}
          spellCheck={false}
          onChange={(e) => {
            setText(e.target.value);
            if (extractTicket(e.target.value) || extractInvite(e.target.value) || extractShort(e.target.value)) void handle(e.target.value);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && text.trim()) void handle(text).then((ok) => { if (!ok) toast("Không phải link Toss", "error"); });
          }}
        />
      </div>

      <div className="mt-6 rise" style={{ "--i": 2 } as React.CSSProperties}>
        <h2 className="h-section">Gần đây</h2>
        {recents.length === 0 ? (
          <div className="text-sm text-mute px-1">Chưa có job nào. Link bạn mở sẽ hiện ở đây.</div>
        ) : (
          <ul className="card divide-y divide-line/10 overflow-hidden">
            {recents.map((r, i) => {
              const exp = fmtExpiry(r.exp);
              return (
                <li key={r.ticket} className="rise" style={{ "--i": 3 + i } as React.CSSProperties}>
                  <button className="group w-full text-left px-3.5 py-2.5 hover:bg-line/[0.05] transition-colors duration-500 ease-aura flex items-center gap-2" onClick={() => void openTicket(r.ticket)}>
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-base font-medium transition-transform duration-500 ease-aura group-hover:translate-x-1">{r.job}</div>
                      <div className={`text-xs ${exp.expired ? "text-bad" : "text-mute"}`}>{exp.text}</div>
                    </div>
                    {r.get && <Badge tone="accent">Nhận</Badge>}
                    {r.put && <Badge tone="ok">Trả</Badge>}
                    <Icon name="go" className="w-4 h-4 text-mute -ml-1 opacity-0 -translate-x-1 transition-all duration-500 ease-aura group-hover:opacity-100 group-hover:translate-x-0" />
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </div>
  );
}
