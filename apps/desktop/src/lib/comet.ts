/**
 * Geometry for the Toss mark: a file tossed along a parabola, with a tapered comet tail.
 * Same curve as the app icon (apps/desktop/app-icon.svg, 1024 grid).
 */
export type Pt = [number, number];

export const MARK = { P0: [250, 700] as Pt, P1: [512, 40] as Pt, P2: [774, 700] as Pt, REST: 0.57 };

export function quad(P0: Pt, P1: Pt, P2: Pt, t: number): Pt {
  const a = (1 - t) * (1 - t), b = 2 * (1 - t) * t, c = t * t;
  return [a * P0[0] + b * P1[0] + c * P2[0], a * P0[1] + b * P1[1] + c * P2[1]];
}

export function quadDir(P0: Pt, P1: Pt, P2: Pt, t: number): Pt {
  const x = 2 * (1 - t) * (P1[0] - P0[0]) + 2 * t * (P2[0] - P1[0]);
  const y = 2 * (1 - t) * (P1[1] - P0[1]) + 2 * t * (P2[1] - P1[1]);
  const l = Math.hypot(x, y) || 1;
  return [x / l, y / l];
}

/**
 * Closed SVG path of a tail that ends at curve(t1) and starts at curve(t0); width tapers from ~0 to `w`.
 */
export function tailPath(P0: Pt, P1: Pt, P2: Pt, t0: number, t1: number, w: number, n = 36): string {
  if (t1 - t0 < 0.004) return "";
  const L: string[] = [], R: string[] = [];
  for (let i = 0; i <= n; i++) {
    const u = i / n;
    const t = t0 + (t1 - t0) * u;
    const [x, y] = quad(P0, P1, P2, t);
    const [dx, dy] = quadDir(P0, P1, P2, t);
    const k = 0.5 + w * Math.pow(u, 1.6);
    L.push(`${(x - dy * k).toFixed(1)} ${(y + dx * k).toFixed(1)}`);
    R.push(`${(x + dy * k).toFixed(1)} ${(y - dx * k).toFixed(1)}`);
  }
  return `M${L.join(" L")} L${R.reverse().join(" L")} Z`;
}

export const easeInOut = (t: number) => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);
export const easeOutBack = (t: number) => { const c1 = 1.70158, c3 = c1 + 1; return 1 + c3 * Math.pow(t - 1, 3) + c1 * Math.pow(t - 1, 2); };
/** slow out of the hand, hang at the apex, fall faster: a thrown object */
export const easeToss = (t: number) => {
  const a = t * t * (3 - 2 * t); // smoothstep
  return a * 0.7 + t * 0.3;
};
