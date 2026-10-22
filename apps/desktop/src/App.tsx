import { useEffect, useRef } from "react";
import { LayoutGroup, motion, useReducedMotion } from "motion/react";
import { api, errorMessage, isMock, on } from "./lib/ipc";
import { Screen, useStore } from "./lib/store";
import { Home } from "./screens/Home";
import { Transfers } from "./screens/Transfers";
import { Settings } from "./screens/Settings";
import { TicketSheet } from "./components/TicketSheet";
import { ShareAsk, ShareView } from "./components/ShareView";
import { Wizard } from "./components/Wizard";
import { AnimatePresence, EASE, Icon } from "./components/ui";
import { CometLayer } from "./components/Comet";
import { TossMark } from "./components/TossMark";
import { usePointerFx } from "./lib/pointer-fx";

const TABS: { id: Screen; label: string; icon: "home" | "transfer" | "settings" }[] = [
  { id: "home", label: "Trang chính", icon: "home" },
  { id: "transfers", label: "Chuyển file", icon: "transfer" },
  { id: "settings", label: "Cài đặt", icon: "settings" },
];

export default function App() {
  const screen = useStore((s) => s.screen);
  const setScreen = useStore((s) => s.setScreen);
  const app = useStore((s) => s.app);
  const transfers = useStore((s) => s.transfers);
  const toasts = useStore((s) => s.toasts);
  const dropHover = useStore((s) => s.dropHover);
  const sheet = useStore((s) => s.sheet);
  const share = useStore((s) => s.share);
  const shareAsk = useStore((s) => s.shareAsk);
  const navPulse = useStore((s) => s.navPulse);
  const reduce = useReducedMotion();
  usePointerFx();

  // direction of the tab change drives the screen transition
  const order: Screen[] = ["home", "transfers", "settings"];
  const prev = useRef(screen);
  const dir = order.indexOf(screen) >= order.indexOf(prev.current) ? 1 : -1;
  useEffect(() => { prev.current = screen; }, [screen]);

  useEffect(() => {
    const st = useStore.getState();
    const offs: (() => void)[] = [];
    let dead = false;
    (async () => {
      try {
        st.setApp(await api.get_state());
        await st.refreshTransfers();
      } catch (e) { st.toast(errorMessage(e), "error"); }

      const reg = async (p: Promise<() => void>) => { const off = await p; if (dead) off(); else offs.push(off); };
      await reg(on("transfer:event", (ev) => useStore.getState().applyEvent(ev)));
      await reg(on("host:status", (h) => useStore.getState().setHostStatus(h)));
      await reg(on("deep-link", ({ ticket }) => { void useStore.getState().receiveLink(ticket); }));
      await reg(on("share", ({ path }) => useStore.getState().openShare(path)));
      await reg(on("share:error", ({ msg }) => useStore.getState().toast(msg, "error")));
      await reg(on("navigate", ({ screen }) => useStore.getState().setScreen(screen)));
      await reg(on("drop:hover", ({ on: v }) => useStore.getState().setDropHover(v)));
      await reg(on("drop", ({ paths }) => {
        const s = useStore.getState();
        s.setDropHover(false);
        const sheet = s.sheet;
        if (sheet && sheet.put && !sheet.admin) {
          api.start_put({ ticket: sheet.ticket, paths })
            .then(async () => { await s.refreshTransfers(); s.fly(); s.closeSheet(); s.setScreen("transfers"); s.toast("Đang trả file…"); })
            .catch((e) => s.toast(errorMessage(e), "error"));
        } else if (!sheet && !s.share && paths.length > 0) {
          // no ticket selected, not in a put flow: offer to share the dropped folder
          s.askShare(paths[0]);
        } else {
          s.toast("Mở link trả hàng trước, rồi kéo folder vào");
        }
      }));
      try { await api.ui_ready(); } catch { /* older shells */ }
    })();
    return () => { dead = true; offs.forEach((f) => f()); };
  }, []);

  const active = Object.values(transfers).filter((t) => t.status === "running" || t.status === "connecting" || t.status === "paused").length;

  if (!app) return (
    <div className="relative h-full grid place-items-center text-mute text-sm">
      <div className="aura" /><div className="grain" />
      <div className="relative flex flex-col items-center gap-3">
        <TossMark className="w-28" speed={1.4} />
        <span className="title text-lg text-mute"><em>Toss</em> đang khởi động…</span>
      </div>
    </div>
  );

  return (
    <div className="relative h-full flex flex-col">
      <div className="aura" /><div className="grain" />
      {isMock && <div className="relative z-10 text-center text-[10px] leading-4 tracking-wide bg-warn/15 text-warn">MOCK — dữ liệu giả lập</div>}
      <main className="relative z-10 flex-1 overflow-y-auto overflow-x-hidden pb-20">
        <AnimatePresence mode="popLayout" initial={false} custom={dir}>
          <motion.div
            key={screen} custom={dir}
            variants={{
              enter: (d: number) => (reduce ? { opacity: 0 } : { opacity: 0, x: 36 * d, scale: 0.985, filter: "blur(8px)" }),
              center: { opacity: 1, x: 0, scale: 1, filter: "blur(0px)" },
              exit: (d: number) => (reduce ? { opacity: 0 } : { opacity: 0, x: -28 * d, scale: 0.985, filter: "blur(6px)" }),
            }}
            initial="enter" animate="center" exit="exit"
            transition={{ duration: 0.55, ease: EASE }}
          >
            {screen === "home" && <Home />}
            {screen === "transfers" && <Transfers />}
            {screen === "settings" && <Settings />}
          </motion.div>
        </AnimatePresence>
      </main>

      <nav className="fixed bottom-3 left-0 right-0 z-20 flex justify-center pointer-events-none">
        <LayoutGroup id="tabs">
        <div className="glass pointer-events-auto rounded-full p-1 flex gap-0.5">
          {TABS.map((t) => {
            const on = screen === t.id;
            return (
              <button
                key={t.id} data-tab={t.id} onClick={() => setScreen(t.id)} aria-label={t.label} aria-current={on ? "page" : undefined}
                className={`relative flex items-center gap-1.5 h-10 px-3.5 rounded-full text-xs font-medium transition-colors duration-500 ease-aura active:scale-95 ${on ? "text-accent-ink" : "text-mute hover:text-ink"}`}
              >
                {on && (
                  <motion.span
                    layoutId="tab-pill" className="absolute inset-0 rounded-full bg-accent"
                    style={{ boxShadow: "inset 0 1px 0 rgb(255 255 255 / 0.28), 0 8px 24px -10px rgb(var(--accent) / 0.8)" }}
                    transition={reduce ? { duration: 0 } : { type: "spring", stiffness: 420, damping: 32, mass: 0.8 }}
                  />
                )}
                {t.id === "transfers" && navPulse > 0 && !reduce && (
                  <motion.span
                    key={navPulse} className="absolute inset-0 rounded-full border-2 border-accent pointer-events-none"
                    initial={{ opacity: 0.9, scale: 1 }} animate={{ opacity: 0, scale: 1.7 }} transition={{ duration: 0.7, ease: EASE }}
                  />
                )}
                <motion.span
                  key={t.id === "transfers" ? `p${navPulse}` : t.id} className="relative"
                  initial={t.id === "transfers" && navPulse > 0 && !reduce ? { scale: 1.5, rotate: -12 } : false}
                  animate={{ scale: 1, rotate: 0 }} transition={{ type: "spring", stiffness: 500, damping: 14 }}
                >
                  <Icon name={t.icon} className="w-[18px] h-[18px]" weight={on ? "fill" : "regular"} />
                </motion.span>
                <AnimatePresence initial={false}>
                  {on && (
                    <motion.span
                      className="relative overflow-hidden whitespace-nowrap"
                      initial={{ width: 0, opacity: 0 }} animate={{ width: "auto", opacity: 1 }} exit={{ width: 0, opacity: 0 }}
                      transition={{ duration: reduce ? 0 : 0.45, ease: EASE }}
                    >{t.label}</motion.span>
                  )}
                </AnimatePresence>
                {t.id === "transfers" && active > 0 && (
                  <motion.span
                    layout key="count" initial={{ scale: 0 }} animate={{ scale: 1 }} transition={{ type: "spring", stiffness: 600, damping: 18 }}
                    className={`relative min-w-[18px] h-[18px] px-1 rounded-full text-[10px] leading-[18px] text-center font-semibold mono ${on ? "bg-accent-ink/20 text-accent-ink" : "bg-accent text-accent-ink"}`}
                  >{active}</motion.span>
                )}
              </button>
            );
          })}
        </div>
        </LayoutGroup>
      </nav>

      <AnimatePresence>{sheet && <TicketSheet key="sheet" info={sheet} />}</AnimatePresence>
      <AnimatePresence>{shareAsk && !share && <ShareAsk key="share-ask" path={shareAsk} />}</AnimatePresence>
      <AnimatePresence>{share && <ShareView key={share.nonce} path={share.path} />}</AnimatePresence>
      {app.first_run && <Wizard />}

      <AnimatePresence>
        {dropHover && !sheet && !share && (
          <motion.div
            key="drop" className="fixed inset-0 z-20 pointer-events-none grid place-items-center"
            initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: 0.35, ease: EASE }}
          >
            <div className="absolute inset-0 bg-accent/10 backdrop-blur-[2px]" />
            <svg className="absolute inset-3 w-[calc(100%-24px)] h-[calc(100%-24px)] overflow-visible" aria-hidden>
              <rect x="1" y="1" width="calc(100% - 2px)" height="calc(100% - 2px)" rx="24" fill="none" strokeWidth="2" strokeDasharray="10 18" strokeLinecap="round" className="march" style={{ stroke: "rgb(var(--accent))" }} />
            </svg>
            {!reduce && [0, 1, 2].map((i) => (
              <motion.span
                key={i} className="absolute w-40 h-40 rounded-full border border-accent/50"
                initial={{ scale: 0.4, opacity: 0 }} animate={{ scale: 2.6, opacity: [0, 0.8, 0] }}
                transition={{ duration: 2.4, ease: "easeOut", repeat: Infinity, delay: i * 0.8 }}
              />
            ))}
            <motion.div
              className="relative glass rounded-[28px] px-6 pt-4 pb-4 flex flex-col items-center gap-1"
              initial={{ scale: 0.85, y: 16 }} animate={{ scale: 1, y: 0 }} exit={{ scale: 0.9, y: 8 }}
              transition={{ type: "spring", stiffness: 360, damping: 24 }}
            >
              <TossMark className="w-24" speed={1.8} />
              <div className="title text-xl">Thả để <em>chia sẻ</em></div>
              <div className="text-xs text-mute">Toss tạo link ngay, copy sẵn vào clipboard</div>
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>

      <div className="fixed left-0 right-0 bottom-20 z-50 flex flex-col items-center gap-2 pointer-events-none px-4">
        <AnimatePresence initial={false}>
          {toasts.map((t) => (
            <motion.div
              key={t.id} layout
              initial={reduce ? { opacity: 0 } : { opacity: 0, y: 24, scale: 0.9, filter: "blur(6px)" }}
              animate={{ opacity: 1, y: 0, scale: 1, filter: "blur(0px)" }}
              exit={reduce ? { opacity: 0 } : { opacity: 0, y: -10, scale: 0.96, filter: "blur(4px)", transition: { duration: 0.3, ease: EASE } }}
              transition={{ type: "spring", stiffness: 420, damping: 30 }}
              className={`glass rounded-full pl-3 pr-4 py-2 text-sm max-w-full flex items-center gap-2 ${t.kind === "error" ? "text-bad" : ""}`}
            >
              <span className={`pulse-dot w-1.5 h-1.5 rounded-full shrink-0 ${t.kind === "error" ? "bg-bad" : "bg-accent"}`} />
              {t.text}
            </motion.div>
          ))}
        </AnimatePresence>
      </div>
      <CometLayer />
    </div>
  );
}
