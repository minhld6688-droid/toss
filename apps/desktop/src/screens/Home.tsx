import { useEffect, useRef, useState } from "react";
import { extractTicket } from "../lib/ticket";
import { fmtExpiry } from "../lib/format";
import { useStore } from "../lib/store";
import { Badge, Icon } from "../components/ui";
import { isMock } from "../lib/ipc";

export function Home() {
  const openTicket = useStore((s) => s.openTicket);
  const recents = useStore((s) => s.recents);
  const dropHover = useStore((s) => s.dropHover);
  const toast = useStore((s) => s.toast);
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

  return (
    <div className="p-4 pb-2">
      <div
        className={`rounded-xl border-2 border-dashed px-4 py-7 text-center transition-colors ${dropHover ? "border-accent bg-accent/10" : "border-line bg-card"}`}
        onDragOver={isMock ? (e) => e.preventDefault() : undefined}
        onDrop={isMock ? (e) => { e.preventDefault(); toast("Dán link trước, rồi kéo folder vào để trả"); } : undefined}
      >
        <div className="mx-auto w-11 h-11 rounded-full bg-accent/10 text-accent grid place-items-center mb-3">
          <Icon name="link" className="w-5 h-5" />
        </div>
        <div className="text-lg font-semibold">Dán link hoặc kéo folder vào đây</div>
        <div className="text-sm text-mute mt-1 mb-3">Link nhận hoặc trả file từ máy tổng</div>
        <button className="btn btn-primary" onClick={pasteFromClipboard}>Dán từ clipboard</button>
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

      <div className="mt-5">
        <h2 className="h-section">Gần đây</h2>
        {recents.length === 0 ? (
          <div className="text-sm text-mute px-1">Chưa có job nào. Link bạn mở sẽ hiện ở đây.</div>
        ) : (
          <ul className="card divide-y divide-line overflow-hidden">
            {recents.map((r) => {
              const exp = fmtExpiry(r.exp);
              return (
                <li key={r.ticket}>
                  <button className="w-full text-left px-3 py-2 hover:bg-bg flex items-center gap-2" onClick={() => void openTicket(r.ticket)}>
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-base">{r.job}</div>
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
