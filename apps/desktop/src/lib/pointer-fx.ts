import { useEffect } from "react";

/**
 * One window-level pointer listener drives two effects (no per-element listeners, no React re-renders):
 *  - spotlight: `.core` / `.card` under the cursor get --mx/--my/--spot, lighting their rim from the cursor
 *  - magnet: `.magnet` buttons lean toward the cursor (--tx/--ty, max ~6px), springing back on leave
 */
export function usePointerFx() {
  useEffect(() => {
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    let lit: HTMLElement | null = null;
    let mag: HTMLElement | null = null;
    let raf = 0;
    let ev: PointerEvent | null = null;

    const clearLit = () => { if (lit) { lit.style.setProperty("--spot", "0"); lit = null; } };
    const clearMag = () => { if (mag) { mag.style.removeProperty("--tx"); mag.style.removeProperty("--ty"); mag = null; } };

    const tick = () => {
      raf = 0;
      const e = ev; if (!e) return;
      const t = e.target instanceof Element ? e.target : null;
      const s = t?.closest<HTMLElement>(".core, .card") ?? null;
      if (s !== lit) { clearLit(); lit = s; }
      if (s) {
        const r = s.getBoundingClientRect();
        s.style.setProperty("--mx", `${e.clientX - r.left}px`);
        s.style.setProperty("--my", `${e.clientY - r.top}px`);
        s.style.setProperty("--spot", "1");
      }
      const m = t?.closest<HTMLElement>(".magnet:not(:disabled)") ?? null;
      if (m !== mag) { clearMag(); mag = m; }
      if (m) {
        const r = m.getBoundingClientRect();
        const dx = (e.clientX - (r.left + r.width / 2)) / (r.width / 2);
        const dy = (e.clientY - (r.top + r.height / 2)) / (r.height / 2);
        m.style.setProperty("--tx", `${(dx * 6).toFixed(2)}px`);
        m.style.setProperty("--ty", `${(dy * 4).toFixed(2)}px`);
      }
    };
    const move = (e: PointerEvent) => { ev = e; if (!raf) raf = requestAnimationFrame(tick); };
    const leave = () => { clearLit(); clearMag(); };
    window.addEventListener("pointermove", move, { passive: true });
    document.addEventListener("pointerleave", leave);
    window.addEventListener("blur", leave);
    return () => {
      window.removeEventListener("pointermove", move);
      document.removeEventListener("pointerleave", leave);
      window.removeEventListener("blur", leave);
      cancelAnimationFrame(raf);
    };
  }, []);
}
