import { useState } from "react";
import { api, errorMessage } from "../lib/ipc";
import { useStore } from "../lib/store";
import { Icon } from "./ui";

export function Wizard() {
  const app = useStore((s) => s.app)!;
  const setApp = useStore((s) => s.setApp);
  const toast = useStore((s) => s.toast);
  const [step, setStep] = useState(0);
  const [name, setName] = useState(app.device_name);
  const [dest, setDest] = useState(app.dest_dir);
  const [busy, setBusy] = useState(false);

  async function finish() {
    setBusy(true);
    try { setApp(await api.finish_setup({ device_name: name.trim() || app.device_name, dest_dir: dest })); }
    catch (e) { toast(errorMessage(e), "error"); setBusy(false); }
  }

  return (
    <div className="fixed inset-0 z-40 bg-bg flex flex-col p-6">
      <div className="flex gap-1.5 mb-8">
        {[0, 1, 2].map((i) => <div key={i} className={`h-1 flex-1 rounded-full ${i <= step ? "bg-accent" : "bg-line"}`} />)}
      </div>

      <div className="flex-1 flex flex-col">
        {step === 0 && (
          <>
            <div className="text-xs text-mute mb-1">Bước 1/3</div>
            <h1 className="text-xl font-semibold mb-1">Tên máy của bạn</h1>
            <p className="text-sm text-mute mb-4">Tên này hiện ở phía máy tổng khi bạn nhận hoặc trả file.</p>
            <input className="input h-10 text-base" autoFocus value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === "Enter" && name.trim() && setStep(1)} />
          </>
        )}
        {step === 1 && (
          <>
            <div className="text-xs text-mute mb-1">Bước 2/3</div>
            <h1 className="text-xl font-semibold mb-1">Thư mục nhận file</h1>
            <p className="text-sm text-mute mb-4">File nhận về sẽ nằm trong thư mục này, mỗi job một folder riêng.</p>
            <div className="card p-3 flex items-center gap-2">
              <Icon name="folder" className="w-5 h-5 text-accent shrink-0" />
              <div className="font-mono text-xs break-all flex-1 selectable">{dest}</div>
              <button className="btn" onClick={async () => { try { const p = await api.pick_folder({ title: "Chọn thư mục nhận file" }); if (p) setDest(p); } catch (e) { toast(errorMessage(e), "error"); } }}>Đổi…</button>
            </div>
          </>
        )}
        {step === 2 && (
          <>
            <div className="text-xs text-mute mb-1">Bước 3/3</div>
            <div className="w-12 h-12 rounded-full bg-ok/15 text-ok grid place-items-center mb-3"><Icon name="check" className="w-6 h-6" /></div>
            <h1 className="text-xl font-semibold mb-1">Xong rồi</h1>
            <p className="text-sm text-mute">Dán link từ máy tổng vào Trang chính để nhận file. Muốn dùng máy này làm máy tổng, bật “Chế độ máy tổng” trong Cài đặt.</p>
            <div className="card p-3 mt-4 text-sm space-y-1">
              <div><span className="text-mute">Tên máy: </span>{name}</div>
              <div className="truncate"><span className="text-mute">Nhận về: </span><span className="font-mono text-xs">{dest}</span></div>
            </div>
          </>
        )}
      </div>

      <div className="flex gap-2">
        {step > 0 && <button className="btn btn-lg" onClick={() => setStep(step - 1)}>Quay lại</button>}
        {step < 2
          ? <button className="btn btn-primary btn-lg flex-1" disabled={step === 0 && !name.trim()} onClick={() => setStep(step + 1)}>Tiếp tục</button>
          : <button className="btn btn-primary btn-lg flex-1" disabled={busy} onClick={finish}>Bắt đầu dùng Toss</button>}
      </div>
    </div>
  );
}
