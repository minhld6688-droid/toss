import { useState } from "react";
import { api, errorMessage, isMock } from "../lib/ipc";
import { fmtExpiry, shortId } from "../lib/format";
import { useStore } from "../lib/store";
import { Badge, Icon, Sheet } from "./ui";

export function TicketSheet() {
  const info = useStore((s) => s.sheet);
  const close = useStore((s) => s.closeSheet);
  const setScreen = useStore((s) => s.setScreen);
  const toast = useStore((s) => s.toast);
  const refresh = useStore((s) => s.refreshTransfers);
  const dropHover = useStore((s) => s.dropHover);
  const [busy, setBusy] = useState(false);
  if (!info) return null;
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
        <div className="flex items-center gap-2 mb-1"><Icon name="shield" className="w-5 h-5 text-accent" /><div className="text-lg font-semibold">Link admin</div></div>
        <p className="text-sm text-mute mb-3">
          Link này cho phép máy này xem folder và tạo link nhận/trả file trên máy tổng <span className="font-mono">{shortId(info.host, 8)}</span>. Chỉ dùng trên máy tin cậy.
        </p>
        <button
          className="btn btn-primary btn-lg w-full" disabled={busy || exp.expired}
          onClick={async () => {
            try { await api.set_admin_ticket({ ticket: info.ticket }); close(); setScreen("settings"); toast("Đã kết nối máy tổng từ xa"); }
            catch (e) { toast(errorMessage(e), "error"); }
          }}
        >Dùng làm máy tổng từ xa</button>
        <button className="btn btn-ghost w-full mt-2" onClick={close}>Đóng</button>
      </Sheet>
    );
  }

  return (
    <Sheet onClose={close} title={info.job}>
      <div className="flex items-start justify-between gap-2 mb-1">
        <div className="min-w-0">
          <div className="text-lg font-semibold break-words">{info.job}</div>
          <div className="text-xs text-mute">Máy tổng <span className="font-mono">{shortId(info.host, 8)}</span></div>
        </div>
        <div className="flex gap-1 shrink-0">
          {info.get && <Badge tone="accent">Nhận</Badge>}
          {info.put && <Badge tone="ok">Trả</Badge>}
        </div>
      </div>
      <div className="text-xs mb-3"><span className={exp.expired ? "text-bad" : "text-mute"}>{exp.text}</span></div>

      {exp.expired && <div className="rounded bg-bad/10 text-bad text-sm p-2 mb-3">Link đã hết hạn. Nhờ máy tổng tạo link mới.</div>}

      {info.get && (
        <button className="btn btn-primary btn-lg w-full" disabled={busy || exp.expired} onClick={doGet}>
          <Icon name="down" />Nhận
        </button>
      )}

      {info.put && (
        <div
          className={`mt-3 rounded-lg border-2 border-dashed p-4 text-center transition-colors ${dropHover ? "border-accent bg-accent/10" : "border-line"} ${exp.expired ? "opacity-50 pointer-events-none" : ""}`}
          onDragOver={isMock ? (e) => e.preventDefault() : undefined}
          onDrop={isMock ? (e) => {
            e.preventDefault();
            const names = Array.from(e.dataTransfer.files).map((f) => f.name);
            void doPut(names.length ? names : ["Folder demo"]);
          } : undefined}
        >
          <Icon name="up" className="w-6 h-6 mx-auto text-mute mb-1" />
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
