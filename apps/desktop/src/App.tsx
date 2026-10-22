import { useEffect } from "react";
import { api, errorMessage, isMock, on } from "./lib/ipc";
import { Screen, useStore } from "./lib/store";
import { Home } from "./screens/Home";
import { Transfers } from "./screens/Transfers";
import { Settings } from "./screens/Settings";
import { TicketSheet } from "./components/TicketSheet";
import { Wizard } from "./components/Wizard";
import { Icon } from "./components/ui";

const TABS: { id: Screen; label: string; icon: string }[] = [
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
      await reg(on("deep-link", ({ ticket }) => { void useStore.getState().openTicket(ticket); }));
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
        } else {
          s.toast("Mở link trả hàng trước, rồi kéo folder vào");
        }
      }));
      try { await api.ui_ready(); } catch { /* older shells */ }
    })();
    return () => { dead = true; offs.forEach((f) => f()); };
  }, []);

  const active = Object.values(transfers).filter((t) => t.status === "running" || t.status === "connecting" || t.status === "paused").length;

  if (!app) return <div className="h-full grid place-items-center text-mute text-sm">Đang khởi động…</div>;

  return (
    <div className="h-full flex flex-col">
      {isMock && <div className="text-center text-[10px] leading-4 bg-warn/20 text-warn">MOCK — dữ liệu giả lập</div>}
      <main className="flex-1 overflow-y-auto">
        {screen === "home" && <Home />}
        {screen === "transfers" && <Transfers />}
        {screen === "settings" && <Settings />}
      </main>

      <nav className="grid grid-cols-3 border-t border-line bg-card shrink-0">
        {TABS.map((t) => (
          <button key={t.id} onClick={() => setScreen(t.id)} className={`relative flex flex-col items-center gap-0.5 py-2 text-xs ${screen === t.id ? "text-accent" : "text-mute hover:text-ink"}`}>
            <Icon name={t.icon} className="w-5 h-5" />
            {t.label}
            {t.id === "transfers" && active > 0 && (
              <span className="absolute top-1 left-1/2 ml-2 min-w-[16px] h-4 px-1 rounded-full bg-accent text-accent-ink text-[10px] leading-4 text-center">{active}</span>
            )}
          </button>
        ))}
      </nav>

      <TicketSheet />
      {app.first_run && <Wizard />}

      {dropHover && !sheet && (
        <div className="fixed inset-0 z-20 pointer-events-none border-4 border-dashed border-accent bg-accent/10 grid place-items-center">
          <div className="card px-4 py-2 text-base font-medium">Thả để trả file</div>
        </div>
      )}

      <div className="fixed left-0 right-0 bottom-16 z-50 flex flex-col items-center gap-2 pointer-events-none px-4">
        {toasts.map((t) => (
          <div key={t.id} className={`card px-3 py-2 text-sm shadow-lg max-w-full ${t.kind === "error" ? "border-bad text-bad" : ""}`}>{t.text}</div>
        ))}
      </div>
    </div>
  );
}
