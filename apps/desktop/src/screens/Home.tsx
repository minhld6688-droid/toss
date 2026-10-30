import type React from "react";
import { useEffect, useRef, useState } from "react";
import { extractInvite, extractShort, extractTicket } from "../lib/ticket";
import { fmtExpiry } from "../lib/format";
import { useStore, type Recent } from "../lib/store";
import { Badge, Icon, Words } from "../components/ui";
import { TossMark } from "../components/TossMark";
import { ContactsSection } from "../components/Contacts";
import { NearbySection } from "../components/Nearby";
import { IncomingBanner } from "../components/Incoming";
import { WebShareCard } from "../components/WebShare";
import { HowItWorksFold } from "../components/HowItWorks";
import { FirewallNotice } from "../components/Firewall";
import { LanBlockedCard } from "../components/LanBlocked";
import { UpdateBanner } from "../components/Update";
import { api, errorMessage, isMock } from "../lib/ipc";
import { useUpdate } from "../lib/update";
import { motion, useReducedMotion } from "motion/react";

/** Longest pasted text we offer to send as a message (the core limit is 60 KiB). */
const MAX_PASTE_CHARS = 20000;

export function Home() {
  const openTicket = useStore((s) => s.openTicket);
  const recents = useStore((s) => s.recents);
  const dropHover = useStore((s) => s.dropHover);
  const toast = useStore((s) => s.toast);
  const openSend = useStore((s) => s.openSend);
  const openSendText = useStore((s) => s.openSendText);
  const pickAndSend = useStore((s) => s.pickAndSend);
  const receiveInvite = useStore((s) => s.receiveInvite);
  const refreshNearby = useStore((s) => s.refreshNearby);
  const deviceName = useStore((s) => s.app?.device_name ?? "");
  const [text, setText] = useState("");

  // the list is live (nearby:changed); this only covers the first paint and coming back to the tab
  useEffect(() => { void refreshNearby(); }, [refreshNearby]);

  /** A Toss link, invite or short link. Resolves false when the text is none of them. */
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

  // Paste anywhere on the page (when no text field is focused): a Toss link opens, any other text can be sent as a message.
  useEffect(() => {
    const h = (e: ClipboardEvent) => {
      const ae = document.activeElement;
      if (ae && (ae.tagName === "INPUT" || ae.tagName === "TEXTAREA")) return;
      const s = useStore.getState();
      if (s.sheet || s.send || s.share || s.contactSheet || s.peerSheet || s.addDevice || s.incomingSheet || s.texts.length > 0) return;
      const t = e.clipboardData?.getData("text") ?? "";
      if (!t.trim()) return;
      void handle(t).then((ok) => { if (!ok && t.length <= MAX_PASTE_CHARS) useStore.getState().openSendText(t); });
    };
    window.addEventListener("paste", h);
    return () => window.removeEventListener("paste", h);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className="p-4 pb-2">
      <TopBar name={deviceName} />
      <UpdateBanner />
      <LanBlockedCard className="mb-4" />
      <FirewallNotice className="mb-4" />
      <IncomingBanner className="mb-4" />
      <WebShareCard className="mb-4" />

      <div className={`shell rise mb-4 transition-colors duration-500 ease-aura ${dropHover ? "bg-accent/10" : ""}`}>
        <div
          className={`core relative overflow-hidden px-4 pt-3 pb-4 text-center transition-colors duration-500 ease-aura ${dropHover ? "border-accent/60" : ""}`}
          onDragOver={isMock ? (e) => e.preventDefault() : undefined}
          onDrop={isMock ? (e) => {
            e.preventDefault();
            const f = e.dataTransfer.files[0];
            if (f) openSend([f.name]); else toast("Kéo một folder hoặc file vào đây");
          } : undefined}
        >
          <div className="pointer-events-none absolute -top-24 left-1/2 -translate-x-1/2 w-72 h-48 rounded-full bg-accent/20 blur-3xl" />
          <TossMark className={`relative mx-auto w-[124px] -mt-1 -mb-2 transition-transform duration-700 ease-aura ${dropHover ? "scale-110" : ""}`} speed={dropHover ? 1.8 : 1} />
          <Words as="div" text="Kéo vào đây để gửi" accent={["gửi"]} className="relative text-[22px] px-2" delay={0.1} />
          <div className="relative text-xs text-mute mt-1.5 mb-3.5">Thả vào tên một máy bên dưới là gửi luôn</div>
          <div className="relative flex justify-center gap-2 [&>.btn]:whitespace-nowrap">
            <button className="btn btn-primary !px-3.5 magnet" onClick={() => void pickAndSend("files")}><Icon name="up" weight="bold" />Gửi file…</button>
            <button className="btn !px-3.5" onClick={() => void pickAndSend("folder")}><Icon name="folder" />Folder…</button>
            <button className="btn !px-3.5" onClick={() => openSendText()}><Icon name="chat" />Gửi chữ</button>
          </div>
        </div>
      </div>

      <NearbySection />
      <ContactsSection />
      <PasteField onHandle={handle} text={text} setText={setText} />
      <RecentLinks recents={recents} onOpen={(t) => void openTicket(t)} />
      <HowItWorksFold className="mt-6 rise" />
    </div>
  );
}

/** The one field that takes a link, an invite or a short link (tossinv1… / toss1… / https://…/abc). */
function PasteField({ onHandle, text, setText }: { onHandle: (t: string) => Promise<boolean>; text: string; setText: (t: string) => void }) {
  const toast = useStore((s) => s.toast);
  const inputRef = useRef<HTMLInputElement>(null);
  return (
    <div className="mt-5 rise" style={{ "--i": 4 } as React.CSSProperties}>
      <h2 className="h-section">Có link hoặc lời mời?</h2>
      <input
        ref={inputRef} className="input" aria-label="Dán link hoặc lời mời"
        placeholder="Dán link nhận file hoặc lời mời vào đây"
        value={text} spellCheck={false}
        onChange={(e) => {
          setText(e.target.value);
          if (extractTicket(e.target.value) || extractInvite(e.target.value) || extractShort(e.target.value)) void onHandle(e.target.value);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" && text.trim()) void onHandle(text).then((ok) => { if (!ok) toast("Không phải link Toss", "error"); });
        }}
      />
    </div>
  );
}

/** Links opened before (a "gửi lại" link keeps working until it expires): three at most, and only when there are some. */
function RecentLinks({ recents, onOpen }: { recents: Recent[]; onOpen: (ticket: string) => void }) {
  if (recents.length === 0) return null;
  return (
    <div className="mt-5 rise" style={{ "--i": 5 } as React.CSSProperties}>
      <h2 className="h-section">Link đã mở</h2>
      <ul className="card divide-y divide-line/10 overflow-hidden">
        {recents.slice(0, 3).map((r) => {
          const exp = fmtExpiry(r.exp);
          return (
            <li key={r.ticket}>
              <button className="group w-full text-left px-3.5 py-2.5 hover:bg-line/[0.05] transition-colors duration-500 ease-aura flex items-center gap-2" onClick={() => onOpen(r.ticket)}>
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
    </div>
  );
}

/** Device name on the left, "Kiểm tra cập nhật" on the right (always one click away). */
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
