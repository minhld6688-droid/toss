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
import { useUpdate } from "../lib/update";
import { Hint } from "../components/Hint";
import { motion, useReducedMotion } from "motion/react";

export function Home() {
  const openTicket = useStore((s) => s.openTicket);
  const recents = useStore((s) => s.recents);
  const dropHover = useStore((s) => s.dropHover);
  const toast = useStore((s) => s.toast);
  const openSend = useStore((s) => s.openSend);
  const pickAndSend = useStore((s) => s.pickAndSend);
  const receiveInvite = useStore((s) => s.receiveInvite);
  const deviceName = useStore((s) => s.app?.device_name ?? "");
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

  return (
    <div className="p-4 pb-2">
      <TopBar name={deviceName} />
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
            if (f) openSend([f.name]); else toast("Kéo một folder hoặc file vào đây");
          } : undefined}
        >
          <div className="pointer-events-none absolute -top-24 left-1/2 -translate-x-1/2 w-72 h-48 rounded-full bg-accent/20 blur-3xl" />
          <TossMark className={`relative mx-auto w-[180px] -mt-1 -mb-3 transition-transform duration-700 ease-aura ${dropHover ? "scale-110" : ""}`} speed={dropHover ? 1.8 : 1} />
          <Words as="div" text="Kéo folder vào để gửi" accent={["gửi"]} className="relative text-[24px] px-2" delay={0.1} />
          <div className="relative text-sm text-mute mt-2 mb-5">Thả vào tên một máy ở trên là gửi thẳng · thả ở đây để chọn người hoặc tạo link</div>
          <div className="relative flex flex-wrap justify-center gap-2">
            <button className="btn btn-primary btn-lg magnet" onClick={() => void pickAndSend()}><Icon name="up" weight="bold" />Gửi folder…<Nest /></button>
            <button className="btn btn-lg" onClick={pasteFromClipboard}><Icon name="down" />Dán link để nhận</button>
          </div>
        </div>
      </div>

      <div className="mt-3 rise" style={{ "--i": 1 } as React.CSSProperties}>
        <input
          ref={inputRef}
          className="input"
          placeholder="Ai gửi link cho bạn? Dán vào đây để nhận"
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
          <div className="text-sm text-mute px-1">Chưa có gì. Link bạn đã mở sẽ hiện ở đây để mở lại nhanh.</div>
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
                    {r.put && <Badge tone="ok">Gửi lại</Badge>}
                    <Icon name="go" className="w-4 h-4 text-mute -ml-1 opacity-0 -translate-x-1 transition-all duration-500 ease-aura group-hover:opacity-100 group-hover:translate-x-0" />
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </div>

      <HowItWorks />
    </div>
  );
}

/** Device name on the left, "Kiểm tra cập nhật" on the right (v0.6: always one click away). */
function TopBar({ name }: { name: string }) {
  const setScreen = useStore((s) => s.setScreen);
  const { info, check, runCheck } = useUpdate();
  const reduce = useReducedMotion();
  const avail = !!info?.available;
  const spinning = check === "checking";
  return (
    <div className="flex items-center justify-between gap-2 mb-3 -mt-1">
      <div className="inline-flex items-center gap-2 h-7 pl-2.5 pr-3 rounded-full text-xs text-mute bg-line/[0.06] hair min-w-0">
        <span className="pulse-dot w-1.5 h-1.5 rounded-full shrink-0 bg-ok" />
        <span className="truncate">{name || "Sẵn sàng"}</span>
      </div>
      <button
        className="btn btn-ghost !h-8 !px-2.5 !text-xs shrink-0 relative" title="Kiểm tra cập nhật" aria-label="Kiểm tra cập nhật"
        onClick={() => { setScreen("settings"); void runCheck(); }}
      >
        <motion.span className="grid" animate={spinning && !reduce ? { rotate: 360 } : { rotate: 0 }} transition={spinning && !reduce ? { duration: 1.1, ease: "linear", repeat: Infinity } : { duration: 0.3 }}>
          <Icon name="update" className="w-4 h-4" />
        </motion.span>
        Cập nhật
        {avail && <span className="pulse-dot absolute top-1 right-1 w-2 h-2 rounded-full bg-accent" />}
      </button>
    </div>
  );
}

/** Three lines, closed by default: how Toss works when every machine is the same. */
function HowItWorks() {
  const [open, setOpen] = useState(false);
  return (
    <div className="mt-6 rise" style={{ "--i": 4 } as React.CSSProperties}>
      <button className="h-section !mb-2 inline-flex items-center gap-1.5 hover:text-ink transition-colors" onClick={() => setOpen(!open)} aria-expanded={open}>
        <Icon name="info" className="w-3.5 h-3.5" />Toss dùng thế nào?
        <Icon name="chevron" className={`w-3.5 h-3.5 transition-transform duration-500 ease-aura ${open ? "rotate-180" : ""}`} />
      </button>
      {open && (
        <ol className="card px-3.5 py-3 space-y-2.5 text-sm">
          <li className="flex gap-2.5"><span className="mono text-accent font-semibold">1</span><span><b className="font-medium">Gửi:</b> kéo folder thả vào tên một máy trong danh bạ{navigator.platform.toLowerCase().includes("mac") ? " — hoặc thả lên icon Toss trên thanh menu" : ""}. Máy nào cũng gửi và nhận như nhau.</span></li>
          <li className="flex gap-2.5"><span className="mono text-accent font-semibold">2</span><span><b className="font-medium">Người lạ:</b> chọn “Tạo link”, gửi link cho họ. Họ bấm link là nhận; bật “Cho gửi lại” thì họ thả file vào để gửi ngược về.</span></li>
          <li className="flex gap-2.5"><span className="mono text-accent font-semibold">3</span><span><b className="font-medium">Thêm máy:</b> bấm “Thêm máy”, gửi lời mời một lần. Từ đó hai máy thấy nhau trong danh bạ.</span></li>
          <li><Hint>File nhận về nằm trong thư mục nhận (Cài đặt → Máy này). Truyền thẳng giữa hai máy, không qua máy chủ.</Hint></li>
        </ol>
      )}
    </div>
  );
}
