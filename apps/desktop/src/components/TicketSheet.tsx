import { useState } from "react";
import type { TicketInfo } from "../lib/ipc-types";
import { api, errorMessage, isMock } from "../lib/ipc";
import { fmtExpiry, shortId } from "../lib/format";
import { useStore } from "../lib/store";
import { Badge, Icon, Nest, Sheet } from "./ui";

export function TicketSheet({ info }: { info: TicketInfo }) {
  const close = useStore((s) => s.closeSheet);
  const setScreen = useStore((s) => s.setScreen);
  const toast = useStore((s) => s.toast);
  const refresh = useStore((s) => s.refreshTransfers);
  const dropHover = useStore((s) => s.dropHover);
  const [busy, setBusy] = useState(false);
  const exp = fmtExpiry(info.exp);

  async function run(fn: () => Promise<unknown>, okText: string) {
    setBusy(true);
    try {
      await fn();
      await refresh();
      close();
      setScreen("transfers");
      toast(okText);
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      setBusy(false);
    }
  }

  const doGet = () => run(() => api.start_get({ ticket: info.ticket }), "Đang nhận file…");
  const doPut = (paths: string[]) => run(() => api.start_put({ ticket: info.ticket, paths }), "Đang trả file…");
  async function pick() {
    const p = await api.pick_folder({ title: "Chọn folder để trả" });
    if (p) await doPut([p]);
  }

  if (info.admin) {
    return (
      <Sheet onClose={close} title="Link admin">
        <div className="flex items-center gap-2.5 mb-1"><span className="grid place-items-center w-9 h-9 rounded-full bg-accent/15 text-accent"><Icon name="shield" className="w-5 h-5" /></span><div className="text-lg font-semibold tracking-tight">Link admin</div></div>
        <p className="text-sm text-mute mb-3">
          Link này cho phép máy này xem folder và tạo link nhận/trả file trên máy tổng <span className="mono">{shortId(info.host, 8)}</span>. Chỉ dùng trên máy tin cậy.
        </p>
        <button
          className="btn btn-primary btn-lg w-full" disabled={busy || exp.expired}
          onClick={async () => {
            try { await api.set_admin_ticket({ ticket: info.ticket }); close(); setScreen("settings"); toast("Đã kết nối máy tổng từ xa"); }
            catch (e) { toast(errorMessage(e), "error"); }
          }}
        >Dùng làm máy tổng từ xa<Nest /></button>
        <button className="btn btn-ghost w-full mt-2" onClick={close}>Đóng</button>
      </Sheet>
    );
  }

  return (
    <Sheet onClose={close} title={info.job}>
      <div className="flex items-start justify-between gap-2 mb-1">
        <div className="min-w-0">
          <div className="text-xl font-semibold tracking-tight break-words">{info.job}</div>
          <div className="text-xs text-mute">Máy tổng <span className="mono">{shortId(info.host, 8)}</span></div>
        </div>
        <div className="flex gap-1 shrink-0">
          {info.get && <Badge tone="accent">Nhận</Badge>}
          {info.put && <Badge tone="ok">Trả</Badge>}
        </div>
      </div>
      <div className="text-xs mb-3"><span className={exp.expired ? "text-bad" : "text-mute"}>{exp.text}</span></div>

      {exp.expired && <div className="rounded-[12px] bg-bad/10 text-bad text-sm px-3 py-2 mb-3">Link đã hết hạn. Nhờ máy tổng tạo link mới.</div>}

      {info.get && (
        <button className="btn btn-primary btn-lg w-full" disabled={busy || exp.expired} onClick={doGet}>
          Nhận<Nest name="down" />
        </button>
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
          <span className="mx-auto mb-2 grid place-items-center w-10 h-10 rounded-full bg-ok/15 text-ok"><Icon name="up" className="w-5 h-5" /></span>
          <div className="text-base font-medium">Kéo folder vào để trả</div>
          <div className="text-xs text-mute mb-2">Thả folder hoặc file vào cửa sổ này</div>
          <button className="btn" disabled={busy} onClick={pick}><Icon name="folder" />Chọn folder…</button>
        </div>
      )}

      {!info.get && !info.put && <div className="text-sm text-mute">Link này không có quyền nhận hay trả.</div>}
      <button className="btn btn-ghost w-full mt-2" onClick={close}>Đóng</button>
    </Sheet>
  );
}
