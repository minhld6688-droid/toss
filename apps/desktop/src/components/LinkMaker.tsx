import { useEffect, useState } from "react";
import type { DirEntry, TicketInfo, TicketSpec } from "../lib/ipc-types";
import { errorMessage } from "../lib/ipc";
import { useStore } from "../lib/store";
import { QrLink } from "./QrLink";
import { Icon, Nest } from "./ui";

export interface LinkBackend {
  roots: string[];
  listDir(root: string, path: string): Promise<DirEntry[]>;
  create(spec: TicketSpec): Promise<TicketInfo>;
}

const DAYS = [
  { v: 1, t: "1 ngày" }, { v: 3, t: "3 ngày" }, { v: 7, t: "7 ngày" }, { v: 14, t: "14 ngày" },
  { v: 30, t: "30 ngày" }, { v: 90, t: "90 ngày" }, { v: 0, t: "Không hết hạn" },
];

function lastSeg(p: string) { return p.split("/").filter(Boolean).pop() ?? ""; }

export function LinkMaker({ backend, onCreated }: { backend: LinkBackend; onCreated?: () => void }) {
  const toast = useStore((s) => s.toast);
  const [job, setJob] = useState("");
  const [jobTouched, setJobTouched] = useState(false);
  const [root, setRoot] = useState(backend.roots[0] ?? "");
  const [path, setPath] = useState("");
  const [ret, setRet] = useState("");
  const [get, setGet] = useState(true);
  const [put, setPut] = useState(false);
  const [days, setDays] = useState(7);
  const [browse, setBrowse] = useState(false);
  const [entries, setEntries] = useState<DirEntry[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<TicketInfo | null>(null);

  useEffect(() => {
    if (!backend.roots.includes(root)) setRoot(backend.roots[0] ?? "");
  }, [backend.roots, root]);

  useEffect(() => {
    if (!browse || !root) return;
    let alive = true;
    setEntries(null);
    backend.listDir(root, path).then((e) => alive && setEntries(e)).catch((e) => { if (alive) { setEntries([]); toast(errorMessage(e), "error"); } });
    return () => { alive = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [browse, root, path]);

  function choosePath(p: string) {
    setPath(p);
    if (!jobTouched) setJob(lastSeg(p) || root);
  }

  async function create() {
    if (!job.trim()) { toast("Nhập tên job", "error"); return; }
    if (!get && !put) { toast("Chọn Nhận và/hoặc Trả", "error"); return; }
    setBusy(true);
    try {
      const info = await backend.create({ job: job.trim(), root, path, ret: ret.trim(), get, put, days });
      setResult(info);
      onCreated?.();
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      setBusy(false);
    }
  }

  if (backend.roots.length === 0) {
    return <div className="text-sm text-mute">Chưa có thư mục gốc nào. Thêm ít nhất một thư mục gốc trong cấu hình máy tổng.</div>;
  }

  const crumbs = path.split("/").filter(Boolean);
  const dirs = (entries ?? []).filter((e) => e.is_dir).sort((a, b) => a.name.localeCompare(b.name, "vi"));
  const fileCount = (entries ?? []).filter((e) => !e.is_dir).length;

  return (
    <div className="space-y-3">
      <div>
        <label className="label">Tên job</label>
        <input className="input" value={job} placeholder="VD: 12A3 - RAW" onChange={(e) => { setJob(e.target.value); setJobTouched(true); }} />
      </div>

      <div className="grid grid-cols-[110px_1fr] gap-2">
        <div>
          <label className="label">Thư mục gốc</label>
          <select className="input" value={root} onChange={(e) => { setRoot(e.target.value); setPath(""); }}>
            {backend.roots.map((r) => <option key={r} value={r}>{r}</option>)}
          </select>
        </div>
        <div>
          <label className="label">Đường dẫn</label>
          <div className="flex gap-2">
            <input className="input mono" value={path} placeholder="(gốc)" onChange={(e) => setPath(e.target.value.replace(/\\/g, "/"))} />
            <button className="btn shrink-0" onClick={() => setBrowse((b) => !b)}><Icon name="folder" />Duyệt</button>
          </div>
        </div>
      </div>

      {browse && (
        <div className="rounded-[14px] hair bg-line/[0.04] overflow-hidden">
          <div className="flex flex-wrap items-center gap-1 px-2.5 py-2 border-b border-line/10 text-xs">
            <button className="text-accent hover:underline" onClick={() => choosePath("")}>{root}</button>
            {crumbs.map((c, i) => (
              <span key={i} className="flex items-center gap-1">
                <span className="text-mute">/</span>
                <button className="text-accent hover:underline" onClick={() => choosePath(crumbs.slice(0, i + 1).join("/"))}>{c}</button>
              </span>
            ))}
          </div>
          <div className="max-h-40 overflow-y-auto">
            {entries === null && <div className="p-2 text-xs text-mute">Đang tải…</div>}
            {entries && dirs.length === 0 && <div className="p-2 text-xs text-mute">Không có thư mục con{fileCount ? ` · ${fileCount} file` : ""}</div>}
            {dirs.map((d) => (
              <button key={d.name} className="w-full text-left px-2.5 py-2 text-sm flex items-center gap-2 hover:bg-line/[0.07] transition-colors duration-300"
                onClick={() => choosePath(crumbs.concat(d.name).join("/"))}>
                <Icon name="folder" className="w-4 h-4 text-mute" />{d.name}
              </button>
            ))}
            {entries && dirs.length > 0 && fileCount > 0 && <div className="px-2 py-1 text-xs text-mute">+ {fileCount} file</div>}
          </div>
          <div className="px-2.5 py-2 border-t border-line/10 flex justify-end">
            <button className="btn btn-primary" onClick={() => setBrowse(false)}>Chọn thư mục này</button>
          </div>
        </div>
      )}

      <div className="flex items-center gap-4">
        <label className="flex items-center gap-2 text-sm cursor-pointer"><input type="checkbox" checked={get} onChange={(e) => setGet(e.target.checked)} />Nhận (máy tổng → máy khách)</label>
      </div>
      <div className="flex items-center gap-4">
        <label className="flex items-center gap-2 text-sm cursor-pointer"><input type="checkbox" checked={put} onChange={(e) => setPut(e.target.checked)} />Trả (máy khách → máy tổng)</label>
      </div>
      {put && (
        <div>
          <label className="label">Thư mục trả về (trong thư mục trả, để trống = tên job)</label>
          <input className="input mono" value={ret} placeholder={job || "tên job"} onChange={(e) => setRet(e.target.value.replace(/\\/g, "/"))} />
        </div>
      )}

      <div>
        <label className="label">Hết hạn sau</label>
        <select className="input" value={days} onChange={(e) => setDays(Number(e.target.value))}>
          {DAYS.map((d) => <option key={d.v} value={d.v}>{d.t}</option>)}
        </select>
      </div>

      <button className="btn btn-primary btn-lg w-full" disabled={busy} onClick={create}>
        <Icon name="link" weight="bold" />{busy ? "Đang tạo…" : "Tạo link"}<Nest />
      </button>

      {result && <QrLink info={result} caption={`Link cho “${result.job}” — gửi cho editor hoặc quét QR`} />}
    </div>
  );
}
