import { ReactNode, useEffect } from "react";
import { AnimatePresence, motion, useDragControls, useReducedMotion } from "motion/react";
import { IconWeight, TOSS_ICONS, TossIcon, TossIconName } from "./icons";

const EASE = [0.32, 0.72, 0, 1] as const;

export function Toggle({ on, onChange, disabled, label }: { on: boolean; onChange: (v: boolean) => void; disabled?: boolean; label?: string }) {
  return (
    <button
      role="switch" aria-checked={on} aria-label={label} disabled={disabled}
      onClick={() => onChange(!on)}
      className={`group relative w-11 h-6 rounded-full shrink-0 transition-colors duration-500 ease-aura ${on ? "bg-accent" : "bg-line/15"} ${disabled ? "opacity-40" : ""}`}
      style={on ? { boxShadow: "inset 0 1px 0 rgb(255 255 255 / 0.3), 0 8px 24px -12px rgb(var(--accent) / 0.8)" } : undefined}
    >
      <span
        className={`absolute top-0.5 left-0.5 h-5 w-5 rounded-full bg-white shadow-[0_1px_3px_rgba(0,0,0,0.35)] transition-[transform,width] duration-500 group-active:w-6 ${on ? "translate-x-5 group-active:translate-x-4" : ""}`}
        style={{ transitionTimingFunction: "var(--spring)" }}
      />
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
        className={`relative h-full rounded-full ${live && tone === "accent" ? "flow" : color} transition-[width] duration-700 ease-aura`}
        style={{ width: `${pct}%`, boxShadow: live ? `0 0 14px 0 ${glow}` : undefined }}
      >{live && pct > 2 && <span className="spark" />}</div>
    </div>
  );
}

export function Sheet({ children, onClose, title }: { children: ReactNode; onClose: () => void; title?: string }) {
  const reduce = useReducedMotion();
  const drag = useDragControls();
  useEffect(() => {
    const h = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [onClose]);
  return (
    <motion.div
      className="fixed inset-0 z-30 flex flex-col justify-end"
      initial={{ backgroundColor: "rgba(0,0,0,0)", backdropFilter: "blur(0px)" }}
      animate={{ backgroundColor: "rgba(0,0,0,0.5)", backdropFilter: "blur(3px)" }}
      exit={{ backgroundColor: "rgba(0,0,0,0)", backdropFilter: "blur(0px)" }}
      transition={{ duration: 0.4, ease: EASE }}
      onMouseDown={(e) => e.target === e.currentTarget && onClose()}
    >
      <motion.div
        className="glass rounded-t-[28px] rounded-b-none p-4 pt-2 max-h-[92%] overflow-y-auto origin-bottom"
        role="dialog" aria-label={title}
        initial={reduce ? false : { y: "100%", scale: 0.96, opacity: 0.4 }}
        animate={{ y: 0, scale: 1, opacity: 1 }}
        exit={reduce ? undefined : { y: "100%", scale: 0.98, opacity: 0.6, transition: { duration: 0.38, ease: EASE } }}
        transition={{ type: "spring", stiffness: 380, damping: 34, mass: 0.9 }}
        drag={reduce ? false : "y"} dragListener={false} dragControls={drag}
        dragConstraints={{ top: 0, bottom: 0 }} dragElastic={{ top: 0.05, bottom: 0.7 }}
        onDragEnd={(_, i) => { if (i.offset.y > 90 || i.velocity.y > 600) onClose(); }}
      >
        <div className="py-2 -mx-4 mb-1.5 cursor-grab active:cursor-grabbing touch-none" onPointerDown={(e) => drag.start(e)}>
          <div className="mx-auto w-9 h-1 rounded-full bg-line/20" />
        </div>
        <Stagger>{children}</Stagger>
      </motion.div>
    </motion.div>
  );
}

/** Children cascade in one after another (direct children only). */
export function Stagger({ children, delay = 0.08, step = 0.055 }: { children: ReactNode; delay?: number; step?: number }) {
  const reduce = useReducedMotion();
  const list = Array.isArray(children) ? children.flat() : [children];
  if (reduce) return <>{children}</>;
  return (
    <>
      {list.filter((c) => c !== null && c !== undefined && c !== false).map((c, i) => (
        <motion.div
          key={i}
          initial={{ opacity: 0, y: 14, filter: "blur(5px)" }}
          animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
          transition={{ duration: 0.7, ease: EASE, delay: delay + i * step }}
        >{c}</motion.div>
      ))}
    </>
  );
}

/** Headline whose words rise in one by one. `accent` words render as the italic display accent. */
export function Words({ text, accent = [], className = "", as: Tag = "h1", delay = 0 }: { text: string; accent?: string[]; className?: string; as?: "h1" | "h2" | "div"; delay?: number }) {
  const reduce = useReducedMotion();
  const words = text.split(" ");
  return (
    <Tag className={`title ${className}`} aria-label={text}>
      {words.map((w, i) => {
        const inner = accent.includes(w) ? <em>{w}</em> : w;
        return (
          <motion.span
            key={`${w}-${i}`} aria-hidden className="inline-block whitespace-pre"
            initial={reduce ? false : { opacity: 0, y: "0.45em", rotate: 4, filter: "blur(6px)" }}
            animate={{ opacity: 1, y: 0, rotate: 0, filter: "blur(0px)" }}
            transition={{ duration: 0.8, ease: EASE, delay: delay + i * 0.06 }}
          >{inner}{i < words.length - 1 ? " " : ""}</motion.span>
        );
      })}
    </Tag>
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
      initial={reduce ? false : { clipPath: "inset(100% 0% 0% 0% round 28px)", y: 30 }}
      animate={{ clipPath: "inset(0% 0% 0% 0% round 0px)", y: 0 }}
      exit={reduce ? undefined : { clipPath: "inset(100% 0% 0% 0% round 28px)", y: 30, transition: { duration: 0.45, ease: EASE } }}
      transition={{ duration: 0.75, ease: EASE }}
    >
      <div className="aura" /><div className="grain" />
      {children}
    </motion.div>
  );
}

export { AnimatePresence, motion, EASE };

export type IconName = TossIconName;

export function Icon({ name, className = "w-4 h-4", weight = "regular" }: { name: IconName | string; className?: string; weight?: IconWeight }) {
  const n = (name in TOSS_ICONS ? name : "link") as TossIconName;
  return <TossIcon name={n} className={className} weight={weight} />;
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
