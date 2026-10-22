import { useEffect, useRef, useState } from "react";
import { extractTicket } from "../lib/ticket";
import { fmtExpiry } from "../lib/format";
import { useStore } from "../lib/store";
import { Badge, Icon, Nest } from "../components/ui";
import { api, errorMessage, isMock } from "../lib/ipc";

export function Home() {
  const openTicket = useStore((s) => s.openTicket);
  const recents = useStore((s) => s.recents);
  const dropHover = useStore((s) => s.dropHover);
  const toast = useStore((s) => s.toast);
  const openShare = useStore((s) => s.openShare);
  const askShare = useStore((s) => s.askShare);
  const [text, setText] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  async function handle(t: string): Promise<boolean> {
    const found = extractTicket(t);
    if (!found) return false;
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
      <div className={`shell rise transition-colors duration-500 ease-aura ${dropHover ? "bg-accent/10" : ""}`}>
        <div
          className={`core relative overflow-hidden px-4 py-7 text-center transition-colors duration-500 ease-aura ${dropHover ? "border-accent/60" : ""}`}
          onDragOver={isMock ? (e) => e.preventDefault() : undefined}
          onDrop={isMock ? (e) => {
            e.preventDefault();
            const f = e.dataTransfer.files[0];
            if (f) askShare(f.name); else toast("Dán link trước, rồi kéo folder vào để trả");
          } : undefined}
        >
          <div className="pointer-events-none absolute -top-24 left-1/2 -translate-x-1/2 w-72 h-48 rounded-full bg-accent/20 blur-3xl" />
          <div className="relative mx-auto w-14 h-14 rounded-full grid place-items-center mb-3 bg-accent/12 text-accent" style={{ boxShadow: "inset 0 1px 0 rgb(255 255 255 / 0.2), 0 0 0 6px rgb(var(--accent) / 0.06)" }}>
            <Icon name="link" className="w-6 h-6" />
          </div>
          <div className="relative text-lg font-semibold tracking-tight">Dán link hoặc kéo folder vào đây</div>
          <div className="relative text-sm text-mute mt-1 mb-4">Link để nhận/trả file · folder để tạo link chia sẻ</div>
          <div className="relative flex flex-wrap justify-center gap-2">
            <button className="btn btn-primary" onClick={pasteFromClipboard}>Dán từ clipboard<Nest name="copy" /></button>
            <button className="btn" onClick={shareFolder}><Icon name="share" />Chia sẻ folder…</button>
          </div>
        </div>
      </div>

      <div className="mt-3">
        <input
          ref={inputRef}
          className="input"
          placeholder="…hoặc dán link / mã toss1… vào đây"
          value={text}
          spellCheck={false}
          onChange={(e) => {
            setText(e.target.value);
            if (extractTicket(e.target.value)) void handle(e.target.value);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && text.trim()) void handle(text).then((ok) => { if (!ok) toast("Không phải link Toss", "error"); });
          }}
        />
      </div>

      <div className="mt-6">
        <h2 className="h-section">Gần đây</h2>
        {recents.length === 0 ? (
          <div className="text-sm text-mute px-1">Chưa có job nào. Link bạn mở sẽ hiện ở đây.</div>
        ) : (
          <ul className="card divide-y divide-line/10 overflow-hidden rise">
            {recents.map((r) => {
              const exp = fmtExpiry(r.exp);
              return (
                <li key={r.ticket}>
                  <button className="w-full text-left px-3.5 py-2.5 hover:bg-line/[0.06] transition-colors duration-500 ease-aura flex items-center gap-2" onClick={() => void openTicket(r.ticket)}>
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-base font-medium">{r.job}</div>
                      <div className={`text-xs ${exp.expired ? "text-bad" : "text-mute"}`}>{exp.text}</div>
                    </div>
                    {r.get && <Badge tone="accent">Nhận</Badge>}
                    {r.put && <Badge tone="ok">Trả</Badge>}
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
