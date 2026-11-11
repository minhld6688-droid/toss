// 0.9 Máy studio: link của từng folder thợ (tự tạo khi copy xong) và link backup của editor.
// Mục tiêu: người ngồi máy studio chỉ bấm "Chép cả job" rồi dán vào 2007ops.
import { useCallback, useEffect, useMemo, useState } from "react";
import type React from "react";
import type { StudioLink, StudioView } from "@contracts/ipc";
import { api, errorMessage, on } from "../lib/ipc";
import { fmtAgo, fmtBytes, fmtExpiry, fmtSpeed } from "../lib/format";
import { useStore } from "../lib/store";
import { Icon, Progress, Words } from "../components/ui";

/** One line for 2007ops: "Tuan Future (48.2 GB, 1912 file): https://…". */
const lineOf = (l: StudioLink) => `${l.shooter || l.job} (${fmtBytes(l.bytes)}, ${l.files} file): ${l.url}`;
/** The job name, then one line per shooter. */
export const jobText = (job: string, links: StudioLink[]) => [job, ...links.map(lineOf)].join("\n");
const singleText = (l: StudioLink) => (l.shooter ? `${l.job} - ${lineOf(l)}` : lineOf(l));

const dead = (l: StudioLink) => !!l.revoked || (l.exp ? fmtExpiry(l.exp).expired : false);

/** What is happening with a link right now, in one line. */
function activity(l: StudioLink): { text: string; frac: number | null; tone: "accent" | "ok" | "mute" | "bad" } {
  if (l.revoked) return { text: "Đã thu hồi", frac: null, tone: "mute" };
  if (l.exp && fmtExpiry(l.exp).expired) return { text: "Hết hạn", frac: null, tone: "mute" };
  const a = l.active?.find((x) => x.status === "running") ?? l.active?.[0];
  if (a) {
    const pct = a.bytes_total > 0 ? Math.round((a.bytes_done / a.bytes_total) * 100) : 0;
    const verb = l.kind === "backup" ? "đang gửi về" : "đang tải";
    const speed = a.status === "running" && a.speed_bps > 0 ? `, ${fmtSpeed(a.speed_bps)}` : "";
    const slow = a.path === "relay" ? " (đường vòng, chậm)" : "";
    return { text: `${a.peer} ${verb} ${pct}%${speed}${slow}`, frac: a.bytes_total > 0 ? a.bytes_done / a.bytes_total : 0, tone: "accent" };
  }
  if (l.last) {
    const what = l.last.op === "put" ? "gửi về" : "tải";
    const ok = l.last.ok ? `đã ${what} ${fmtBytes(l.last.bytes)}` : `${what} lỗi`;
    return { text: `${l.last.device} ${ok}, ${fmtAgo(l.last.at)}`, frac: null, tone: l.last.ok ? "ok" : "bad" };
  }
  return { text: l.kind === "backup" ? "Chưa gửi lần nào" : "Chưa ai tải", frac: null, tone: "mute" };
}

function useCopy() {
  const toast = useStore((s) => s.toast);
  return useCallback(async (text: string, what: string) => {
    try { await api.copy_text({ text }); toast(`Đã chép ${what}`); } catch (e) { toast(errorMessage(e), "error"); }
  }, [toast]);
}

/** Two taps to revoke, like the rest of the app. Quiet until armed: copying is the job here. */
function RevokeButton({ name, onConfirm }: { name: string; onConfirm: () => Promise<void> }) {
  const [armed, setArmed] = useState(false);
  useEffect(() => { if (!armed) return; const t = setTimeout(() => setArmed(false), 3000); return () => clearTimeout(t); }, [armed]);
  const click = async () => { if (!armed) { setArmed(true); return; } setArmed(false); await onConfirm(); };
  return armed ? (
    <button className="btn btn-danger shrink-0" onClick={() => void click()}>Thu hồi?</button>
  ) : (
    <button className="btn btn-icon btn-ghost shrink-0 text-mute" title="Thu hồi link" aria-label={`Thu hồi link ${name}`} onClick={() => void click()}>
      <Icon name="x" />
    </button>
  );
}

function LinkRow({ l, onRemove }: { l: StudioLink; onRemove: (id: string) => Promise<void> }) {
  const copy = useCopy();
  const act = activity(l);
  const off = dead(l);
  return (
    <li className="px-3.5 py-3">
      <div className="flex items-center gap-3">
        <div className="min-w-0 flex-1">
          <div className={`truncate text-base font-medium ${off ? "text-mute line-through" : ""}`} title={l.rel || l.job}>{l.shooter || l.job}</div>
          <div className="text-xs text-mute truncate mono">{fmtBytes(l.bytes)}, {l.files} file</div>
        </div>
        {!off && (
          <button className="btn shrink-0" onClick={() => void copy(singleText(l), `link ${l.shooter || l.job}`)} aria-label={`Chép link ${l.shooter || l.job}`}>
            <Icon name="copy" />Chép
          </button>
        )}
        {!l.revoked && <RevokeButton name={l.shooter || l.job} onConfirm={() => onRemove(l.id)} />}
      </div>
      {act.frac !== null && <div className="mt-2"><Progress value={act.frac} tone="accent" live /></div>}
      <div className={`mt-1.5 text-xs truncate ${act.tone === "bad" ? "text-bad" : act.tone === "ok" ? "text-ok" : "text-mute"}`}>{act.text}</div>
    </li>
  );
}

function JobGroup({ job, links, i, onRemove }: { job: string; links: StudioLink[]; i: number; onRemove: (id: string) => Promise<void> }) {
  const copy = useCopy();
  const live = links.filter((l) => !dead(l));
  const bytes = links.reduce((n, l) => n + l.bytes, 0);
  return (
    <section className="mb-4 rise" style={{ "--i": i } as React.CSSProperties}>
      <div className="shell"><div className="core">
        <div className="flex items-center gap-3 px-3.5 pt-3.5 pb-2.5 border-b border-line/10">
          <div className="min-w-0 flex-1">
            <div className="title text-[19px] leading-tight break-words">{job}</div>
            <div className="text-xs text-mute mt-0.5">{links.length} thợ, {fmtBytes(bytes)}</div>
          </div>
          {live.length > 0 && (
            <button className="btn btn-primary shrink-0" onClick={() => void copy(jobText(job, live), `cả job (${live.length} link)`)}>
              <Icon name="copy" weight="bold" />Chép cả job
            </button>
          )}
        </div>
        <ul className="divide-y divide-line/10">{links.map((l) => <LinkRow key={l.id} l={l} onRemove={onRemove} />)}</ul>
      </div></div>
    </section>
  );
}

function BackupSection({ view, onChange }: { view: StudioView; onChange: () => Promise<void> }) {
  const toast = useStore((s) => s.toast);
  const copy = useCopy();
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const list = view.links.filter((l) => l.kind === "backup");
  async function create() {
    setBusy(true);
    try {
      const l = await api.studio_backup_link({ editor: name });
      await api.copy_text({ text: `Backup về studio (${l.job}): ${l.url}` });
      toast(`Đã chép link backup của ${l.job}`);
      setName("");
      await onChange();
    } catch (e) { toast(errorMessage(e), "error"); } finally { setBusy(false); }
  }
  return (
    <section className="mb-4 rise" style={{ "--i": 3 } as React.CSSProperties}>
      <h2 className="h-section">Backup về studio</h2>
      <div className="shell"><div className="core">
        <div className="p-3.5 border-b border-line/10">
          <p className="text-xs text-mute mb-2.5">
            Mỗi editor một link cố định, không hết hạn. Kéo folder vào link là file về{" "}
            <span className="mono text-ink selectable">{view.return_root ? `${view.return_root}${view.return_root.includes("/") ? "/" : "\\"}<tên editor>` : "nơi nhận file gửi lại"}</span>.
          </p>
          <label className="label" htmlFor="backup-editor">Tên editor</label>
          <div className="flex gap-2">
            <input
              id="backup-editor" className="input" value={name} placeholder="VD: Ngọc Anh"
              onChange={(e) => setName(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter" && name.trim()) void create(); }}
            />
            <button className="btn btn-primary shrink-0" disabled={busy || !name.trim()} onClick={() => void create()}>
              <Icon name="link" weight="bold" />{busy ? "Đang tạo…" : "Tạo link"}
            </button>
          </div>
        </div>
        {list.length === 0 ? (
          <div className="px-3.5 py-3 text-sm text-mute">Chưa có link backup nào.</div>
        ) : (
          <ul className="divide-y divide-line/10">
            {list.map((l) => {
              const act = activity(l);
              return (
                <li key={l.id} className="px-3.5 py-3">
                  <div className="flex items-center gap-3">
                    <div className="min-w-0 flex-1">
                      <div className={`truncate text-base font-medium ${l.revoked ? "text-mute line-through" : ""}`}>{l.job}</div>
                      <div className={`text-xs ${act.tone === "bad" ? "text-bad" : act.tone === "ok" ? "text-ok" : "text-mute"}`}>{act.text}</div>
                    </div>
                    {!l.revoked && (
                      <button className="btn shrink-0" onClick={() => void copy(`Backup về studio (${l.job}): ${l.url}`, `link backup của ${l.job}`)}>
                        <Icon name="copy" />Chép
                      </button>
                    )}
                    {!l.revoked && <RevokeButton name={l.job} onConfirm={async () => { await api.studio_remove({ id: l.id }); await onChange(); }} />}
                  </div>
                  {act.frac !== null && <div className="mt-2"><Progress value={act.frac} tone="accent" live /></div>}
                </li>
              );
            })}
          </ul>
        )}
      </div></div>
    </section>
  );
}

export function Links() {
  const toast = useStore((s) => s.toast);
  const setScreen = useStore((s) => s.setScreen);
  const [view, setView] = useState<StudioView | null>(null);
  const [scanning, setScanning] = useState(false);

  const load = useCallback(async () => {
    try { setView(await api.studio_view()); } catch (e) { toast(errorMessage(e), "error"); }
  }, [toast]);

  useEffect(() => {
    void load();
    let un: (() => void) | null = null;
    let alive = true;
    void on("studio:links", () => void load()).then((u) => { if (alive) un = u; else u(); });
    // progress of downloads moves; the list is cheap to fetch
    const t = setInterval(() => void load(), 4000);
    return () => { alive = false; un?.(); clearInterval(t); };
  }, [load]);

  const groups = useMemo(() => {
    const map = new Map<string, StudioLink[]>();
    for (const l of view?.links ?? []) {
      if (l.kind === "backup") continue;
      const g = map.get(l.job) ?? [];
      g.push(l);
      map.set(l.job, g);
    }
    return [...map.entries()]
      .map(([job, links]) => ({ job, links: links.sort((a, b) => a.shooter.localeCompare(b.shooter, "vi")), at: Math.max(...links.map((l) => l.at)) }))
      .sort((a, b) => b.at - a.at);
  }, [view]);

  async function remove(id: string) {
    try { setView(await api.studio_remove({ id })); toast("Đã thu hồi link"); } catch (e) { toast(errorMessage(e), "error"); }
  }

  if (!view) return <div className="p-4"><Words text="Links" className="text-[30px] mb-4" /><div className="text-sm text-mute">Đang tải…</div></div>;

  return (
    <div className="p-4">
      <Words text="Links" className="text-[30px] mb-3" />

      {!view.on ? (
        <div className="shell rise"><div className="core p-4">
          <div className="text-base font-medium mb-1">Chế độ máy studio đang tắt</div>
          <p className="text-sm text-mute mb-3">Bật trong Cài đặt, mục Máy studio: chọn thư mục Jobs, link sẽ tự có cho từng folder thợ.</p>
          <button className="btn btn-primary" onClick={() => setScreen("settings")}><Icon name="settings" />Mở Cài đặt</button>
        </div></div>
      ) : (
        <div className="shell rise mb-4"><div className="core p-3.5 flex items-center gap-3">
          <span className={`grid place-items-center w-9 h-9 rounded-full shrink-0 ${view.error ? "bg-bad/15 text-bad" : "bg-ok/15 text-ok"}`}><Icon name="folder" className="w-[18px] h-[18px]" /></span>
          <div className="min-w-0 flex-1">
            <div className="text-base font-medium truncate mono selectable" title={view.root_path ?? ""}>{view.root_path ?? "Chưa chọn thư mục Jobs"}</div>
            <div className={`text-xs ${view.error ? "text-bad" : "text-mute"}`}>
              {view.error ?? `Folder trong 1_RAW copy xong 2 phút là có link. Quét ${view.last_scan ? fmtAgo(view.last_scan) : "sắp tới"}.`}
            </div>
          </div>
          <button
            className="btn shrink-0" disabled={scanning}
            onClick={async () => { setScanning(true); try { setView(await api.studio_scan()); } catch (e) { toast(errorMessage(e), "error"); } finally { setScanning(false); } }}
          ><Icon name="refresh" />{scanning ? "Đang quét…" : "Quét ngay"}</button>
        </div></div>
      )}

      {view.on && groups.length === 0 && (
        <div className="text-center py-10 rise" style={{ "--i": 1 } as React.CSSProperties}>
          <div className="title text-xl">Chưa có link nào</div>
          <p className="text-sm text-mute mt-1 max-w-[42ch] mx-auto">
            Copy thẻ của thợ vào <span className="mono">…\1_RAW\&lt;tên thợ&gt;</span> trong thư mục Jobs. Copy xong 2 phút, link tự hiện ở đây.
            Folder nằm chỗ khác thì chuột phải, chọn <span className="text-ink">Tạo link Toss</span>.
          </p>
        </div>
      )}

      {groups.map((g, i) => <JobGroup key={g.job} job={g.job} links={g.links} i={i + 1} onRemove={remove} />)}

      {(view.on || view.links.some((l) => l.kind === "backup")) && <BackupSection view={view} onChange={load} />}

      {view.on && (
        <p className="text-xs text-mute text-center mt-2">
          {view.awake ? "Máy đang được giữ thức để editor tải lúc nào cũng được." : "Máy có thể ngủ: editor sẽ không tải được khi máy ngủ."}
          {view.parallel ? ` Tối đa ${view.parallel} lượt tải cùng lúc, người sau tự xếp hàng.` : ""}
        </p>
      )}
    </div>
  );
}
