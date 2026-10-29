import { useEffect, useId, useRef } from "react";
import { useReducedMotion } from "motion/react";
import { MARK, Pt, easeToss, quad, quadDir, tailPath } from "../lib/comet";

const { P0, P1, P2, REST } = MARK;
const CYCLE = 2.9; // s

/**
 * The Toss mark: a file tossed from one machine (dot) to another (ring), comet tail tapering behind it.
 * `animate` loops the toss; otherwise (or with reduced motion) it holds the icon pose.
 */
export function TossMark({ animate = true, className = "", speed = 1 }: { animate?: boolean; className?: string; speed?: number }) {
  const reduce = useReducedMotion();
  const uid = useId().replace(/[^a-zA-Z0-9]/g, "");
  const tail = useRef<SVGPathElement>(null);
  const tailGlow = useRef<SVGPathElement>(null);
  const head = useRef<SVGGElement>(null);
  const ring = useRef<SVGCircleElement>(null);
  const ripple = useRef<SVGCircleElement>(null);
  const grad = useRef<SVGLinearGradientElement>(null);
  const speedRef = useRef(speed);
  speedRef.current = speed;

  useEffect(() => {
    const pose = (t: number, tailLen: number, w: number, s: number, landed: number) => {
      const [hx, hy] = quad(P0, P1, P2, t);
      const [dx, dy] = quadDir(P0, P1, P2, t);
      const d = tailPath(P0, P1, P2, Math.max(0.03, t - tailLen), t, w);
      tail.current?.setAttribute("d", d);
      tailGlow.current?.setAttribute("d", d);
      grad.current?.setAttribute("x2", hx.toFixed(1));
      grad.current?.setAttribute("y2", hy.toFixed(1));
      const tt = Math.max(0.03, t - tailLen);
      const [sx, sy] = quad(P0, P1, P2, tt);
      grad.current?.setAttribute("x1", sx.toFixed(1));
      grad.current?.setAttribute("y1", sy.toFixed(1));
      const ang = (Math.atan2(dy, dx) * 180) / Math.PI;
      head.current?.setAttribute("transform", `translate(${hx.toFixed(1)} ${hy.toFixed(1)}) rotate(${ang.toFixed(1)}) scale(${s.toFixed(3)})`);
      // ring reacts on landing
      const r = 30 + 10 * landed;
      ring.current?.setAttribute("r", r.toFixed(1));
      if (ripple.current) {
        ripple.current.setAttribute("r", (34 + 90 * landed).toFixed(1));
        ripple.current.setAttribute("opacity", landed > 0 ? (0.6 * (1 - landed)).toFixed(3) : "0");
      }
    };

    if (reduce || !animate) { pose(REST, REST, 44, 1, 0); return; }

    let raf = 0, last = performance.now(), clock = 0.75; // start mid-flight so the first frame isn't empty
    const frame = (now: number) => {
      const dt = Math.min(0.05, (now - last) / 1000); last = now;
      clock = (clock + dt * speedRef.current) % CYCLE;
      const c = clock;
      if (c < 0.35) {
        // spawn at the dot
        const k = c / 0.35;
        pose(0.03, 0, 0, k * k * (3 - 2 * k) * 0.8, 0);
      } else if (c < 1.95) {
        const k = easeToss((c - 0.35) / 1.6);
        const t = 0.03 + 0.97 * k;
        const near = Math.max(0, (t - 0.86) / 0.14); // shrink into the ring
        const w = 44 * Math.min(1, k * 2.4) * (1 - near * 0.8);
        pose(t, 0.42 * Math.min(1, k * 2.2) * (1 - near * 0.6), w, 0.8 + 0.2 * Math.sin(Math.PI * Math.min(1, k * 1.2)) - near * 0.75, 0);
      } else if (c < 2.6) {
        const k = (c - 1.95) / 0.65;
        pose(1, 0, 0, 0, 1 - Math.pow(1 - k, 3));
      } else {
        pose(1, 0, 0, 0, 0);
      }
      raf = requestAnimationFrame(frame);
    };
    const vis = () => {
      cancelAnimationFrame(raf);
      if (!document.hidden) { last = performance.now(); raf = requestAnimationFrame(frame); }
    };
    raf = requestAnimationFrame(frame);
    document.addEventListener("visibilitychange", vis);
    return () => { cancelAnimationFrame(raf); document.removeEventListener("visibilitychange", vis); };
  }, [animate, reduce]);

  const dots: Pt[] = [];
  for (let i = 1; i <= 7; i++) dots.push(quad(P0, P1, P2, REST + (1 - REST) * (i / 7.6)));

  return (
    <svg viewBox="160 230 704 560" className={className} aria-hidden fill="none">
      <defs>
        <linearGradient id={`t${uid}`} ref={grad} gradientUnits="userSpaceOnUse" x1={P0[0]} y1={P0[1]} x2="584" y2="430">
          <stop offset="0" style={{ stopColor: "rgb(var(--mark-tail))", stopOpacity: 0 }} />
          <stop offset="0.45" style={{ stopColor: "rgb(var(--mark-tail))", stopOpacity: 0.5 }} />
          <stop offset="1" style={{ stopColor: "rgb(var(--mark-head-2))", stopOpacity: 1 }} />
        </linearGradient>
        <linearGradient id={`h${uid}`} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" style={{ stopColor: "rgb(var(--mark-head-1))" }} />
          <stop offset="1" style={{ stopColor: "rgb(var(--mark-head-2))" }} />
        </linearGradient>
        <filter id={`g${uid}`} x="-60%" y="-60%" width="220%" height="220%"><feGaussianBlur stdDeviation="22" /></filter>
      </defs>
      {/* where it's going */}
      {dots.map(([x, y], i) => (
        <circle key={i} cx={x} cy={y} r={11 - (i + 1) * 0.6} style={{ fill: "rgb(var(--mark-tail))" }} opacity={0.55 - i * 0.05} />
      ))}
      <circle ref={ripple} cx={P2[0]} cy={P2[1]} r="34" strokeWidth="6" style={{ stroke: "rgb(var(--mark-tail))" }} opacity="0" />
      <circle ref={ring} cx={P2[0]} cy={P2[1]} r="30" strokeWidth="12" style={{ stroke: "rgb(var(--mark-ring))" }} />
      <circle cx={P0[0]} cy={P0[1]} r="24" style={{ fill: "rgb(var(--mark-tail))" }} />
      <path ref={tailGlow} style={{ fill: "rgb(var(--mark-tail))" }} opacity="0.4" filter={`url(#g${uid})`} />
      <path ref={tail} fill={`url(#t${uid})`} />
      <g ref={head}>
        <rect x="-90" y="-90" width="180" height="180" rx="50" style={{ fill: "rgb(var(--mark-tail))" }} opacity="0.55" filter={`url(#g${uid})`} />
        <rect x="-72" y="-72" width="144" height="144" rx="42" fill={`url(#h${uid})`} />
        <rect x="-60" y="-66" width="120" height="40" rx="20" fill="#fff" opacity="0.5" />
      </g>
    </svg>
  );
}
