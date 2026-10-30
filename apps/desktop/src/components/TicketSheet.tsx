import { useState } from "react";
import type { TicketInfo } from "@contracts/ipc";
import { api, errorMessage, isMock } from "../lib/ipc";
import { fmtExpiry, shortId } from "../lib/format";
import { useStore } from "../lib/store";
import { Badge, Icon, Nest, Sheet } from "./ui";
import { centerOf } from "./Comet";

export function TicketSheet({ info }: { info: TicketInfo }) {
  const close = useStore((s) => s.closeSheet);
  const setScreen = useStore((s) => s.setScreen);
  const toast = useStore((s) => s.toast);
  const refresh = useStore((s) => s.refreshTransfers);
  const dropHover = useStore((s) => s.dropHover);
  const note = useStore((s) => s.sheetNote);
  const doneDest = useStore((s) => Object.values(s.transfers).find((t) => t.op === "get" && t.status === "done" && t.job === info.job && t.dest)?.dest ?? null);
  const [busy, setBusy] = useState(false);
  const exp = fmtExpiry(info.exp);

  const fly = useStore((s) => s.fly);

  async function run(fn: () => Promise<unknown>, okText: string, from?: Element | null) {
    setBusy(true);
    const origin = from ? centerOf(from) : undefined;
    try {
      await fn();
      await refresh();
      fly(origin);
      close();
      setScreen("transfers");
      toast(okText);
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      setBusy(false);
    }
  }

  const doGet = (e?: { currentTarget: Element }) => run(() => api.start_get({ ticket: info.ticket }), "Đang nhận file…", e?.currentTarget);
  const doPut = (paths: string[], from?: Element | null) => run(() => api.start_put({ ticket: info.ticket, paths }), "Đang gửi lại file…", from);
  async function pick() {
    const p = await api.pick_folder({ title: "Chọn folder để gửi lại" });
    if (p) await doPut([p]);
  }

  return (
    <Sheet onClose={close} title={info.job}>
      <div className="flex items-start justify-between gap-2 mb-1">
        <div className="min-w-0">
          <div className="title text-[26px] break-words">{info.job}</div>
          <div className="text-xs text-mute">Từ máy <span className="mono">{shortId(info.host, 8)}</span></div>
        </div>
        <div className="flex gap-1 shrink-0">
          {info.get && <Badge tone="accent">Nhận</Badge>}
          {info.put && <Badge tone="ok">Gửi lại</Badge>}
        </div>
      </div>
      <div className="text-xs mb-3"><span className={exp.expired ? "text-bad" : "text-mute"}>{exp.text}</span></div>

      {exp.expired && <div className="rounded-[12px] bg-bad/10 text-bad text-sm px-3 py-2 mb-3">Link đã hết hạn. Nhờ người gửi tạo link mới.</div>}

      {note && (
        <div className={`rounded-[12px] text-sm px-3 py-2 mb-3 ${note.kind === "error" ? "bg-bad/10 text-bad" : "bg-ok/10 text-ok"}`}>{note.text}</div>
      )}

      {info.get && (
        <button className="btn btn-primary btn-lg w-full magnet" disabled={busy || exp.expired} onClick={doGet}>
          {note?.kind === "exists" ? "Tải lại" : "Nhận"}<Nest name={note?.kind === "exists" ? "refresh" : "down"} />
        </button>
      )}

      {note?.kind === "exists" && doneDest && (
        <button className="btn w-full mt-2" onClick={() => api.open_path({ path: doneDest }).catch((e) => toast(errorMessage(e), "error"))}><Icon name="folder" />Mở folder</button>
      )}

      {info.put && (
        <div
          className={`mt-3 rounded-[18px] border border-dashed p-4 text-center transition-colors duration-500 ease-aura ${dropHover ? "border-accent bg-accent/10" : "border-line/25 bg-line/[0.04]"} ${exp.expired ? "opacity-50 pointer-events-none" : ""}`}
          onDragOver={isMock ? (e) => e.preventDefault() : undefined}
          onDrop={isMock ? (e) => {
            e.preventDefault();
            const names = Array.from(e.dataTransfer.files).map((f) => f.name);
            void doPut(names.length ? names : ["Folder demo"]);
          } : undefined}
        >
          <span className={`mx-auto mb-2 grid place-items-center w-10 h-10 rounded-full bg-ok/15 text-ok transition-transform duration-500 ${dropHover ? "drip up scale-110" : ""}`} style={{ transitionTimingFunction: "var(--spring)" }}><Icon name="up" className="w-5 h-5" weight="bold" /></span>
          <div className="text-base font-medium">Kéo folder vào để gửi lại</div>
          <div className="text-xs text-mute mb-2">Người tạo link cho phép bạn gửi file ngược về máy họ — thả vào cửa sổ này</div>
          <button className="btn" disabled={busy} onClick={pick}><Icon name="folder" />Chọn folder…</button>
        </div>
      )}

      {!info.get && !info.put && <div className="text-sm text-mute">Link này không cho nhận hay gửi lại.</div>}
      <button className="btn btn-ghost w-full mt-2" onClick={close}>Đóng</button>
    </Sheet>
  );
}
