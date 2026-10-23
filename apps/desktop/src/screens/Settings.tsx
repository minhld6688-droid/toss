import { useCallback, useEffect, useMemo, useState } from "react";
import type { HostConfig, HostStatus, TicketInfo } from "../lib/ipc-types";
import { api, errorMessage, isMock } from "../lib/ipc";
import { midTruncate, shortId } from "../lib/format";
import { useStore } from "../lib/store";
import { AnimatePresence, Icon, Nest, Row, Section, Toggle, Words } from "../components/ui";
import { LinkMaker, LinkBackend } from "../components/LinkMaker";
import { JobList } from "../components/JobList";
import { QrLink } from "../components/QrLink";
import { ConnectionSection } from "../components/Connection";
import { FirewallSheet } from "../components/Firewall";
import { RemoteUpdate, UpdateSection } from "../components/Update";

const ALIAS_RE = /^[A-Za-z0-9_-]{1,32}$/;
/** Host mode listens on this UDP port unless the user picks another (contracts/speed-v0.4.md §2). */
const DEFAULT_HOST_PORT = 41820;

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
function ShareSection() {
  const app = useStore((s) => s.app)!;
  const toast = useStore((s) => s.toast);
  const patch = useStore((s) => s.patchApp);
  const openShare = useStore((s) => s.openShare);
  const rev = useStore((s) => s.sharesRev);
  const refreshApp = useRefreshApp();
  const [shares, setShares] = useState<{ alias: string; path: string; job_id: string | null }[] | null>(null);
  const [confirm, setConfirm] = useState<string | null>(null);

  const guard = async (fn: () => Promise<void>) => { try { await fn(); } catch (e) { toast(errorMessage(e), "error"); } };
  const load = useCallback(async () => {
    try { setShares(await api.list_quick_shares()); } catch (e) { setShares([]); toast(errorMessage(e), "error"); }
  }, [toast]);
  useEffect(() => { void load(); }, [load, rev]);
  useEffect(() => {
    if (!confirm) return;
    const t = window.setTimeout(() => setConfirm(null), 3000);
    return () => window.clearTimeout(t);
  }, [confirm]);

  async function remove(alias: string) {
    if (confirm !== alias) { setConfirm(alias); return; }
    setConfirm(null);
    await guard(async () => { await api.remove_quick_share({ alias }); await load(); await refreshApp(); toast("Đã gỡ chia sẻ"); });
  }

  return (
    <>
      <Section title="Nhận & chia sẻ">
        <Row title="Tự động nhận khi bấm link" hint="Bấm link có quyền nhận là tải ngay">
          <Toggle on={app.auto_receive} label="Tự động nhận khi bấm link" onChange={(on) => guard(async () => { await api.set_auto_receive({ on }); patch({ auto_receive: on }); })} />
        </Row>
        <Row title="Menu chuột phải: Tạo link Toss" hint="Chuột phải vào folder để tạo link chia sẻ">
          <Toggle on={app.context_menu} label="Menu chuột phải: Tạo link Toss" onChange={(on) => guard(async () => { await api.set_context_menu({ on }); patch({ context_menu: on }); })} />
        </Row>
      </Section>

      <Section
        title="Folder đang chia sẻ"
        right={<button className="btn btn-ghost !h-7 !px-2.5 !text-xs -mt-1" onClick={() => guard(async () => {
          const p = await api.pick_folder({ title: "Chọn folder để chia sẻ" });
          if (p) openShare(p);
        })}><Icon name="plus" className="w-3.5 h-3.5" />Chia sẻ…</button>}
      >
        {shares === null ? (
          <div className="text-sm text-mute">Đang tải…</div>
        ) : shares.length === 0 ? (
          <div className="text-sm text-mute">Chưa chia sẻ folder nào. Chuột phải vào một folder và chọn “Tạo link Toss”, hoặc bấm “Chia sẻ…”.</div>
        ) : (
          <ul className="divide-y divide-line/10 -my-1">
            {shares.map((s) => (
              <li key={s.alias} className="flex items-center gap-2.5 py-2.5">
                <span className="grid place-items-center w-8 h-8 rounded-full shrink-0 bg-accent/15 text-accent"><Icon name="folder" className="w-4 h-4" weight="bold" /></span>
                <div className="min-w-0 flex-1">
                  <div className="mono text-xs text-ink selectable whitespace-nowrap overflow-hidden" title={s.path}>{midTruncate(s.path, 8, 18)}</div>
                  <div className="text-xs text-mute">{s.job_id ? <>Job <span className="mono selectable">{shortId(s.job_id, 8)}</span></> : "Chưa có link"}</div>
                </div>
                <button
                  className={`btn btn-ghost btn-danger shrink-0 !h-8 ${confirm === s.alias ? "!bg-bad/10" : ""}`}
                  onClick={() => void remove(s.alias)}
                  title="Thu hồi link và ngừng chia sẻ folder này"
                >{confirm === s.alias ? "Bấm lại để gỡ" : "Gỡ"}</button>
              </li>
            ))}
          </ul>
        )}
      </Section>
    </>
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
  // Port field keeps the raw text so typing "41820" is not clamped halfway; it commits when valid.
  const [portText, setPortText] = useState(app.host?.bind_port != null ? String(app.host.bind_port) : "");
  useEffect(() => setPortText(app.host?.bind_port != null ? String(app.host.bind_port) : ""), [app.host?.bind_port]);
  const portBad = portText !== "" && !(Number(portText) >= 1024 && Number(portText) <= 65535);
  const [fwAsk, setFwAsk] = useState(false);

  async function toggle(on: boolean) {
    try {
      if (on) {
        const cfg: HostConfig = { roots: [], return_root: `${app.dest_dir}${sep}Tra-ve`, max_parallel_jobs: 2, bind_port: DEFAULT_HOST_PORT };
        await api.set_host({ config: cfg });
      } else {
        await api.set_host({ config: null });
      }
      await refreshApp();
      // First time as a host: editors cannot reach us directly while the firewall blocks UDP.
      if (on) api.get_diagnostics().then((d) => { if (d.firewall === "missing") setFwAsk(true); }).catch(() => undefined);
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
    if (portBad) { toast("Cổng UDP phải từ 1024 đến 65535", "error"); return; }
    try { await api.set_host({ config: draft }); await refreshApp(); toast("Đã lưu cấu hình máy tổng"); } catch (e) { toast(errorMessage(e), "error"); }
  }

  return (
    <>
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

          <Row title="Cổng UDP" hint={`Mặc định ${DEFAULT_HOST_PORT}. Mở/forward cổng này trên router để editor ở xa nối trực tiếp`}>
            <input
              className={`input w-24 text-right mono ${portBad ? "border-bad" : ""}`}
              inputMode="numeric"
              placeholder={String(DEFAULT_HOST_PORT)}
              aria-label="Cổng UDP"
              value={portText}
              onChange={(e) => {
                const v = e.target.value.replace(/\D/g, "").slice(0, 5);
                setPortText(v);
                const n = Number(v);
                if (v === "") setDraft({ ...draft, bind_port: null });
                else if (n >= 1024 && n <= 65535) setDraft({ ...draft, bind_port: n });
              }}
            />
          </Row>

          <button className="btn btn-primary w-full" disabled={!dirty} onClick={save}>{dirty ? "Lưu cấu hình" : "Đã lưu"}</button>
        </div>
      )}
    </Section>
    <AnimatePresence>{fwAsk && <FirewallSheet key="fw" onClose={() => setFwAsk(false)} />}</AnimatePresence>
    </>
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

      {status && <RemoteUpdate status={status} onStatus={setStatus} />}

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
      <Words text="Cài đặt" accent={["đặt"]} className="text-[30px] mb-4" />
      <DeviceSection />
      <UpdateSection />
      <ConnectionSection />
      <ShareSection />
      <HostEditor />
      {app.host && <LocalHostTools />}
      <RemoteSection />
    </div>
  );
}
