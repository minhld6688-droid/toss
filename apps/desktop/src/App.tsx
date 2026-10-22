import { useEffect } from "react";
import { api, errorMessage, isMock, on } from "./lib/ipc";
import { Screen, useStore } from "./lib/store";
import { Home } from "./screens/Home";
import { Transfers } from "./screens/Transfers";
import { Settings } from "./screens/Settings";
import { TicketSheet } from "./components/TicketSheet";
import { ShareAsk, ShareView } from "./components/ShareView";
import { Wizard } from "./components/Wizard";
import { AnimatePresence, Icon } from "./components/ui";

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
            .then(async () => { await s.refreshTransfers(); s.closeSheet(); s.setScreen("transfers"); s.toast("Đang trả file…"); })
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
      <span className="relative">Đang khởi động…</span>
    </div>
  );

  return (
    <div className="relative h-full flex flex-col">
      <div className="aura" /><div className="grain" />
      {isMock && <div className="relative z-10 text-center text-[10px] leading-4 tracking-wide bg-warn/15 text-warn">MOCK — dữ liệu giả lập</div>}
      <main className="relative z-10 flex-1 overflow-y-auto pb-20">
        {screen === "home" && <Home />}
        {screen === "transfers" && <Transfers />}
        {screen === "settings" && <Settings />}
      </main>

      <nav className="fixed bottom-3 left-0 right-0 z-20 flex justify-center pointer-events-none">
        <div className="glass pointer-events-auto rounded-full p-1 flex gap-0.5">
          {TABS.map((t) => {
            const on = screen === t.id;
            return (
              <button
                key={t.id} onClick={() => setScreen(t.id)}
                className={`relative flex items-center gap-1.5 h-10 px-3.5 rounded-full text-xs font-medium transition-[background-color,color,transform] duration-500 ease-aura active:scale-95 ${on ? "bg-accent text-accent-ink" : "text-mute hover:text-ink hover:bg-line/10"}`}
                style={on ? { boxShadow: "inset 0 1px 0 rgb(255 255 255 / 0.28), 0 8px 24px -10px rgb(var(--accent) / 0.8)" } : undefined}
              >
                <Icon name={t.icon} className="w-[18px] h-[18px]" weight={on ? "bold" : "regular"} />
                <span className={on ? "" : "sr-only"}>{t.label}</span>
                {t.id === "transfers" && active > 0 && (
                  <span className={`min-w-[18px] h-[18px] px-1 rounded-full text-[10px] leading-[18px] text-center font-semibold ${on ? "bg-accent-ink/20 text-accent-ink" : "bg-accent text-accent-ink"}`}>{active}</span>
                )}
              </button>
            );
          })}
        </div>
      </nav>

      <AnimatePresence>{sheet && <TicketSheet key="sheet" info={sheet} />}</AnimatePresence>
      <AnimatePresence>{shareAsk && !share && <ShareAsk key="share-ask" path={shareAsk} />}</AnimatePresence>
      <AnimatePresence>{share && <ShareView key={share.nonce} path={share.path} />}</AnimatePresence>
      {app.first_run && <Wizard />}

      {dropHover && !sheet && !share && (
        <div className="fixed inset-0 z-20 pointer-events-none grid place-items-center bg-accent/10">
          <div className="absolute inset-3 rounded-[24px] border-2 border-dashed border-accent/70" />
          <div className="glass rounded-full px-5 py-2.5 text-base font-medium">Thả để chia sẻ folder</div>
        </div>
      )}

      <div className="fixed left-0 right-0 bottom-20 z-50 flex flex-col items-center gap-2 pointer-events-none px-4">
        {toasts.map((t) => (
          <div key={t.id} className={`glass rise rounded-full px-4 py-2 text-sm max-w-full ${t.kind === "error" ? "text-bad" : ""}`}>{t.text}</div>
        ))}
      </div>
    </div>
  );
}
