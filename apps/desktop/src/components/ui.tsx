import { ReactNode, useEffect } from "react";

export function Toggle({ on, onChange, disabled, label }: { on: boolean; onChange: (v: boolean) => void; disabled?: boolean; label?: string }) {
  return (
    <button
      role="switch" aria-checked={on} aria-label={label} disabled={disabled}
      onClick={() => onChange(!on)}
      className={`relative w-9 h-5 rounded-full transition-colors shrink-0 ${on ? "bg-accent" : "bg-line"} ${disabled ? "opacity-50" : ""}`}
    >
      <span className={`absolute top-0.5 left-0.5 w-4 h-4 rounded-full bg-white shadow transition-transform ${on ? "translate-x-4" : ""}`} />
    </button>
  );
}

export function Row({ title, hint, children }: { title: string; hint?: string; children?: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-3 min-h-[40px] py-1">
      <div className="min-w-0">
        <div className="text-base">{title}</div>
        {hint && <div className="text-xs text-mute">{hint}</div>}
      </div>
      {children}
    </div>
  );
}

export function Section({ title, children, right }: { title: string; children: ReactNode; right?: ReactNode }) {
  return (
    <section className="mb-4">
      <div className="flex items-center justify-between">
        <h2 className="h-section">{title}</h2>
        {right}
      </div>
      <div className="card p-3">{children}</div>
    </section>
  );
}

export function Progress({ value, tone = "accent" }: { value: number; tone?: "accent" | "ok" | "bad" | "mute" }) {
  const color = { accent: "bg-accent", ok: "bg-ok", bad: "bg-bad", mute: "bg-mute" }[tone];
  return (
    <div className="h-1.5 rounded-full bg-line overflow-hidden" role="progressbar" aria-valuenow={Math.round(value * 100)} aria-valuemin={0} aria-valuemax={100}>
      <div className={`h-full rounded-full transition-[width] duration-500 ease-linear ${color}`} style={{ width: `${Math.max(0, Math.min(1, value)) * 100}%` }} />
    </div>
  );
}

export function Sheet({ children, onClose, title }: { children: ReactNode; onClose: () => void; title?: string }) {
  useEffect(() => {
    const h = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [onClose]);
  return (
    <div className="fixed inset-0 z-30 flex flex-col justify-end bg-black/40" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="card rounded-b-none rounded-t-xl p-4 max-h-[92%] overflow-y-auto shadow-2xl" role="dialog" aria-label={title}>
        <div className="mx-auto w-8 h-1 rounded-full bg-line mb-3" />
        {children}
      </div>
    </div>
  );
}

export function Icon({ name, className = "w-4 h-4" }: { name: string; className?: string }) {
  const p: Record<string, string> = {
    home: "M3 11l9-8 9 8M5 10v10h5v-6h4v6h5V10",
    transfer: "M7 4v13m0 0l-3-3m3 3l3-3M17 20V7m0 0l-3 3m3-3l3 3",
    settings: "M12 15a3 3 0 100-6 3 3 0 000 6zM19.4 15a1.7 1.7 0 00.3 1.8l.1.1a2 2 0 11-2.8 2.8l-.1-.1a1.7 1.7 0 00-1.8-.3 1.7 1.7 0 00-1 1.5V21a2 2 0 11-4 0v-.1a1.7 1.7 0 00-1.1-1.5 1.7 1.7 0 00-1.8.3l-.1.1a2 2 0 11-2.8-2.8l.1-.1a1.7 1.7 0 00.3-1.8 1.7 1.7 0 00-1.5-1H3a2 2 0 110-4h.1a1.7 1.7 0 001.5-1.1 1.7 1.7 0 00-.3-1.8l-.1-.1a2 2 0 112.8-2.8l.1.1a1.7 1.7 0 001.8.3H9a1.7 1.7 0 001-1.5V3a2 2 0 114 0v.1a1.7 1.7 0 001 1.5 1.7 1.7 0 001.8-.3l.1-.1a2 2 0 112.8 2.8l-.1.1a1.7 1.7 0 00-.3 1.8V9a1.7 1.7 0 001.5 1H21a2 2 0 110 4h-.1a1.7 1.7 0 00-1.5 1z",
    folder: "M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2V7z",
    file: "M14 3H7a2 2 0 00-2 2v14a2 2 0 002 2h10a2 2 0 002-2V8l-5-5zM14 3v5h5",
    down: "M12 4v12m0 0l-5-5m5 5l5-5M5 20h14",
    up: "M12 20V8m0 0l-5 5m5-5l5 5M5 4h14",
    pause: "M8 5v14M16 5v14",
    play: "M7 4l13 8-13 8V4z",
    x: "M6 6l12 12M18 6L6 18",
    copy: "M9 9h10v10H9zM5 15V5h10",
    trash: "M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3",
    check: "M5 12l5 5 9-10",
    link: "M10 14a4 4 0 005.7 0l3-3a4 4 0 00-5.7-5.7l-1 1M14 10a4 4 0 00-5.7 0l-3 3a4 4 0 005.7 5.7l1-1",
    plus: "M12 5v14M5 12h14",
    back: "M15 6l-6 6 6 6",
    refresh: "M4 12a8 8 0 0114-5l2-2v6h-6l2.4-2.4A5.5 5.5 0 006.5 12M20 12a8 8 0 01-14 5l-2 2v-6h6l-2.4 2.4A5.5 5.5 0 0017.5 12",
    shield: "M12 3l8 3v6c0 5-3.5 8-8 9-4.5-1-8-4-8-9V6l8-3z",
  };
  return (
    <svg viewBox="0 0 24 24" className={className} fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d={p[name] ?? ""} />
    </svg>
  );
}

export function Badge({ children, tone = "mute" }: { children: ReactNode; tone?: "mute" | "ok" | "warn" | "accent" | "bad" }) {
  const c = {
    mute: "bg-line/70 text-mute", ok: "bg-ok/15 text-ok", warn: "bg-warn/15 text-warn", accent: "bg-accent/15 text-accent", bad: "bg-bad/15 text-bad",
  }[tone];
  return <span className={`inline-flex items-center h-5 px-2 rounded-full text-xs font-medium ${c}`}>{children}</span>;
}
