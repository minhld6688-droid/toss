import type React from "react";
import { useEffect, useMemo, useState } from "react";
import { useReducedMotion } from "motion/react";
import type { Contact, IncomingPush, InviteInfo } from "../lib/ipc-types";
import { api, errorMessage, isMock } from "../lib/ipc";
import { fmtAgo, fmtBytes, fmtDate, fmtExpiry, initials, PLATFORM_LABEL, shortId } from "../lib/format";
import { extractInvite, extractShort } from "../lib/ticket";
import { useStore } from "../lib/store";
import { Badge, Dot, EASE, Icon, Nest, Row, Segmented, Sheet, Toggle, motion } from "./ui";
import { centerOf } from "./Comet";

const platformIcon = (p: string) => (p === "macos" ? "laptop" : "desktop");

/** Initials in a circle; the presence light sits on its edge (green = online, grey = offline). */
export function Avatar({ c, size = 40 }: { c: Pick<Contact, "name" | "online">; size?: number }) {
  return (
    <div className="relative shrink-0" style={{ width: size, height: size }}>
      <div
        className={`grid place-items-center w-full h-full rounded-full font-semibold tracking-tight transition-colors duration-500 ease-aura ${c.online ? "bg-accent/15 text-accent" : "bg-line/10 text-mute"}`}
        style={{ fontSize: size * 0.36 }}
      >{initials(c.name)}</div>
      <span className="absolute -right-0.5 -bottom-0.5 grid place-items-center w-3.5 h-3.5 rounded-full bg-bg">
        <Dot tone={c.online ? "ok" : "mute"} />
      </span>
    </div>
  );
}

function presenceLine(c: Contact) {
  return c.online ? (
    <>
      <span className="text-ok font-medium">Online</span>
      {c.rtt_ms != null && <span className="mono"> · {c.rtt_ms} ms</span>}
    </>
  ) : <>Offline · {c.last_seen ? `thấy ${fmtAgo(c.last_seen)}` : fmtAgo(0)}</>;
}

// ---------------------------------------------------------------------------------------------
// Home section
// ---------------------------------------------------------------------------------------------
function ContactRow({ c, i }: { c: Contact; i: number }) {
  const dropHover = useStore((s) => s.dropHover);
  const hovered = useStore((s) => s.hoverContact === c.id);
  const waiting = useStore((s) => s.pushes.some((p) => p.from_id === c.id));
  const openContact = useStore((s) => s.openContact);
  const setHover = useStore((s) => s.setHoverContact);
  const sendTo = useStore((s) => s.sendToContact);
  const target = dropHover && !hovered;
  const reduce = useReducedMotion();

  return (
    <li className="rise" style={{ "--i": 3 + i } as React.CSSProperties}>
      <div
        role="button" tabIndex={0} data-contact-id={c.id} aria-label={`${c.name}, ${c.online ? "online" : "offline"}`}
        onClick={() => openContact(c.id)}
        onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openContact(c.id); } }}
        // mock mode has no native drag events: plain HTML5 drag & drop stands in for them
        onDragOver={isMock ? (e) => { e.preventDefault(); setHover(c.id); } : undefined}
        onDragLeave={isMock ? () => setHover(null) : undefined}
        onDrop={isMock ? (e) => {
          e.preventDefault(); setHover(null);
          const names = Array.from(e.dataTransfer.files).map((f) => f.name);
          void sendTo(c.id, names.length ? names : ["Folder demo"], centerOf(e.currentTarget));
        } : undefined}
        className={`group relative flex items-center gap-3 px-3.5 py-2.5 cursor-pointer outline-none transition-[background-color,box-shadow] duration-500 ease-aura focus-visible:bg-line/[0.06] ${hovered ? "bg-accent/15 shadow-[inset_0_0_0_2px_rgb(var(--accent))]" : target ? "bg-accent/[0.06]" : "hover:bg-line/[0.05]"}`}
      >
        <motion.div animate={{ scale: hovered && !reduce ? 1.12 : 1 }} transition={reduce ? { duration: 0 } : { type: "spring", stiffness: 420, damping: 18 }}>
          <Avatar c={c} />
        </motion.div>
        <div className="min-w-0 flex-1">
          <div className="truncate text-base font-medium transition-transform duration-500 ease-aura group-hover:translate-x-0.5">{c.name}</div>
          <div className="flex items-center gap-1.5 text-xs text-mute min-w-0">
            <Icon name={platformIcon(c.platform)} className="w-3.5 h-3.5 shrink-0" />
            <span className="truncate">{presenceLine(c)}</span>
          </div>
        </div>
        {hovered ? (
          <span className="inline-flex items-center gap-1 text-xs font-semibold text-accent shrink-0"><Icon name="up" className="w-4 h-4 drip up" weight="bold" />Thả để gửi</span>
        ) : waiting ? (
          <Badge tone="accent">Muốn gửi file</Badge>
        ) : (
          <Icon name="chevron" className="w-4 h-4 text-mute -rotate-90 opacity-60 transition-all duration-500 ease-aura group-hover:opacity-100 group-hover:translate-x-0.5" />
        )}
      </div>
    </li>
  );
}

export function ContactsSection() {
  const contacts = useStore((s) => s.contacts);
  const loaded = useStore((s) => s.contactsLoaded);
  const pushes = useStore((s) => s.pushes);
  const sheetOpen = useStore((s) => s.pushSheet);
  const dropHover = useStore((s) => s.dropHover);
  const openAdd = useStore((s) => s.openAddDevice);
  const setPushSheet = useStore((s) => s.setPushSheet);
  const refresh = useStore((s) => s.refreshContacts);
  useEffect(() => { void refresh(); }, [refresh]);

  const first = pushes[0];
  return (
    <section className="mb-4 rise" style={{ "--i": 0 } as React.CSSProperties}>
      <div className="flex items-center justify-between mb-2">
        <h2 className="h-section !mb-0">Danh bạ</h2>
        <button className="btn !h-8 !px-3" onClick={() => openAdd("invite")}><Icon name="plus" weight="bold" />Thêm máy</button>
      </div>

      {first && !sheetOpen && (
        <button
          className="w-full mb-2 rounded-[16px] bg-accent/10 hair border-accent/40 px-3.5 py-2.5 text-left flex items-center gap-2.5 active:scale-[0.99] transition-transform duration-300"
          onClick={() => setPushSheet(true)}
        >
          <span className="grid place-items-center w-8 h-8 rounded-full bg-accent/20 text-accent shrink-0"><Icon name="down" className="w-4 h-4" weight="bold" /></span>
          <span className="min-w-0 flex-1 text-sm"><b className="font-semibold">{first.from_name}</b> muốn gửi <span className="truncate">{first.job}</span>{pushes.length > 1 ? ` (+${pushes.length - 1})` : ""}</span>
          <span className="text-xs font-semibold text-accent shrink-0">Xem</span>
        </button>
      )}

      {contacts.length === 0 ? (
        loaded && (
          <div className="rounded-[20px] border border-dashed border-line/25 bg-line/[0.04] px-4 py-4 text-center">
            <span className="mx-auto mb-2 grid place-items-center w-10 h-10 rounded-full bg-accent/15 text-accent"><Icon name="contact" className="w-5 h-5" weight="bold" /></span>
            <div className="text-base font-medium">Chưa có máy nào trong danh bạ</div>
            <div className="text-xs text-mute mt-0.5 mb-3">Ghép một lần bằng link mời, sau đó kéo file thả vào tên máy là gửi.</div>
            <button className="btn btn-primary" onClick={() => openAdd("invite")}><Icon name="invite" />Mời máy khác</button>
          </div>
        )
      ) : (
        <ul className={`card divide-y divide-line/10 overflow-hidden transition-shadow duration-500 ease-aura ${dropHover ? "shadow-[0_0_0_1.5px_rgb(var(--accent)/0.55)]" : ""}`}>
          {contacts.map((c, i) => <ContactRow key={c.id} c={c} i={i} />)}
        </ul>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------------------------
// Contact detail sheet
// ---------------------------------------------------------------------------------------------
export function ContactSheet({ id }: { id: string }) {
  const c = useStore((s) => s.contacts.find((x) => x.id === id));
  const close = useStore((s) => s.closeContact);
  const refresh = useStore((s) => s.refreshContacts);
  const toast = useStore((s) => s.toast);
  const sendTo = useStore((s) => s.sendToContact);
  const [name, setName] = useState(c?.name ?? "");
  const [armed, setArmed] = useState(false);
  const [busy, setBusy] = useState(false);
  useEffect(() => { if (c) setName(c.name); }, [c?.name]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => { if (!c) close(); }, [c, close]);
  if (!c) return null;

  const dirty = name.trim() !== c.name && name.trim().length > 0;

  async function run(fn: () => Promise<unknown>, ok?: string) {
    setBusy(true);
    try { await fn(); await refresh(); if (ok) toast(ok); return true; }
    catch (e) { toast(errorMessage(e), "error"); return false; }
    finally { setBusy(false); }
  }

  async function pickAndSend(e: React.MouseEvent) {
    const from = centerOf(e.currentTarget);
    try {
      const p = await api.pick_folder({ title: `Chọn folder để gửi cho ${c!.name}` });
      if (p) await sendTo(c!.id, [p], from);
    } catch (err) { toast(errorMessage(err), "error"); }
  }

  return (
    <Sheet onClose={close} title={c.name}>
      <div className="flex items-center gap-3 mb-3">
        <Avatar c={c} size={52} />
        <div className="min-w-0">
          <div className="title text-[26px] leading-tight truncate">{c.name}</div>
          <div className="flex items-center gap-1.5 text-xs text-mute">
            <Icon name={platformIcon(c.platform)} className="w-3.5 h-3.5" />
            {PLATFORM_LABEL[c.platform] ?? c.platform} · {presenceLine(c)}
          </div>
        </div>
      </div>

      <button className="btn btn-primary btn-lg w-full magnet" onClick={pickAndSend}>
        <Icon name="up" weight="bold" />Gửi folder…<Nest />
      </button>
      <div className="text-xs text-mute text-center mt-1.5 mb-3">
        {c.online ? "Hoặc kéo file thả thẳng vào tên máy ở Trang chính." : `${c.name} đang offline: file sẽ nằm trong hàng chờ và tự gửi khi máy online.`}
      </div>

      <div className="rounded-[16px] hair bg-line/[0.04] px-3.5 py-2 mb-3">
        <label className="label !mb-1 pt-1" htmlFor="contact-name">Tên hiển thị</label>
        <div className="flex gap-2 pb-1.5">
          <input
            id="contact-name" className="input" value={name} maxLength={64} spellCheck={false}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter" && dirty) void run(() => api.rename_contact({ id: c.id, name: name.trim() }), "Đã đổi tên"); }}
          />
          <button className="btn btn-primary shrink-0" disabled={!dirty || busy} onClick={() => run(() => api.rename_contact({ id: c.id, name: name.trim() }), "Đã đổi tên")}>Lưu</button>
        </div>
        {c.their_name && c.their_name !== c.name && <div className="text-xs text-mute pb-1.5">Máy tự đặt tên là “{c.their_name}”.</div>}
        <div className="divide-y divide-line/10">
          <Row title="Tự nhận file" hint="Máy này gửi tới là nhận luôn, không hỏi lại">
            <Toggle on={c.auto_accept} label="Tự nhận file" disabled={busy} onChange={(on) => void run(() => api.set_contact_auto_accept({ id: c.id, on }))} />
          </Row>
        </div>
      </div>

      <dl className="text-xs text-mute grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 px-1 mb-3">
        <dt>Thêm vào</dt><dd className="text-right text-ink">{fmtDate(c.added_at)}</dd>
        <dt>Thấy lần cuối</dt><dd className="text-right text-ink">{c.online ? "Đang online" : fmtAgo(c.last_seen)}</dd>
        <dt>Mã máy</dt><dd className="text-right text-ink mono selectable">{shortId(c.id, 12)}</dd>
      </dl>

      <button
        className={`btn w-full ${armed ? "btn-danger !bg-bad/10" : "btn-danger"}`} disabled={busy}
        onClick={async () => {
          if (!armed) { setArmed(true); window.setTimeout(() => setArmed(false), 3500); return; }
          if (await run(() => api.remove_contact({ id: c.id }), `Đã xoá ${c.name} khỏi danh bạ`)) close();
        }}
      ><Icon name="trash" />{armed ? "Bấm lần nữa để xoá" : "Xoá khỏi danh bạ"}</button>
      <button className="btn btn-ghost w-full mt-2" onClick={close}>Đóng</button>
    </Sheet>
  );
}

// ---------------------------------------------------------------------------------------------
// "Thêm máy" sheet
// ---------------------------------------------------------------------------------------------
function InviteQr({ info, onRevoked }: { info: InviteInfo; onRevoked: () => void }) {
  const toast = useStore((s) => s.toast);
  const [svg, setSvg] = useState("");
  const [copied, setCopied] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const exp = fmtExpiry(info.exp);

  useEffect(() => {
    let alive = true;
    api.qr_svg({ text: info.web_url }).then((s) => alive && setSvg(s)).catch((e) => toast(errorMessage(e), "error"));
    return () => { alive = false; };
  }, [info.web_url, toast]);

  async function copy(what: string, text: string) {
    try {
      await api.copy_text({ text });
      setCopied(what);
      setTimeout(() => setCopied((c) => (c === what ? null : c)), 1800);
    } catch (e) { toast(errorMessage(e), "error"); }
  }

  return (
    <div className="space-y-2.5">
      <div className="rounded-[16px] hair bg-line/[0.04] p-3">
        <div className="flex gap-3 items-start">
          <div className="w-[120px] h-[120px] shrink-0 rounded-[12px] bg-white p-1.5 overflow-hidden shadow-[0_8px_24px_-12px_rgb(var(--accent)/0.6)]" dangerouslySetInnerHTML={{ __html: svg }} aria-label="Mã QR lời mời" />
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-1.5 mb-1.5">
              <Badge tone={exp.expired ? "bad" : "ok"}>{exp.text}</Badge>
              <span className="text-[11px] text-mute">ghép được nhiều máy</span>
            </div>
            <div className="selectable text-xs mono break-all line-clamp-3 text-ink" title={info.web_url}>{info.web_url}</div>
            <div className="flex flex-wrap gap-2 mt-2">
              <button className="btn btn-primary" onClick={() => copy("web", info.web_url)}>
                <Icon name={copied === "web" ? "check" : "copy"} weight="bold" />{copied === "web" ? "Đã chép" : "Sao chép link"}
              </button>
              <button className="btn btn-ghost" onClick={() => copy("raw", info.invite)} title="Chép mã tossinv1… (dán vào tab Nhập lời mời)">
                <Icon name={copied === "raw" ? "check" : "link"} />{copied === "raw" ? "Đã chép" : "Mã"}
              </button>
            </div>
          </div>
        </div>
      </div>
      <div className="text-xs text-mute px-1">Gửi link hoặc cho máy kia quét mã. Mở link trong Toss là hai máy nhớ nhau, chỉ cần làm một lần.</div>
      <button
        className="btn btn-danger w-full" disabled={busy}
        onClick={async () => {
          setBusy(true);
          try { await api.revoke_invites(); toast("Đã huỷ mọi lời mời"); onRevoked(); }
          catch (e) { toast(errorMessage(e), "error"); }
          setBusy(false);
        }}
      ><Icon name="x" />Huỷ mọi lời mời</button>
    </div>
  );
}

function InviteTab() {
  const toast = useStore((s) => s.toast);
  const [days, setDays] = useState(7);
  const [info, setInfo] = useState<InviteInfo | null>(null);
  const [busy, setBusy] = useState(false);

  async function create() {
    setBusy(true);
    try { setInfo(await api.create_invite({ days })); toast("Đã tạo lời mời, link đã copy"); }
    catch (e) { toast(errorMessage(e), "error"); }
    setBusy(false);
  }

  return (
    <div className="space-y-3">
      {!info && <p className="text-sm text-mute">Tạo một lời mời rồi gửi cho máy kia (máy ở studio, máy ở nhà, máy của đồng nghiệp…). Mỗi lời mời ghép được nhiều máy cho tới khi hết hạn hoặc bị huỷ.</p>}
      <div className="flex items-center justify-between gap-3">
        <span className="text-sm text-mute">Hiệu lực</span>
        <Segmented label="Hiệu lực lời mời" value={days} onChange={setDays} options={[{ value: 1, label: "1 ngày" }, { value: 7, label: "7 ngày" }, { value: 30, label: "30 ngày" }]} />
      </div>
      <button className={`btn ${info ? "" : "btn-primary"} btn-lg w-full magnet`} disabled={busy} onClick={create}>
        <Icon name={info ? "refresh" : "qr"} />{busy ? "Đang tạo…" : info ? "Tạo lời mời mới" : "Tạo lời mời"}{!info && <Nest />}
      </button>
      {info && <InviteQr info={info} onRevoked={() => setInfo(null)} />}
    </div>
  );
}

function EnterTab({ initial }: { initial: string }) {
  const toast = useStore((s) => s.toast);
  const refresh = useStore((s) => s.refreshContacts);
  const close = useStore((s) => s.closeAddDevice);
  const [text, setText] = useState(initial);
  const [info, setInfo] = useState<InviteInfo | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const invite = useMemo(() => extractInvite(text), [text]);
  const short = useMemo(() => (invite ? null : extractShort(text)), [invite, text]);

  // v0.5.2: a pasted short link is resolved to the invite it stands for.
  useEffect(() => {
    if (!short) return;
    let alive = true;
    api.expand_link({ text: short })
      .then((long) => {
        if (!alive) return;
        if (extractInvite(long)) setText(long);
        else setErr("Link này là link job, không phải lời mời kết nối");
      })
      .catch((e) => alive && setErr(errorMessage(e)));
    return () => { alive = false; };
  }, [short]);

  useEffect(() => {
    setInfo(null); setErr(null);
    if (!invite) return;
    let alive = true;
    api.parse_invite({ text: invite })
      .then((i) => alive && setInfo(i))
      .catch((e) => alive && setErr(errorMessage(e)));
    return () => { alive = false; };
  }, [invite]);

  async function paste() {
    try { setText(await navigator.clipboard.readText()); }
    catch { toast("Nhấn Ctrl/⌘+V để dán lời mời", "info"); }
  }

  async function connect() {
    if (!invite) return;
    setBusy(true);
    try {
      const c = await api.accept_invite({ text: invite });
      await refresh();
      toast(`Đã thêm ${c.name} vào danh bạ`);
      close();
    } catch (e) { setErr(errorMessage(e)); }
    setBusy(false);
  }

  return (
    <div className="space-y-3">
      {!info && <p className="text-sm text-mute">Dán link mời (toss://i/…) hoặc mã tossinv1… mà máy kia gửi cho bạn.</p>}
      {!info && (
        <div className="flex gap-2">
          <input
            className="input mono text-xs" placeholder="Dán link mời / mã tossinv1…" spellCheck={false} autoFocus
            value={text} onChange={(e) => setText(e.target.value)}
          />
          <button className="btn shrink-0" onClick={paste}><Icon name="copy" />Dán</button>
        </div>
      )}
      {text.trim() && !invite && <div className="text-sm text-bad">Không thấy lời mời Toss trong nội dung này.</div>}
      {err && <div className="rounded-[12px] bg-bad/10 text-bad text-sm px-3 py-2">{err}</div>}

      {info && (
        <motion.div
          className="rounded-[18px] hair bg-accent/[0.07] p-4 text-center"
          initial={{ opacity: 0, scale: 0.97 }} animate={{ opacity: 1, scale: 1 }} transition={{ duration: 0.5, ease: EASE }}
        >
          <div className="mx-auto mb-2 flex justify-center"><Avatar c={{ name: info.name, online: true }} size={52} /></div>
          <div className="title text-[24px] leading-tight">Kết nối với <em>{info.name}</em>?</div>
          <div className="text-xs text-mute mt-1">Hai máy sẽ nhớ nhau: sau này gửi file qua lại không cần link. {fmtExpiry(info.exp).text}.</div>
          <button className="btn btn-primary btn-lg w-full mt-3 magnet" disabled={busy} onClick={connect}>
            {busy ? "Đang kết nối…" : "Kết nối"}<Nest name="check" />
          </button>
          <button className="btn btn-ghost w-full mt-1.5" onClick={() => { setText(""); setInfo(null); }}>Không phải máy này</button>
        </motion.div>
      )}
    </div>
  );
}

export function AddDeviceSheet() {
  const add = useStore((s) => s.addDevice)!;
  const close = useStore((s) => s.closeAddDevice);
  const [tab, setTab] = useState<"invite" | "enter">(add.tab);
  useEffect(() => { setTab(add.tab); }, [add]);

  return (
    <Sheet onClose={close} title="Thêm máy">
      <div className="flex items-center justify-between gap-2 mb-3">
        <div className="title text-[26px]">Thêm <em>máy</em></div>
        <Segmented label="Cách thêm" value={tab} onChange={setTab} options={[{ value: "invite", label: "Mời" }, { value: "enter", label: "Nhập lời mời" }]} />
      </div>
      {tab === "invite" ? <InviteTab key="i" /> : <EnterTab key={`e-${add.text}`} initial={add.text} />}
      <button className="btn btn-ghost w-full mt-2" onClick={close}>Đóng</button>
    </Sheet>
  );
}

// ---------------------------------------------------------------------------------------------
// Incoming push sheet
// ---------------------------------------------------------------------------------------------
function pushLine(p: IncomingPush) {
  return `${p.files} file · ${fmtBytes(p.bytes)}`;
}

export function PushSheet() {
  const pushes = useStore((s) => s.pushes);
  const contacts = useStore((s) => s.contacts);
  const setOpen = useStore((s) => s.setPushSheet);
  const remove = useStore((s) => s.removePush);
  const refresh = useStore((s) => s.refreshTransfers);
  const fly = useStore((s) => s.fly);
  const setScreen = useStore((s) => s.setScreen);
  const toast = useStore((s) => s.toast);
  const reduce = useReducedMotion();
  const [busy, setBusy] = useState<string | null>(null);

  const p = pushes[0];
  useEffect(() => { if (!p) setOpen(false); }, [p, setOpen]);
  if (!p) return null;
  const from = contacts.find((c) => c.id === p.from_id);
  const rest = pushes.slice(1);

  async function answer(x: IncomingPush, accept: boolean, el?: Element | null) {
    setBusy(x.push_id);
    try {
      await api.answer_push({ push_id: x.push_id, accept });
      remove(x.push_id);
      if (accept) {
        await refresh();
        fly(el ? centerOf(el) : undefined);
        setScreen("transfers");
        toast(`Đang nhận ${x.job} từ ${x.from_name}…`);
      } else toast(`Đã từ chối ${x.job}`);
    } catch (e) { toast(errorMessage(e), "error"); }
    setBusy(null);
  }

  return (
    <Sheet onClose={() => setOpen(false)} title={`${p.from_name} muốn gửi file`}>
      <div className="flex items-center gap-3 mb-3">
        <div className="relative shrink-0">
          <Avatar c={{ name: p.from_name, online: true }} size={52} />
          {!reduce && (
            <motion.span
              className="absolute inset-0 rounded-full border-2 border-accent pointer-events-none"
              initial={{ scale: 1, opacity: 0.8 }} animate={{ scale: 1.6, opacity: 0 }}
              transition={{ duration: 1.8, ease: "easeOut", repeat: Infinity }}
            />
          )}
        </div>
        <div className="min-w-0">
          <div className="text-[11px] font-semibold uppercase tracking-[0.2em] text-mute">Yêu cầu gửi file</div>
          <div className="title text-[24px] leading-tight"><em className="block truncate">{p.from_name}</em>muốn gửi file</div>
          {from && <div className="text-xs text-mute flex items-center gap-1"><Icon name={platformIcon(from.platform)} className="w-3.5 h-3.5" />{PLATFORM_LABEL[from.platform] ?? from.platform}</div>}
        </div>
      </div>

      <div className="rounded-[16px] hair bg-line/[0.04] px-3.5 py-3 mb-3 flex items-center gap-3">
        <span className="grid place-items-center w-10 h-10 rounded-full bg-accent/15 text-accent shrink-0"><Icon name="folder" className="w-5 h-5" /></span>
        <div className="min-w-0 flex-1">
          <div className="truncate text-base font-semibold tracking-tight" title={p.job}>{p.job}</div>
          <div className="text-sm text-mute mono">{pushLine(p)}</div>
        </div>
      </div>

      <div className="flex gap-2">
        <button className="btn btn-primary btn-lg flex-1 magnet" disabled={busy !== null} onClick={(e) => answer(p, true, e.currentTarget)}>Nhận<Nest name="down" /></button>
        <button className="btn btn-lg" disabled={busy !== null} onClick={() => answer(p, false)}>Từ chối</button>
      </div>

      {rest.length > 0 && (
        <div className="mt-3">
          <div className="text-xs text-mute px-1 mb-1.5">Còn {rest.length} yêu cầu đang chờ</div>
          <ul className="card divide-y divide-line/10 overflow-hidden">
            {rest.map((x) => (
              <li key={x.push_id} className="flex items-center gap-2.5 px-3 py-2">
                <div className="min-w-0 flex-1">
                  <div className="truncate text-sm font-medium">{x.job}</div>
                  <div className="truncate text-xs text-mute">{x.from_name} · {pushLine(x)}</div>
                </div>
                <button className="btn btn-icon" aria-label={`Nhận ${x.job}`} disabled={busy !== null} onClick={(e) => answer(x, true, e.currentTarget)}><Icon name="check" weight="bold" /></button>
                <button className="btn btn-icon btn-danger" aria-label={`Từ chối ${x.job}`} disabled={busy !== null} onClick={() => answer(x, false)}><Icon name="x" weight="bold" /></button>
              </li>
            ))}
          </ul>
        </div>
      )}
      <button className="btn btn-ghost w-full mt-2" onClick={() => setOpen(false)}>Để sau</button>
    </Sheet>
  );
}
