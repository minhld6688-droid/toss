import { useCallback, useEffect, useMemo, useState } from "react";
import type { HostConfig, HostStatus, ReceivePolicy, TicketInfo } from "@contracts/ipc";
import { api, errorMessage, isMock } from "../lib/ipc";
import { midTruncate, shortId } from "../lib/format";
import { useStore } from "../lib/store";
import { AnimatePresence, EASE, Icon, Nest, Row, Section, Toggle, Words, motion } from "../components/ui";
import { LinkMaker, LinkBackend } from "../components/LinkMaker";
import { JobList } from "../components/JobList";
import { QrLink } from "../components/QrLink";
import { ConnectionSection } from "../components/Connection";
import { FirewallSheet } from "../components/Firewall";
import { RemoteUpdate, UpdateSection } from "../components/Update";
import { Hint } from "../components/Hint";
import { useReducedMotion } from "motion/react";

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
    <Section title="Máy này">
      <div className="space-y-3">
        <div>
          <label className="label">Tên máy (người khác thấy tên này)</label>
          <input
            className="input" value={name} onChange={(e) => setName(e.target.value)}
            onBlur={() => name.trim() && name !== app.device_name && guard(async () => { await api.set_device_name({ name: name.trim() }); patch({ device_name: name.trim() }); })}
            onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
          />
        </div>
        <div>
          <label className="label">Thư mục nhận file (mỗi lần nhận là một folder con)</label>
          <div className="flex gap-2">
            <div className="input flex items-center mono text-xs truncate selectable" title={app.dest_dir}>{app.dest_dir}</div>
            <button className="btn shrink-0" onClick={() => guard(async () => {
              const p = await api.pick_folder({ title: "Chọn thư mục nhận file" });
              if (p) { await api.set_dest_dir({ path: p }); patch({ dest_dir: p }); }
            })}><Icon name="folder" />Đổi…</button>
          </div>
        </div>
        <div className="divide-y divide-line/10">
          <Row title="Mở cùng máy" hint="Tự chạy ẩn khi bật máy — để luôn nhận được file">
            <Toggle on={app.autostart} label="Mở cùng máy" onChange={(on) => guard(async () => { await api.set_autostart({ on }); patch({ autostart: on }); })} />
          </Row>
          <Row title="Chuột phải: “Gửi bằng Toss”" hint="Chuột phải vào file hoặc folder bất kỳ để gửi">
            <Toggle on={app.context_menu} label="Chuột phải: Gửi bằng Toss" onChange={(on) => guard(async () => { await api.set_context_menu({ on }); patch({ context_menu: on }); })} />
          </Row>
        </div>
        <div className="text-xs text-mute">ID máy: <span className="mono selectable">{shortId(app.endpoint_id, 12)}</span>{isMock ? " · MOCK" : ""}</div>
      </div>
    </Section>
  );
}

// ---------------------------------------------------------------------------
const POLICIES: { value: ReceivePolicy; title: string; hint: string }[] = [
  { value: "off", title: "Tắt", hint: "Ai gửi cũng hỏi bạn trước" },
  { value: "contacts", title: "Chỉ danh bạ", hint: "Danh bạ nhận luôn, máy lạ thì hỏi" },
  { value: "everyone", title: "Mọi người cùng mạng", hint: "Máy nào cùng wifi cũng nhận luôn" },
];

/** Who may send straight in; LocalSend visibility and the LocalSend devices that are always accepted. */
function ReceiveSection() {
  const app = useStore((s) => s.app)!;
  const toast = useStore((s) => s.toast);
  const patch = useStore((s) => s.patchApp);
  const refreshApp = useRefreshApp();
  const guard = async (fn: () => Promise<void>) => { try { await fn(); } catch (e) { toast(errorMessage(e), "error"); } };
  const fallback = app.ls_enabled && app.ls_port != null && app.ls_port !== 53317;

  return (
    <Section title="Nhận">
      <div className="text-sm text-mute mb-2.5">Ai được gửi thẳng vào máy này mà không cần bạn bấm Nhận?</div>
      <div role="radiogroup" aria-label="Ai được gửi thẳng" className="space-y-1.5">
        {POLICIES.map((o) => {
          const on = app.receive_policy === o.value;
          return (
            <button
              key={o.value} role="radio" aria-checked={on}
              onClick={() => guard(async () => { await api.set_receive_policy({ policy: o.value }); patch({ receive_policy: o.value }); })}
              className={`w-full text-left flex items-center gap-3 px-3 py-2.5 rounded-[14px] transition-[background-color,box-shadow] duration-500 ease-aura active:scale-[0.99] ${on ? "bg-accent/10 shadow-[inset_0_0_0_1.5px_rgb(var(--accent)/0.7)]" : "hair bg-line/[0.03] hover:bg-line/[0.06]"}`}
            >
              <span className={`grid place-items-center w-[18px] h-[18px] rounded-full shrink-0 border-[1.5px] transition-colors duration-500 ${on ? "border-accent" : "border-line/30"}`}>
                <span className={`w-2 h-2 rounded-full bg-accent transition-transform duration-500 ${on ? "scale-100" : "scale-0"}`} style={{ transitionTimingFunction: "var(--spring)" }} />
              </span>
              <span className="min-w-0">
                <span className="block text-base font-medium">{o.title}</span>
                <span className="block text-xs text-mute">{o.hint}</span>
              </span>
            </button>
          );
        })}
      </div>
      {app.receive_policy === "everyone" && <Hint className="mt-2 !text-warn">Dùng wifi công cộng thì nên chọn “Chỉ danh bạ”: ai cùng mạng cũng gửi file vào máy bạn được.</Hint>}

      <div className="divide-y divide-line/10 mt-2">
        <Row title="Bấm link là nhận luôn" hint="Tắt nếu muốn xem trước rồi mới bấm Nhận">
          <Toggle on={app.auto_receive} label="Tự động nhận khi bấm link" onChange={(on) => guard(async () => { await api.set_auto_receive({ on }); patch({ auto_receive: on }); })} />
        </Row>
        <Row title="Hiện với máy LocalSend" hint="Điện thoại, máy tính dùng LocalSend thấy và gửi được cho bạn">
          <Toggle on={app.ls_enabled} label="Hiện với máy LocalSend" onChange={(on) => guard(async () => { await api.set_ls_enabled({ on }); patch({ ls_enabled: on, ls_port: on ? app.ls_port : null }); })} />
        </Row>
      </div>
      {fallback && <Hint className="mt-1.5">Cổng 53317 đang bận nên Toss dùng cổng {app.ls_port}. LocalSend vẫn tìm thấy bạn.</Hint>}

      <div className="mt-3">
        <div className="label !mb-1.5">Máy LocalSend tin cậy</div>
        {app.ls_trusted.length === 0 ? (
          <div className="text-xs text-mute">Chưa có máy nào. Khi có máy gửi đến, bấm “Luôn nhận từ máy này” để lần sau nhận luôn.</div>
        ) : (
          <ul className="divide-y divide-line/10 -my-1">
            {app.ls_trusted.map((d) => (
              <li key={d.fingerprint} className="flex items-center gap-2.5 py-2">
                <span className="grid place-items-center w-8 h-8 rounded-full shrink-0 bg-ok/15 text-ok"><Icon name="shield" className="w-4 h-4" weight="bold" /></span>
                <div className="min-w-0 flex-1">
                  <div className="truncate text-base">{d.alias}</div>
                  <div className="mono text-[11px] text-mute truncate selectable" title={d.fingerprint}>{shortId(d.fingerprint, 14)}</div>
                </div>
                <button className="btn btn-ghost btn-danger !h-8 shrink-0" onClick={() => guard(async () => { await api.forget_trusted({ fingerprint: d.fingerprint }); await refreshApp(); toast(`Đã quên ${d.alias}`); })}>Quên</button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </Section>
  );
}

// ---------------------------------------------------------------------------
/** Quick-share links that are still open (advanced: most people never need to revoke one by hand). */
function OpenLinks() {
  const toast = useStore((s) => s.toast);
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
    <Section
      title="Link đang mở"
      right={<button className="btn btn-ghost !h-7 !px-2.5 !text-xs -mt-1" onClick={() => guard(async () => {
        const p = await api.pick_folder({ title: "Chọn folder để chia sẻ" });
        if (p) openShare(p);
      })}><Icon name="plus" className="w-3.5 h-3.5" />Tạo link…</button>}
    >
      {shares === null ? (
        <div className="text-sm text-mute">Đang tải…</div>
      ) : shares.length === 0 ? (
        <div className="text-sm text-mute">Chưa có link nào. File hoặc folder bạn tạo link sẽ nằm ở đây; bấm “Gỡ” để thu hồi link.</div>
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
                title="Thu hồi link: người có link không lấy được file nữa"
              >{confirm === s.alias ? "Bấm lại để gỡ" : "Gỡ"}</button>
            </li>
          ))}
        </ul>
      )}
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
  // Port field keeps the raw text so typing "41820" is not clamped halfway; it commits when valid.
  const [portText, setPortText] = useState(app.host?.bind_port != null ? String(app.host.bind_port) : "");
  useEffect(() => setPortText(app.host?.bind_port != null ? String(app.host.bind_port) : ""), [app.host?.bind_port]);
  const portBad = portText !== "" && !(Number(portText) >= 1024 && Number(portText) <= 65535);
  const [fwAsk, setFwAsk] = useState(false);

  /** First fixed folder: serving turns on with it (no "role" switch any more, v0.6). */
  async function addFirst() {
    const p = await pick("Chọn folder cố định");
    if (!p) return;
    try {
      const cfg: HostConfig = { roots: [{ alias: suggestAlias(p), path: p }], return_root: `${app.dest_dir}${sep}Tra-ve`, max_parallel_jobs: 2, bind_port: DEFAULT_HOST_PORT };
      await api.set_host({ config: cfg });
      await refreshApp();
      api.get_diagnostics().then((d) => { if (d.firewall === "missing") setFwAsk(true); }).catch(() => undefined);
    } catch (e) { toast(errorMessage(e), "error"); }
  }

  async function pick(title: string): Promise<string | null> {
    try { return await api.pick_folder({ title }); } catch (e) { toast(errorMessage(e), "error"); return null; }
  }

  async function save() {
    if (!draft) return;
    const aliases = draft.roots.map((r) => r.alias);
    if (draft.roots.some((r) => !ALIAS_RE.test(r.alias))) { toast("Tên ngắn chỉ gồm chữ, số, - và _ (tối đa 32 ký tự)", "error"); return; }
    if (new Set(aliases).size !== aliases.length) { toast("Tên ngắn bị trùng", "error"); return; }
    if (draft.roots.some((r) => !r.path)) { toast("Chọn đường dẫn cho mỗi folder", "error"); return; }
    if (!draft.return_root) { toast("Chọn thư mục nhận file gửi lại", "error"); return; }
    if (portBad) { toast("Cổng UDP phải từ 1024 đến 65535", "error"); return; }
    try {
      // no fixed folder left and no open links: stop serving altogether
      const off = draft.roots.length === 0 && (await api.list_quick_shares()).length === 0;
      await api.set_host({ config: off ? null : draft }); await refreshApp(); toast("Đã lưu");
    } catch (e) { toast(errorMessage(e), "error"); }
  }

  return (
    <>
    <Section title="Folder cố định">
      <Hint className="mb-3">Chỉ cần khi muốn tạo link cho folder con bên trong, hoặc cho máy khác tạo link từ xa (Link điều khiển). Gửi file bình thường không cần mục này.</Hint>
      {!draft && <button className="btn w-full" onClick={() => void addFirst()}><Icon name="plus" />Thêm folder cố định…</button>}

      {draft && (
        <div className="space-y-3">
          <div>
            <div className="label">Folder (tên ngắn + đường dẫn)</div>
            <div className="space-y-2">
              {draft.roots.map((r, i) => (
                <div key={i} className="grid grid-cols-[96px_1fr_auto_auto] gap-2 items-center">
                  <input
                    className={`input ${r.alias && !ALIAS_RE.test(r.alias) ? "border-bad" : ""}`} placeholder="Tên ngắn" value={r.alias}
                    onChange={(e) => setDraft({ ...draft, roots: draft.roots.map((x, j) => (j === i ? { ...x, alias: e.target.value } : x)) })}
                  />
                  <div className="input flex items-center mono text-xs truncate" title={r.path}>{r.path || <span className="text-mute">Chưa chọn</span>}</div>
                  <button className="btn btn-icon" title="Chọn thư mục" onClick={async () => {
                    const p = await pick(`Folder cho “${r.alias || "mới"}”`);
                    if (p) setDraft({ ...draft, roots: draft.roots.map((x, j) => (j === i ? { ...x, path: p, alias: x.alias || suggestAlias(p) } : x)) });
                  }}><Icon name="folder" /></button>
                  <button className="btn btn-icon btn-ghost btn-danger" title="Xoá" onClick={() => setDraft({ ...draft, roots: draft.roots.filter((_, j) => j !== i) })}><Icon name="x" /></button>
                </div>
              ))}
              <button className="btn" onClick={async () => {
                const p = await pick("Thêm folder cố định");
                if (p) setDraft({ ...draft, roots: [...draft.roots, { alias: suggestAlias(p), path: p }] });
              }}><Icon name="plus" />Thêm folder</button>
            </div>
          </div>

          <div>
            <label className="label">File người khác gửi lại qua link sẽ vào đây</label>
            <div className="flex gap-2">
              <div className="input flex items-center mono text-xs truncate" title={draft.return_root}>{draft.return_root || "Chưa chọn"}</div>
              <button className="btn shrink-0" onClick={async () => { const p = await pick("Thư mục nhận file gửi lại"); if (p) setDraft({ ...draft, return_root: p }); }}><Icon name="folder" />Đổi…</button>
            </div>
          </div>

          <Row title="Số lượt nhận cùng lúc" hint="Quá số này, người tải sau phải chờ">
            <div className="flex items-center gap-2">
              <button className="btn btn-icon" onClick={() => setDraft({ ...draft, max_parallel_jobs: Math.max(1, draft.max_parallel_jobs - 1) })}>−</button>
              <span className="w-6 text-center mono">{draft.max_parallel_jobs}</span>
              <button className="btn btn-icon" onClick={() => setDraft({ ...draft, max_parallel_jobs: Math.min(16, draft.max_parallel_jobs + 1) })}>+</button>
            </div>
          </Row>

          <Row title="Cổng UDP" hint={`Mặc định ${DEFAULT_HOST_PORT}. Mở cổng này trên router thì máy ở xa nối thẳng, nhanh hơn`}>
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
      {(app.host?.roots.length ?? 0) > 0 && (
        <Section title="Tạo link trong folder cố định">
          <LinkMaker backend={backend} onCreated={() => setJobsKey((k) => k + 1)} />
        </Section>
      )}

      <Section title="Mọi link đã tạo">
        <JobList load={load} revoke={revoke} reloadKey={jobsKey} />
      </Section>

      <Section title="Link điều khiển">
        <p className="text-sm text-mute mb-3">
          Dán link này vào một máy khác (vd. máy của bạn ở nhà) để từ đó xem folder cố định và tạo link trên máy này mà không cần ngồi tại đây — kể cả bảo máy này tự cập nhật.
          Chỉ đưa cho máy bạn tin: ai có link đều tạo được link lấy file. Hết hạn thì tự vô hiệu.
        </p>
        <div className="flex items-center gap-2">
          <select className="input w-32" value={adminDays} onChange={(e) => setAdminDays(Number(e.target.value))}>
            <option value={7}>7 ngày</option><option value={30}>30 ngày</option><option value={90}>90 ngày</option><option value={365}>1 năm</option>
          </select>
          <button className="btn btn-primary" onClick={async () => {
            try { setAdmin(await api.create_admin_ticket({ days: adminDays })); } catch (e) { toast(errorMessage(e), "error"); }
          }}><Icon name="shield" />Tạo link điều khiển<Nest /></button>
        </div>
        {admin && <div className="mt-3"><QrLink info={admin} caption="Trên máy kia: Cài đặt → Chẩn đoán & nâng cao → Điều khiển máy khác → dán link" /></div>}
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
      const timeout = new Promise<never>((_, rej) => setTimeout(() => rej(new Error("Máy kia không phản hồi (đang offline?)")), 25000));
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
        {loading ? <span className="text-mute">Đang kết nối…</span> : status ? (
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
            <div className="h-section">Link trên máy đó</div>
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

  // The admin ticket lives in the shell settings (never dial the host just to find out).
  useEffect(() => {
    api.has_admin_ticket().then(setHas).catch(() => setHas(false));
  }, []);

  if (has === null) return <Section title="Điều khiển máy khác"><div className="text-sm text-mute">Đang kiểm tra…</div></Section>;

  async function connect() {
    setBusy(true);
    try {
      const info = await api.parse_ticket({ text });
      if (!info.admin) throw new Error("Đây không phải link điều khiển");
      await api.set_admin_ticket({ ticket: info.ticket });
      setText(""); setHas(true); toast("Đã kết nối — điều khiển được máy đó từ đây");
    } catch (e) { toast(errorMessage(e), "error"); }
    setBusy(false);
  }

  return (
    <Section
      title="Điều khiển máy khác"
      right={has ? <button className="btn btn-ghost btn-danger" onClick={async () => { try { await api.set_admin_ticket({ ticket: null }); setHas(false); } catch (e) { toast(errorMessage(e), "error"); } }}>Gỡ</button> : undefined}
    >
      {has ? <RemotePanel /> : (
        <div className="space-y-2">
          <p className="text-sm text-mute">Dán “Link điều khiển” tạo trên máy kia để từ đây xem folder cố định, tạo link và cập nhật máy đó — tiện cho máy không có người ngồi.</p>
          <textarea className="input h-20 py-2 mono text-xs resize-none" placeholder="Dán link điều khiển (toss1…)" value={text} onChange={(e) => setText(e.target.value)} spellCheck={false} />
          <button className="btn btn-primary w-full" disabled={busy || !text.trim()} onClick={connect}>Kết nối<Nest /></button>
        </div>
      )}
    </Section>
  );
}

// ---------------------------------------------------------------------------
/** Everything that is not part of everyday use, in one collapsed block. */
function Advanced({ open, onToggle }: { open: boolean; onToggle: () => void }) {
  const app = useStore((s) => s.app)!;
  const reduce = useReducedMotion();
  return (
    <div className="mt-2 mb-4">
      <button
        type="button" aria-expanded={open} onClick={onToggle}
        className="w-full card px-3.5 py-3 flex items-center gap-3 text-left transition-colors duration-500 ease-aura hover:bg-line/[0.04] active:scale-[0.99]"
      >
        <span className="grid place-items-center w-9 h-9 rounded-full bg-line/10 text-mute shrink-0"><Icon name="settings" className="w-[18px] h-[18px]" /></span>
        <span className="min-w-0 flex-1">
          <span className="block text-base font-medium">Chẩn đoán &amp; nâng cao</span>
          <span className="block text-xs text-mute">Kết nối, folder cố định, link, điều khiển từ xa, xuất log</span>
        </span>
        <motion.span animate={{ rotate: open ? 180 : 0 }} transition={reduce ? { duration: 0 } : { type: "spring", stiffness: 420, damping: 24 }} className="grid text-mute">
          <Icon name="chevron" className="w-4 h-4" weight="bold" />
        </motion.span>
      </button>
      <AnimatePresence initial={false}>
        {open && (
          <motion.div
            key="adv" className="overflow-hidden"
            initial={reduce ? { opacity: 0 } : { height: 0, opacity: 0 }} animate={reduce ? { opacity: 1 } : { height: "auto", opacity: 1 }}
            exit={reduce ? { opacity: 0 } : { height: 0, opacity: 0 }} transition={{ duration: reduce ? 0 : 0.5, ease: EASE }}
          >
            <div className="pt-3">
              <ConnectionSection />
              <OpenLinks />
              <HostEditor />
              {app.host && <LocalHostTools />}
              <RemoteSection />
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

export function Settings() {
  const app = useStore((s) => s.app);
  const diagFocus = useStore((s) => s.diagFocus);
  const [adv, setAdv] = useState(false);
  // "Vì sao chậm?" on a transfer jumps to Kết nối, which lives under Chẩn đoán & nâng cao
  useEffect(() => { if (diagFocus && Date.now() - diagFocus < 5000) setAdv(true); }, [diagFocus]);
  if (!app) return null;
  return (
    <div className="p-4">
      <Words text="Cài đặt" accent={["đặt"]} className="text-[30px] mb-4" />
      <DeviceSection />
      <ReceiveSection />
      <UpdateSection />
      <Advanced open={adv} onToggle={() => setAdv(!adv)} />
    </div>
  );
}
