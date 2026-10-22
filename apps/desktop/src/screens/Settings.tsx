import { useCallback, useEffect, useMemo, useState } from "react";
import type { HostConfig, HostStatus, TicketInfo } from "../lib/ipc-types";
import { api, errorMessage, isMock } from "../lib/ipc";
import { shortId } from "../lib/format";
import { useStore } from "../lib/store";
import { Icon, Nest, Row, Section, Toggle } from "../components/ui";
import { LinkMaker, LinkBackend } from "../components/LinkMaker";
import { JobList } from "../components/JobList";
import { QrLink } from "../components/QrLink";

const ALIAS_RE = /^[A-Za-z0-9_-]{1,32}$/;

function useRefreshApp() {
  const setApp = useStore((s) => s.setApp);
  return useCallback(async () => setApp(await api.get_state()), [setApp]);
}

// ---------------------------------------------------------------------------
function DeviceSection() {
  const app = useStore((s) => s.app)!;
  const toast = useStore((s) => s.toast);
  const patch = useStore((s) => s.patchApp);
  const [name, setName] = useState(app.device_name);
  useEffect(() => setName(app.device_name), [app.device_name]);

  const guard = async (fn: () => Promise<void>) => { try { await fn(); } catch (e) { toast(errorMessage(e), "error"); } };

  return (
    <Section title="Thiết bị">
      <div className="space-y-3">
        <div>
          <label className="label">Tên máy</label>
          <input
            className="input" value={name} onChange={(e) => setName(e.target.value)}
            onBlur={() => name.trim() && name !== app.device_name && guard(async () => { await api.set_device_name({ name: name.trim() }); patch({ device_name: name.trim() }); })}
            onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
          />
        </div>
        <div>
          <label className="label">Thư mục nhận file</label>
          <div className="flex gap-2">
            <div className="input flex items-center mono text-xs truncate selectable" title={app.dest_dir}>{app.dest_dir}</div>
            <button className="btn shrink-0" onClick={() => guard(async () => {
              const p = await api.pick_folder({ title: "Chọn thư mục nhận file" });
              if (p) { await api.set_dest_dir({ path: p }); patch({ dest_dir: p }); }
            })}><Icon name="folder" />Đổi…</button>
          </div>
        </div>
        <Row title="Mở cùng máy" hint="Tự chạy ẩn dưới khay khi đăng nhập">
          <Toggle on={app.autostart} label="Tự khởi động" onChange={(on) => guard(async () => { await api.set_autostart({ on }); patch({ autostart: on }); })} />
        </Row>
        <div className="text-xs text-mute flex items-center justify-between">
          <span>ID máy: <span className="mono selectable">{shortId(app.endpoint_id, 12)}</span></span>
          <span>Toss {app.version}{isMock ? " · MOCK" : ""}</span>
        </div>
      </div>
    </Section>
  );
}

// ---------------------------------------------------------------------------
function HostEditor() {
  const app = useStore((s) => s.app)!;
  const toast = useStore((s) => s.toast);
  const refreshApp = useRefreshApp();
  const [draft, setDraft] = useState<HostConfig | null>(app.host);
  const [saved, setSaved] = useState(JSON.stringify(app.host));
  useEffect(() => { setDraft(app.host); setSaved(JSON.stringify(app.host)); }, [app.host]);

  const dirty = JSON.stringify(draft) !== saved;
  const sep = app.dest_dir.includes("\\") ? "\\" : "/";

  async function toggle(on: boolean) {
    try {
      if (on) {
        const cfg: HostConfig = { roots: [], return_root: `${app.dest_dir}${sep}Tra-ve`, max_parallel_jobs: 2 };
        await api.set_host({ config: cfg });
      } else {
        await api.set_host({ config: null });
      }
      await refreshApp();
    } catch (e) { toast(errorMessage(e), "error"); }
  }

  async function pick(title: string): Promise<string | null> {
    try { return await api.pick_folder({ title }); } catch (e) { toast(errorMessage(e), "error"); return null; }
  }

  async function save() {
    if (!draft) return;
    const aliases = draft.roots.map((r) => r.alias);
    if (draft.roots.some((r) => !ALIAS_RE.test(r.alias))) { toast("Tên gốc chỉ gồm chữ, số, - và _ (tối đa 32 ký tự)", "error"); return; }
    if (new Set(aliases).size !== aliases.length) { toast("Tên gốc bị trùng", "error"); return; }
    if (draft.roots.some((r) => !r.path)) { toast("Chọn thư mục cho mỗi gốc", "error"); return; }
    if (!draft.return_root) { toast("Chọn thư mục trả về", "error"); return; }
    try { await api.set_host({ config: draft }); await refreshApp(); toast("Đã lưu cấu hình máy tổng"); } catch (e) { toast(errorMessage(e), "error"); }
  }

  return (
    <Section title="Chế độ máy tổng">
      <Row title="Chế độ máy tổng" hint="Máy này giữ file và cho phép máy khác nhận/trả">
        <Toggle on={!!app.host} label="Chế độ máy tổng" onChange={toggle} />
      </Row>

      {draft && (
        <div className="mt-3 space-y-3">
          <div>
            <div className="label">Thư mục gốc (alias + thư mục)</div>
            <div className="space-y-2">
              {draft.roots.map((r, i) => (
                <div key={i} className="grid grid-cols-[96px_1fr_auto_auto] gap-2 items-center">
                  <input
                    className={`input ${r.alias && !ALIAS_RE.test(r.alias) ? "border-bad" : ""}`} placeholder="Alias" value={r.alias}
                    onChange={(e) => setDraft({ ...draft, roots: draft.roots.map((x, j) => (j === i ? { ...x, alias: e.target.value } : x)) })}
                  />
                  <div className="input flex items-center mono text-xs truncate" title={r.path}>{r.path || <span className="text-mute">Chưa chọn</span>}</div>
                  <button className="btn btn-icon" title="Chọn thư mục" onClick={async () => {
                    const p = await pick(`Thư mục cho “${r.alias || "gốc"}”`);
                    if (p) setDraft({ ...draft, roots: draft.roots.map((x, j) => (j === i ? { ...x, path: p, alias: x.alias || suggestAlias(p) } : x)) });
                  }}><Icon name="folder" /></button>
                  <button className="btn btn-icon btn-ghost btn-danger" title="Xoá" onClick={() => setDraft({ ...draft, roots: draft.roots.filter((_, j) => j !== i) })}><Icon name="x" /></button>
                </div>
              ))}
              <button className="btn" onClick={async () => {
                const p = await pick("Thêm thư mục gốc");
                if (p) setDraft({ ...draft, roots: [...draft.roots, { alias: suggestAlias(p), path: p }] });
              }}><Icon name="plus" />Thêm thư mục gốc</button>
            </div>
          </div>

          <div>
            <label className="label">Thư mục trả về (file editor gửi lên)</label>
            <div className="flex gap-2">
              <div className="input flex items-center mono text-xs truncate" title={draft.return_root}>{draft.return_root || "Chưa chọn"}</div>
              <button className="btn shrink-0" onClick={async () => { const p = await pick("Thư mục trả về"); if (p) setDraft({ ...draft, return_root: p }); }}><Icon name="folder" />Đổi…</button>
            </div>
          </div>

          <Row title="Số job chạy song song" hint="Quá số này, editor sẽ phải chờ">
            <div className="flex items-center gap-2">
              <button className="btn btn-icon" onClick={() => setDraft({ ...draft, max_parallel_jobs: Math.max(1, draft.max_parallel_jobs - 1) })}>−</button>
              <span className="w-6 text-center mono">{draft.max_parallel_jobs}</span>
              <button className="btn btn-icon" onClick={() => setDraft({ ...draft, max_parallel_jobs: Math.min(16, draft.max_parallel_jobs + 1) })}>+</button>
            </div>
          </Row>

          <button className="btn btn-primary w-full" disabled={!dirty} onClick={save}>{dirty ? "Lưu cấu hình" : "Đã lưu"}</button>
        </div>
      )}
    </Section>
  );
}

function suggestAlias(p: string): string {
  const seg = p.split(/[\\/]+/).filter(Boolean).pop() ?? "Root";
  const a = seg.normalize("NFD").replace(/[̀-ͯ]/g, "").replace(/đ/g, "d").replace(/Đ/g, "D").replace(/[^A-Za-z0-9_-]+/g, "-").replace(/^-+|-+$/g, "");
  return (a || "Root").slice(0, 32);
}

// ---------------------------------------------------------------------------
function LocalHostTools() {
  const app = useStore((s) => s.app)!;
  const toast = useStore((s) => s.toast);
  const [adminDays, setAdminDays] = useState(30);
  const [admin, setAdmin] = useState<TicketInfo | null>(null);
  const [jobsKey, setJobsKey] = useState(0);

  const backend: LinkBackend = useMemo(() => ({
    roots: app.host?.roots.map((r) => r.alias) ?? [],
    listDir: (root, path) => api.list_dir({ root, path }),
    create: (spec) => api.create_ticket({ spec }),
  }), [app.host]);
  const load = useCallback(() => api.list_jobs(), []);
  const revoke = useCallback((id: string) => api.revoke({ id }), []);

  return (
    <>
      <Section title="Tạo link">
        <LinkMaker backend={backend} onCreated={() => setJobsKey((k) => k + 1)} />
      </Section>

      <Section title="Job đã tạo">
        <JobList load={load} revoke={revoke} reloadKey={jobsKey} />
      </Section>

      <Section title="Link admin">
        <p className="text-sm text-mute mb-3">
          Link admin cho phép một máy tin cậy (vd. máy của quản lý) tự xem folder và tạo link nhận/trả file trên máy tổng này mà không cần ngồi tại máy.
          Chỉ dán link này vào máy bạn tin tưởng — ai có link đều tạo được link tải file. Có thể đặt hạn để link tự vô hiệu.
        </p>
        <div className="flex items-center gap-2">
          <select className="input w-32" value={adminDays} onChange={(e) => setAdminDays(Number(e.target.value))}>
            <option value={7}>7 ngày</option><option value={30}>30 ngày</option><option value={90}>90 ngày</option><option value={365}>1 năm</option>
          </select>
          <button className="btn btn-primary" onClick={async () => {
            try { setAdmin(await api.create_admin_ticket({ days: adminDays })); } catch (e) { toast(errorMessage(e), "error"); }
          }}><Icon name="shield" />Tạo link admin<Nest /></button>
        </div>
        {admin && <div className="mt-3"><QrLink info={admin} caption="Dán link này vào mục “Máy tổng từ xa” trên máy kia" /></div>}
      </Section>
    </>
  );
}

// ---------------------------------------------------------------------------
function RemotePanel() {
  const [status, setStatus] = useState<HostStatus | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [jobsKey, setJobsKey] = useState(0);
  const [loading, setLoading] = useState(true);

  const loadStatus = useCallback(async () => {
    setLoading(true);
    try {
      const timeout = new Promise<never>((_, rej) => setTimeout(() => rej(new Error("Máy tổng không phản hồi (đang offline?)")), 25000));
      setStatus(await Promise.race([api.remote_status(), timeout]));
      setErr(null);
    } catch (e) { setStatus(null); setErr(errorMessage(e)); }
    setLoading(false);
  }, []);
  useEffect(() => { void loadStatus(); }, [loadStatus]);

  const backend: LinkBackend = useMemo(() => ({
    roots: status?.roots ?? [],
    listDir: (root, path) => api.remote_list_dir({ root, path }),
    create: (spec) => api.remote_create_ticket({ spec }),
  }), [status]);
  const load = useCallback(() => api.remote_list_jobs(), []);
  const revoke = useCallback((id: string) => api.remote_revoke({ id }), []);

  return (
    <div className="space-y-4">
      <div className="rounded-[14px] hair bg-line/[0.04] px-3 py-2.5 text-sm flex items-center justify-between gap-2">
        {loading ? <span className="text-mute">Đang kết nối máy tổng…</span> : status ? (
          <div className="min-w-0">
            <div className="flex items-center gap-2"><span className="inline-block w-2 h-2 rounded-full bg-ok shadow-[0_0_8px_rgb(var(--ok)/0.8)]" /><span className="font-medium truncate">{status.device}</span><span className="text-xs text-mute">v{status.version}</span></div>
            <div className="text-xs text-mute">{status.active} đang chạy · {status.queued} chờ · online {Math.floor(status.uptime_s / 3600)} giờ</div>
          </div>
        ) : (
          <div className="min-w-0"><div className="flex items-center gap-2"><span className="inline-block w-2 h-2 rounded-full bg-bad" /><span>Không kết nối được</span></div><div className="text-xs text-bad truncate">{err}</div></div>
        )}
        <button className="btn btn-icon shrink-0" onClick={() => void loadStatus()}><Icon name="refresh" /></button>
      </div>

      {status && (
        <>
          <div>
            <div className="h-section">Tạo link từ xa</div>
            <LinkMaker backend={backend} onCreated={() => setJobsKey((k) => k + 1)} />
          </div>
          <div>
            <div className="h-section">Job trên máy tổng</div>
            <JobList load={load} revoke={revoke} reloadKey={jobsKey} />
          </div>
        </>
      )}
    </div>
  );
}

function RemoteSection() {
  const toast = useStore((s) => s.toast);
  const [has, setHas] = useState<boolean | null>(null);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const app = useStore((s) => s.app)!;

  // The admin ticket lives in the shell settings (never dial the host just to find out).
  useEffect(() => {
    api.has_admin_ticket().then(setHas).catch(() => setHas(false));
  }, []);

  if (has === null) return <Section title="Máy tổng từ xa"><div className="text-sm text-mute">Đang kiểm tra…</div></Section>;

  async function connect() {
    setBusy(true);
    try {
      const info = await api.parse_ticket({ text });
      if (!info.admin) throw new Error("Đây không phải link admin");
      await api.set_admin_ticket({ ticket: info.ticket });
      setText(""); setHas(true); toast("Đã kết nối máy tổng từ xa");
    } catch (e) { toast(errorMessage(e), "error"); }
    setBusy(false);
  }

  return (
    <Section
      title="Máy tổng từ xa"
      right={has ? <button className="btn btn-ghost btn-danger" onClick={async () => { try { await api.set_admin_ticket({ ticket: null }); setHas(false); } catch (e) { toast(errorMessage(e), "error"); } }}>Gỡ</button> : undefined}
    >
      {has ? <RemotePanel /> : (
        <div className="space-y-2">
          <p className="text-sm text-mute">Dán link admin do máy tổng tạo để xem folder và tạo link nhận/trả file từ xa{app.host ? " (máy này đang là máy tổng — thường không cần)" : ""}.</p>
          <textarea className="input h-20 py-2 mono text-xs resize-none" placeholder="Dán link admin (toss1…)" value={text} onChange={(e) => setText(e.target.value)} spellCheck={false} />
          <button className="btn btn-primary w-full" disabled={busy || !text.trim()} onClick={connect}>Kết nối máy tổng<Nest /></button>
        </div>
      )}
    </Section>
  );
}

// ---------------------------------------------------------------------------
export function Settings() {
  const app = useStore((s) => s.app);
  if (!app) return null;
  return (
    <div className="p-4">
      <h1 className="text-2xl font-semibold tracking-tight mb-4">Cài đặt</h1>
      <DeviceSection />
      <HostEditor />
      {app.host && <LocalHostTools />}
      <RemoteSection />
    </div>
  );
}
