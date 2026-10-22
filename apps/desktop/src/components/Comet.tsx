import { useEffect, useRef } from "react";
import { useReducedMotion } from "motion/react";
import { Pt, easeInOut, quad, tailPath } from "../lib/comet";
import { useStore } from "../lib/store";

const DUR = 820; // ms

/** One comet: flies from (x,y) to the "Chuyển file" tab along an arc, then pulses the tab. */
function Flight({ id, x, y }: { id: number; x: number; y: number }) {
  const land = useStore((s) => s.land);
  const tail = useRef<SVGPathElement>(null);
  const glow = useRef<SVGPathElement>(null);
  const head = useRef<SVGCircleElement>(null);
  const halo = useRef<SVGCircleElement>(null);

  useEffect(() => {
    const tab = document.querySelector<HTMLElement>('[data-tab="transfers"]');
    const r = tab?.getBoundingClientRect();
    const P0: Pt = [x, y];
    const P2: Pt = r ? [r.left + r.width / 2, r.top + r.height / 2] : [window.innerWidth / 2, window.innerHeight - 30];
    const dist = Math.hypot(P2[0] - P0[0], P2[1] - P0[1]);
    const side = P2[0] >= P0[0] ? 1 : -1;
    const P1: Pt = [(P0[0] + P2[0]) / 2 + side * dist * 0.35, Math.min(P0[1], P2[1]) - dist * 0.45];
    const t0 = performance.now();
    let raf = 0;
    const frame = (now: number) => {
      const k = Math.min(1, (now - t0) / DUR);
      const t = easeInOut(k);
      const [hx, hy] = quad(P0, P1, P2, t);
      const d = tailPath(P0, P1, P2, Math.max(0, t - 0.35), t, 8 * (1 - k * 0.45), 28);
      tail.current?.setAttribute("d", d);
      glow.current?.setAttribute("d", d);
      head.current?.setAttribute("cx", hx.toFixed(1));
      head.current?.setAttribute("cy", hy.toFixed(1));
      head.current?.setAttribute("r", (5 + 2 * Math.sin(Math.PI * k)).toFixed(2));
      halo.current?.setAttribute("cx", hx.toFixed(1));
      halo.current?.setAttribute("cy", hy.toFixed(1));
      if (k < 1) raf = requestAnimationFrame(frame);
      else land(id);
    };
    raf = requestAnimationFrame(frame);
    return () => cancelAnimationFrame(raf);
  }, [id, x, y, land]);

  return (
    <g>
      <path ref={glow} style={{ fill: "rgb(var(--accent))" }} opacity="0.45" filter="url(#comet-blur)" />
      <path ref={tail} style={{ fill: "rgb(var(--accent))" }} opacity="0.85" />
      <circle ref={halo} r="14" style={{ fill: "rgb(var(--accent))" }} opacity="0.35" filter="url(#comet-blur)" />
      <circle ref={head} r="5" fill="#fff" />
    </g>
  );
}

export function CometLayer() {
  const flights = useStore((s) => s.flights);
  const reduce = useReducedMotion();
  if (reduce || flights.length === 0) return null;
  return (
    <svg className="fixed inset-0 z-50 pointer-events-none w-full h-full overflow-visible" aria-hidden>
      <defs><filter id="comet-blur" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="5" /></filter></defs>
      {flights.map((f) => <Flight key={f.id} {...f} />)}
    </svg>
  );
}

/** Center of an element (for fly()). */
export function centerOf(el: Element | null | undefined): { x: number; y: number } {
  const r = el?.getBoundingClientRect();
  return r ? { x: r.left + r.width / 2, y: r.top + r.height / 2 } : { x: window.innerWidth / 2, y: window.innerHeight / 2 };
}
