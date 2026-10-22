import { ReactNode, useEffect } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import {
  ArrowDown, ArrowLeft, ArrowsDownUp, ArrowUp, ArrowUpRight, ArrowsClockwise, Check, Copy, Folder, FileText, GearSix, House, Link as LinkIcon,
  Pause, Play, Plus, ShareNetwork, ShieldCheck, Trash, X,
} from "@phosphor-icons/react";

const EASE = [0.32, 0.72, 0, 1] as const;

export function Toggle({ on, onChange, disabled, label }: { on: boolean; onChange: (v: boolean) => void; disabled?: boolean; label?: string }) {
  return (
    <button
      role="switch" aria-checked={on} aria-label={label} disabled={disabled}
      onClick={() => onChange(!on)}
      className={`relative w-11 h-6 rounded-full shrink-0 transition-colors duration-500 ease-aura ${on ? "bg-accent" : "bg-line/15"} ${disabled ? "opacity-40" : ""}`}
      style={on ? { boxShadow: "inset 0 1px 0 rgb(255 255 255 / 0.3), 0 8px 24px -12px rgb(var(--accent) / 0.8)" } : undefined}
    >
      <span className={`absolute top-0.5 left-0.5 w-5 h-5 rounded-full bg-white shadow-[0_1px_3px_rgba(0,0,0,0.35)] transition-transform duration-500 ease-aura ${on ? "translate-x-5" : ""}`} />
    </button>
  );
}

export function Row({ title, hint, children }: { title: string; hint?: string; children?: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-3 min-h-[44px] py-1">
      <div className="min-w-0">
        <div className="text-base">{title}</div>
        {hint && <div className="text-xs text-mute">{hint}</div>}
      </div>
      {children}
    </div>
  );
}

/** Double-bezel section: outer shell tray + inner core plate. */
export function Section({ title, children, right }: { title: string; children: ReactNode; right?: ReactNode }) {
  return (
    <section className="mb-4 rise">
      <div className="flex items-center justify-between">
        <h2 className="h-section">{title}</h2>
        {right}
      </div>
      <div className="shell"><div className="core p-3.5">{children}</div></div>
    </section>
  );
}

export function Progress({ value, tone = "accent", live = false }: { value: number; tone?: "accent" | "ok" | "bad" | "mute"; live?: boolean }) {
  const color = { accent: "bg-accent", ok: "bg-ok", bad: "bg-bad", mute: "bg-mute" }[tone];
  const glow = { accent: "rgb(var(--accent) / 0.8)", ok: "rgb(var(--ok) / 0.7)", bad: "rgb(var(--bad) / 0.6)", mute: "transparent" }[tone];
  const pct = Math.max(0, Math.min(1, value)) * 100;
  return (
    <div className="h-1 rounded-full bg-line/10 overflow-visible" role="progressbar" aria-valuenow={Math.round(pct)} aria-valuemin={0} aria-valuemax={100}>
      <div
        className={`relative h-full rounded-full ${color} ${live ? "sheen" : ""} transition-[width] duration-700 ease-aura`}
        style={{ width: `${pct}%`, boxShadow: live ? `0 0 12px 0 ${glow}` : undefined }}
      />
    </div>
  );
}

export function Sheet({ children, onClose, title }: { children: ReactNode; onClose: () => void; title?: string }) {
  const reduce = useReducedMotion();
  useEffect(() => {
    const h = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [onClose]);
  return (
    <motion.div
      className="fixed inset-0 z-30 flex flex-col justify-end bg-black/50"
      initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: 0.3 }}
      onMouseDown={(e) => e.target === e.currentTarget && onClose()}
    >
      <motion.div
        className="glass rounded-t-[24px] rounded-b-none p-4 pt-3 max-h-[92%] overflow-y-auto"
        role="dialog" aria-label={title}
        initial={reduce ? false : { y: 48, opacity: 0, filter: "blur(6px)" }}
        animate={{ y: 0, opacity: 1, filter: "blur(0px)" }}
        exit={reduce ? undefined : { y: 32, opacity: 0, filter: "blur(4px)" }}
        transition={{ duration: 0.6, ease: EASE }}
      >
        <div className="mx-auto w-9 h-1 rounded-full bg-line/20 mb-3" />
        {children}
      </motion.div>
    </motion.div>
  );
}

/** Full-height sheet: covers the whole window (incl. the tab bar), same aura backdrop and motion as Sheet. */
export function FullSheet({ children, onClose, title }: { children: ReactNode; onClose: () => void; title?: string }) {
  const reduce = useReducedMotion();
  useEffect(() => {
    const h = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [onClose]);
  return (
    <motion.div
      className="fixed inset-0 z-40 flex flex-col bg-bg"
      role="dialog" aria-label={title}
      initial={reduce ? false : { y: 40, opacity: 0, filter: "blur(6px)" }}
      animate={{ y: 0, opacity: 1, filter: "blur(0px)" }}
      exit={reduce ? undefined : { y: 24, opacity: 0, filter: "blur(4px)" }}
      transition={{ duration: 0.6, ease: EASE }}
    >
      <div className="aura" /><div className="grain" />
      {children}
    </motion.div>
  );
}

export { AnimatePresence };

const ICONS = {
  home: House, transfer: ArrowsDownUp, settings: GearSix, folder: Folder, file: FileText, down: ArrowDown, up: ArrowUp, pause: Pause, play: Play,
  x: X, copy: Copy, trash: Trash, check: Check, link: LinkIcon, plus: Plus, back: ArrowLeft, refresh: ArrowsClockwise, shield: ShieldCheck, go: ArrowUpRight, share: ShareNetwork,
} as const;
export type IconName = keyof typeof ICONS;

export function Icon({ name, className = "w-4 h-4", weight = "regular" }: { name: IconName | string; className?: string; weight?: "light" | "regular" | "bold" }) {
  const C = ICONS[name as IconName] ?? LinkIcon;
  return <C className={className} weight={weight} aria-hidden />;
}

/** Trailing icon nested in its own circle (button-in-button). */
export function Nest({ name = "go" }: { name?: IconName }) {
  return <span className="nest"><Icon name={name} className="w-3.5 h-3.5" weight="bold" /></span>;
}

export function Badge({ children, tone = "mute" }: { children: ReactNode; tone?: "mute" | "ok" | "warn" | "accent" | "bad" }) {
  const c = {
    mute: "bg-line/10 text-mute", ok: "bg-ok/15 text-ok", warn: "bg-warn/15 text-warn", accent: "bg-accent/15 text-accent", bad: "bg-bad/15 text-bad",
  }[tone];
  return <span className={`inline-flex items-center h-5 px-2 rounded-full text-[11px] font-semibold tracking-wide ${c}`}>{children}</span>;
}
