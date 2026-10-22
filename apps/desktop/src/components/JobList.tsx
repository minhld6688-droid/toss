import { useCallback, useEffect, useState } from "react";
import type { JobRecord } from "../lib/ipc-types";
import { api, errorMessage } from "../lib/ipc";
import { fmtBytes, fmtDate, fmtExpiry } from "../lib/format";
import { useStore } from "../lib/store";
import { Badge, Icon } from "./ui";

export function JobList({ load, revoke, reloadKey }: { load: () => Promise<JobRecord[]>; revoke: (id: string) => Promise<void>; reloadKey?: number }) {
  const toast = useStore((s) => s.toast);
  const pageHost = useStore((s) => s.app?.page_host ?? "");
  const [jobs, setJobs] = useState<JobRecord[] | null>(null);
  const [confirm, setConfirm] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try { setJobs(await load()); } catch (e) { setJobs([]); toast(errorMessage(e), "error"); }
  }, [load, toast]);

  useEffect(() => { void reload(); }, [reload, reloadKey]);

  if (jobs === null) return <div className="text-sm text-mute">Đang tải…</div>;
  return (
    <div>
      <div className="flex justify-end mb-1">
        <button className="btn btn-ghost" onClick={() => void reload()}><Icon name="refresh" />Làm mới</button>
      </div>
      {jobs.length === 0 && <div className="text-sm text-mute">Chưa có job nào.</div>}
      <ul className="divide-y divide-line">
        {jobs.map((j) => {
          const exp = fmtExpiry(j.exp);
          const dead = j.revoked || exp.expired;
          return (
            <li key={j.id} className="py-2">
              <div className="flex items-center gap-2">
                <div className="min-w-0 flex-1">
                  <div className={`truncate text-base ${dead ? "text-mute line-through" : ""}`}>{j.job}</div>
                  <div className="text-xs text-mute truncate">{j.root}/{j.path}</div>
                </div>
                <div className="flex gap-1 shrink-0">
                  {j.get && <Badge tone="accent">Nhận</Badge>}
                  {j.put && <Badge tone="ok">Trả</Badge>}
                </div>
              </div>
              <div className="flex items-center justify-between mt-1 gap-2">
                <div className="text-xs min-w-0 truncate">
                  {j.revoked ? <span className="text-bad">Đã thu hồi</span> : <span className={exp.expired ? "text-bad" : "text-mute"}>{exp.text}</span>}
                  {j.last && (
                    <span className="text-mute"> · {j.last.device} {j.last.op === "get" ? "nhận" : "trả"} {fmtBytes(j.last.bytes)} {fmtDate(j.last.at)}{j.last.ok ? "" : " (lỗi)"}</span>
                  )}
                </div>
                <div className="flex gap-1 shrink-0">
                  {j.ticket && !dead && (
                    <button className="btn btn-ghost" title="Sao chép link" onClick={() => void api.copy_text({ text: `https://${pageHost}/#${j.ticket}` }).then(() => toast("Đã chép link"))}>
                      <Icon name="copy" />
                    </button>
                  )}
                  {!j.revoked && (
                    <button
                      className={`btn ${confirm === j.id ? "btn-primary" : "btn-danger"}`}
                      onClick={async () => {
                        if (confirm !== j.id) { setConfirm(j.id); setTimeout(() => setConfirm((c) => (c === j.id ? null : c)), 3000); return; }
                        try { await revoke(j.id); setConfirm(null); await reload(); toast("Đã thu hồi link"); } catch (e) { toast(errorMessage(e), "error"); }
                      }}
                    >{confirm === j.id ? "Chắc chắn?" : "Thu hồi"}</button>
                  )}
                </div>
              </div>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
