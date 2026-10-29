import { useCallback, useEffect, useRef, useState } from "react";
import { useReducedMotion } from "motion/react";
import type { ConnectReport, FirewallState, Likelihood, NatInfo, NetConfig, NetDiagUi, PortmapState } from "../lib/ipc-types";
import { api, errorMessage } from "../lib/ipc";
import { baseName, midTruncate } from "../lib/format";
import { useStore } from "../lib/store";
import { Badge, Disclosure, Dot, Icon, Nest, Row, Section, Segmented, Toggle } from "./ui";
import { FirewallAllow } from "./Firewall";
import { LanBlockedCard } from "./LanBlocked";

type Tone = "ok" | "warn" | "bad" | "mute";

const LIKELIHOOD: Record<Likelihood, { label: string; tone: Tone; line: string }> = {
  high: { label: "Cao", tone: "ok", line: "Máy khác sẽ nối thẳng tới máy này, tốc độ tối đa." },
  medium: { label: "Trung bình", tone: "warn", line: "Hầu hết máy khác nối thẳng được. Mở cổng UDP trên router để chắc chắn hơn." },
  low: { label: "Thấp", tone: "bad", line: "Máy ở xa nhiều khả năng phải đi vòng qua relay, sẽ chậm." },
  unknown: { label: "Đang đo", tone: "mute", line: "Toss đang đo mạng, vài giây nữa có kết quả." },
};

const POLL_MS = 5000;

/** get_diagnostics every 5 s while the panel is mounted and the window is visible. */
function useDiagnostics() {
  const [diag, setDiag] = useState<NetDiagUi | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const busy = useRef(false);
  const alive = useRef(true);

  const load = useCallback(async () => {
    if (busy.current) return;
    busy.current = true;
    try {
      const d = await api.get_diagnostics();
      if (alive.current) { setDiag(d); setErr(null); }
    } catch (e) {
      if (alive.current) setErr(errorMessage(e));
    } finally { busy.current = false; }
  }, []);

  useEffect(() => {
    alive.current = true;
    void load();
    const visible = () => document.visibilityState === "visible";
    const t = window.setInterval(() => { if (visible()) void load(); }, POLL_MS);
    const onVis = () => { if (visible()) void load(); };
    document.addEventListener("visibilitychange", onVis);
    return () => { alive.current = false; window.clearInterval(t); document.removeEventListener("visibilitychange", onVis); };
  }, [load]);

  return { diag, err, reload: load };
}

function relayHost(url: string): string {
  if (!url) return "—";
  try { return new URL(url).host.replace(/\.$/, ""); } catch { return midTruncate(url, 22, 8); }
}

const PROTOCOLS: Record<string, string> = { upnp: "UPnP", pcp: "PCP", "nat-pmp": "NAT-PMP" };

function portmapView(p: PortmapState): { tone: Tone; text: string; sub?: string } {
  switch (p.status) {
    case "mapped": return { tone: "ok", text: "Đã mở cổng", sub: [p.external_addr, p.protocol && (PROTOCOLS[p.protocol] ?? p.protocol.toUpperCase())].filter(Boolean).join(" · ") || undefined };
    case "probing": return { tone: "mute", text: "Đang dò router…" };
    case "unavailable": return { tone: "warn", text: "Router không hỗ trợ" };
    default: return { tone: "mute", text: "Tắt" };
  }
}

function Fact({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-start justify-between gap-3 py-2 text-sm">
      <span className="text-mute shrink-0 pt-px">{label}</span>
      <div className="min-w-0 text-right">{children}</div>
    </div>
  );
}

function Status({ tone, children }: { tone: Tone; children: React.ReactNode }) {
  return <span className="inline-flex items-center gap-1.5 justify-end"><Dot tone={tone} />{children}</span>;
}

function NatChips({ nat }: { nat: NatInfo }) {
  return (
    <div className="flex flex-wrap justify-end gap-1">
      <Badge tone={nat.udp ? "ok" : "bad"}>{nat.udp ? "UDP thông" : "UDP bị chặn"}</Badge>
      {nat.mapping_varies_by_dest === true && <Badge tone="bad">NAT đối xứng</Badge>}
      {nat.mapping_varies_by_dest === false && <Badge tone="ok">NAT dễ</Badge>}
      {nat.global_v4 && <Badge><span className="mono font-medium">v4 {nat.global_v4}</span></Badge>}
      {nat.global_v6 && <Badge tone="ok">IPv6</Badge>}
      {nat.hairpin === true && <Badge>Hairpin</Badge>}
    </div>
  );
}

function firewallView(f: FirewallState): { tone: Tone; text: string } {
  if (f === "ok") return { tone: "ok", text: "Đã cho phép" };
  if (f === "missing") return { tone: "bad", text: "Chưa cho phép" };
  return { tone: "mute", text: "Chưa kiểm tra được" };
}

// ---------------------------------------------------------------------------
function TestConnect({ onDone }: { onDone: () => void }) {
  const toast = useStore((s) => s.toast);
  const [hasAdmin, setHasAdmin] = useState<boolean | null>(null);
  const [link, setLink] = useState("");
  const [testing, setTesting] = useState(false);
  const [report, setReport] = useState<ConnectReport | null>(null);
  const [more, setMore] = useState(false);
  useEffect(() => { api.has_admin_ticket().then(setHasAdmin).catch(() => setHasAdmin(false)); }, []);

  async function run() {
    setTesting(true); setReport(null);
    try {
      // An empty ticket = the stored admin ticket (kept in the shell, the UI never sees it).
      const ticket = hasAdmin ? "" : (await api.parse_ticket({ text: link })).ticket;
      setReport(await api.test_connect({ ticket }));
      onDone();
    } catch (e) { toast(errorMessage(e), "error"); }
    setTesting(false);
  }

  return (
    <div className="space-y-2.5">
      {hasAdmin === false && (
        <input
          className="input mono text-xs" placeholder="Dán một link Toss của máy kia (toss1… hoặc https://…)" spellCheck={false}
          value={link} onChange={(e) => setLink(e.target.value)} onKeyDown={(e) => e.key === "Enter" && link.trim() && !testing && void run()}
        />
      )}
      <button className="btn btn-primary w-full magnet" disabled={testing || hasAdmin === null || (hasAdmin === false && !link.trim())} onClick={run}>
        <Icon name="signal" />{testing ? "Đang thử nối… (tới 8 giây)" : "Thử nối tới máy đó"}<Nest />
      </button>
      {report && <TestResult r={report} more={more} onMore={() => setMore((m) => !m)} />}
    </div>
  );
}

function TestResult({ r, more, onMore }: { r: ConnectReport; more: boolean; onMore: () => void }) {
  const direct = r.ok && r.path === "direct";
  const label = !r.ok ? "Không nối được" : { direct: "Trực tiếp", relay: "Relay", mixed: "Hỗn hợp", unknown: "Chưa rõ" }[r.path];
  const tone = !r.ok ? "bad" : direct ? "ok" : r.path === "relay" ? "warn" : "mute";
  return (
    <div className="rounded-[14px] hair bg-line/[0.04] px-3 py-2.5 text-sm space-y-1.5">
      <div className="flex items-center gap-2 flex-wrap">
        <Badge tone={tone}>{label}</Badge>
        {r.rtt_ms != null && <span className="mono">{r.rtt_ms} ms</span>}
        {r.ok && <span className="text-xs text-mute">nối sau {r.connect_ms} ms</span>}
      </div>
      {r.ok && r.direct_after_ms != null && (
        <div className="text-xs text-mute">Chuyển sang đường trực tiếp sau {(r.direct_after_ms / 1000).toFixed(1)} giây.</div>
      )}
      {r.reason && <div className={tone === "bad" ? "text-bad" : "text-warn"}>{r.reason}</div>}
      {r.known_paths.length > 0 && (
        <Disclosure title={`Đường đã thấy (${r.known_paths.length})`} open={more} onToggle={onMore}>
          <ul className="mono text-[11px] text-mute selectable space-y-0.5 pb-1 break-all">
            {r.known_paths.slice(0, 8).map((p, i) => <li key={i}>{p}</li>)}
          </ul>
        </Disclosure>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
const splitUrls = (t: string) => t.split(/[\s,]+/).map((u) => u.trim()).filter(Boolean);

function Advanced() {
  const app = useStore((s) => s.app)!;
  const patch = useStore((s) => s.patchApp);
  const toast = useStore((s) => s.toast);
  const [draft, setDraft] = useState<NetConfig>(app.net);
  const [relayText, setRelayText] = useState(app.net.relay_urls.join(" "));
  const [busy, setBusy] = useState(false);
  const saved = JSON.stringify(app.net);
  useEffect(() => { setDraft(app.net); setRelayText(app.net.relay_urls.join(" ")); }, [saved, app.net]);

  const urls = splitUrls(relayText);
  const badUrl = urls.find((u) => !/^https?:\/\/[^\s/]+\.[^\s/]+|^https?:\/\/localhost/i.test(u));
  const net: NetConfig = { ...draft, relay_urls: urls };
  const dirty = JSON.stringify(net) !== saved;
  const t = draft.tuning;
  const setTuning = (p: Partial<NetConfig["tuning"]>) => setDraft({ ...draft, tuning: { ...t, ...p } });

  async function save() {
    if (badUrl) { toast("Địa chỉ relay phải có dạng https://relay.ten-mien.vn", "error"); return; }
    setBusy(true);
    try {
      await api.set_net({ net });
      patch({ net });
      const active = Object.values(useStore.getState().transfers).some((x) => x.status === "running" || x.status === "connecting");
      toast(active ? "Đã lưu. Sẽ áp dụng khi các lượt chuyển hiện tại xong" : "Đã lưu và áp dụng");
    } catch (e) { toast(errorMessage(e), "error"); }
    setBusy(false);
  }

  return (
    <div className="pt-1 space-y-2">
      <div>
        <label className="label">Relay riêng (tuỳ chọn)</label>
        <input
          className={`input mono text-xs ${badUrl ? "border-bad" : ""}`} placeholder="https://relay.ten-mien.vn" spellCheck={false}
          value={relayText} onChange={(e) => setRelayText(e.target.value)}
        />
        <div className="text-xs text-mute mt-1">Thử relay này trước; relay công cộng vẫn là dự phòng nên không bao giờ mất kết nối.</div>
      </div>
      <Row title="Tìm máy trong mạng LAN" hint="mDNS: cùng mạng thì nối thẳng, không cần relay">
        <Toggle on={draft.mdns} label="mDNS" onChange={(mdns) => setDraft({ ...draft, mdns })} />
      </Row>
      <Row title="Tự mở cổng trên router" hint="UPnP / NAT-PMP / PCP — giúp máy khác nối thẳng tới máy này">
        <Toggle on={draft.portmap} label="Mở cổng tự động" onChange={(portmap) => setDraft({ ...draft, portmap })} />
      </Row>
      <Row title="Luồng song song" hint="16 nhanh hơn trên đường xa, tốn RAM hơn">
        <Segmented label="Luồng song song" value={t.streams >= 16 ? 16 : 8} options={[{ value: 8, label: "8" }, { value: 16, label: "16" }]}
          onChange={(streams) => setTuning({ streams, streams_high_rtt: Math.max(t.streams_high_rtt, streams) })} />
      </Row>
      <Row title="Thuật toán nghẽn" hint="BBR nhanh hơn khi đường truyền xa">
        <Segmented label="Thuật toán nghẽn" value={t.congestion} options={[{ value: "bbr", label: "BBR" }, { value: "cubic", label: "Cubic" }]}
          onChange={(congestion) => setTuning({ congestion })} />
      </Row>
      <button className="btn btn-primary w-full" disabled={!dirty || busy} onClick={save}>{dirty ? "Lưu và áp dụng" : "Đã lưu"}</button>
      <div className="text-xs text-mute">Áp dụng bằng cách khởi động lại phần mạng của Toss khi không có lượt chuyển nào đang chạy.</div>
    </div>
  );
}

// ---------------------------------------------------------------------------
export function ConnectionSection() {
  const toast = useStore((s) => s.toast);
  const focus = useStore((s) => s.diagFocus);
  const { diag, err, reload } = useDiagnostics();
  const reduce = useReducedMotion();
  const setLan = useStore((s) => s.setLanBlocked);
  const lanBlocked = useStore((s) => s.lanBlocked);
  useEffect(() => { if (diag) setLan(!!diag.lan_blocked); }, [diag, setLan]);
  const rootRef = useRef<HTMLDivElement>(null);
  const [adv, setAdv] = useState(false);
  const [exporting, setExporting] = useState(false);

  // "Vì sao?" on a relay transfer lands here: wait for the screen change, then scroll.
  useEffect(() => {
    if (!focus || Date.now() - focus > 4000) return;
    const t = window.setTimeout(() => rootRef.current?.scrollIntoView({ behavior: reduce ? "auto" : "smooth", block: "start" }), 650);
    return () => window.clearTimeout(t);
  }, [focus, reduce]);

  async function exportLogs() {
    setExporting(true);
    try {
      const path = await api.export_logs();
      toast(`Đã xuất log: ${baseName(path)}`);
      api.open_path({ path: path.replace(/[\\/][^\\/]*$/, "") }).catch(() => undefined);
    } catch (e) { toast(errorMessage(e), "error"); }
    setExporting(false);
  }

  const like = diag ? LIKELIHOOD[diag.direct_likelihood] : null;
  const pm = diag ? portmapView(diag.portmap) : null;
  const fw = diag ? firewallView(diag.firewall) : null;

  return (
    <div ref={rootRef} className="scroll-mt-3">
      <Section title="Kết nối">
        <LanBlockedCard className="mb-3" />
        {!diag ? (
          err ? (
            <div className="flex items-center justify-between gap-2 text-sm">
              <span className="text-bad min-w-0 truncate">{err}</span>
              <button className="btn btn-icon shrink-0" aria-label="Thử lại" onClick={() => void reload()}><Icon name="refresh" /></button>
            </div>
          ) : <div className="text-sm text-mute">Đang đo mạng…</div>
        ) : (
          <div className="space-y-3">
            <div>
              <div className="flex items-center justify-between gap-2">
                <span className="text-sm text-mute">Khả năng nối trực tiếp</span>
                <Badge tone={like!.tone}>
                  {diag.direct_likelihood === "unknown" ? <span className="pulse-dot relative w-1.5 h-1.5 rounded-full bg-mute mr-1.5" /> : null}
                  {like!.label}
                </Badge>
              </div>
              <div className="text-xs text-mute mt-1">{like!.line}</div>
            </div>

            <div className="divide-y divide-line/10 -my-1">
              <Fact label="Relay">
                <Status tone={diag.relay_connected ? "ok" : "bad"}>{diag.relay_connected ? "Đã nối" : "Chưa nối"}</Status>
                <div className="mono text-xs text-mute selectable">
                  {relayHost(diag.relay_url)}{diag.nat.relay_latency_ms != null ? ` · ${diag.nat.relay_latency_ms} ms` : ""}
                  {diag.custom_relay ? " · relay riêng" : ""}
                </div>
              </Fact>
              <Fact label="Cổng UDP">
                {diag.bind_port != null ? <span className="mono selectable">{diag.bind_port}</span> : <span className="text-mute">ngẫu nhiên</span>}
              </Fact>
              <Fact label="Mở cổng tự động">
                <Status tone={pm!.tone}>{pm!.text}</Status>
                {pm!.sub && <div className="mono text-xs text-mute selectable">{pm!.sub}</div>}
              </Fact>
              <Fact label="Mạng (NAT)"><NatChips nat={diag.nat} /></Fact>
              <Fact label="Firewall"><Status tone={fw!.tone}>{fw!.text}</Status></Fact>
            </div>

            {diag.firewall === "missing" && (
              <div>
                <p className="text-xs text-mute mb-2">Firewall đang chặn kết nối UDP vào Toss: máy ở xa sẽ chỉ nối được qua relay (chậm).</p>
                <FirewallAllow compact onDone={() => window.setTimeout(() => void reload(), 400)} />
              </div>
            )}

            {diag.reasons.filter((r) => !(lanBlocked && /mạng cục bộ|mạng nội bộ/i.test(r))).length > 0 && (
              <ul className="space-y-2">
                {diag.reasons.filter((r) => !(lanBlocked && /mạng cục bộ|mạng nội bộ/i.test(r))).map((r, i) => (
                  <li key={i} className="flex gap-2 text-sm">
                    <Icon name="info" className="w-4 h-4 mt-0.5 shrink-0 text-warn" />
                    <span className="min-w-0">{r}</span>
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}

        <div className={`${diag ? "mt-4" : "mt-3"} space-y-3`}>
          <TestConnect onDone={() => void reload()} />
          <Disclosure
            title="Nâng cao" open={adv} onToggle={() => setAdv((a) => !a)}
            aside={<button className="btn" disabled={exporting} onClick={exportLogs}><Icon name="file" />{exporting ? "Đang xuất…" : "Xuất log"}</button>}
          ><Advanced /></Disclosure>
        </div>
      </Section>
    </div>
  );
}
