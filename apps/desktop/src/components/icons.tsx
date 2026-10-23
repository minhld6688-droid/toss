/**
 * Toss icon set — drawn for this app, same language as the app icon.
 *
 * Grammar (keep it when adding icons):
 *  - 24 grid, 2px safe margin, round caps + joins, corners r≈1.5–2.
 *  - Motion is drawn as an ARC, never a straight dash: arrows that "go" follow the toss trajectory.
 *  - One "spark" per icon at most: a small solid dot (r 1.4–1.6) where the energy is (a knob, a door, a landing).
 *  - Optional "body" layer (shape fill) only shows in duotone / fill weights.
 */
type Part = { d?: string; c?: [number, number, number]; r?: [number, number, number, number, number]; k: "s" | "f" | "dot"; tr?: string };

const S = (d: string): Part => ({ d, k: "s" });
const F = (d: string): Part => ({ d, k: "f" });
const DOT = (x: number, y: number, r = 1.5): Part => ({ c: [x, y, r], k: "dot" });
const RING = (x: number, y: number, r: number): Part => ({ c: [x, y, r], k: "s" });
const RECT = (x: number, y: number, w: number, h: number, rx: number, k: "s" | "f" | "dot" = "s", tr?: string): Part => ({ r: [x, y, w, h, rx], k, tr });

const HOUSE = "M4 10.6 11.1 4.7a1.4 1.4 0 0 1 1.8 0L20 10.6V18.5A1.5 1.5 0 0 1 18.5 20h-13A1.5 1.5 0 0 1 4 18.5Z";
const FOLDER_OPEN = "M5.1 11.6A1.5 1.5 0 0 1 6.5 10.5h13.4a1 1 0 0 1 .96 1.28l-1.8 6.1A1.5 1.5 0 0 1 17.6 19H5a1.5 1.5 0 0 1-1.46-1.86Z";
const PAGE = "M6 4.5A1.5 1.5 0 0 1 7.5 3H13l5 5v10.5a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 6 18.5Z";
const SHIELD = "M12 3.5 19 6v5.4c0 4.3-2.9 7.6-7 9.1-4.1-1.5-7-4.8-7-9.1V6Z";
const BIN = "M6.5 8h11l-.8 10.6A1.5 1.5 0 0 1 15.2 20H8.8a1.5 1.5 0 0 1-1.5-1.4Z";

export const TOSS_ICONS = {
  // the brand verb: dot (máy tổng) → file mid-air → ring (editor)
  transfer: [S("M5 16.4C6.2 11.2 8.6 8.6 11.6 8"), RECT(12, 5.2, 5.4, 5.4, 1.6, "dot", "rotate(24 14.7 7.9)"), DOT(18.6, 12.2, 0.95), DOT(19.5, 15.1, 0.95), DOT(4.6, 19.1, 1.7), RING(19.8, 19, 1.7)],
  home: [F(HOUSE), S(HOUSE), S("M10 20v-3.6a2 2 0 0 1 4 0V20"), DOT(12, 11.3, 1.4)],
  // sliders, not a gear: one knob is the spark
  settings: [S("M4 8h8.5"), S("M17.5 8H20"), RING(15, 8, 2.3), S("M4 16h2.5"), S("M11.5 16H20"), DOT(9, 16, 2.3)],
  // open folder: the front flap tilts forward, ready to receive
  folder: [F(FOLDER_OPEN), S("M3.5 17.5V7A1.5 1.5 0 0 1 5 5.5h3.6a1.5 1.5 0 0 1 1.06.44L11.5 7.8H17a1.5 1.5 0 0 1 1.5 1.5v1.2"), S(FOLDER_OPEN)],
  file: [F(PAGE), S(PAGE), S("M13 3v3.5A1.5 1.5 0 0 0 14.5 8H18"), S("M9 13h6"), S("M9 16.2h3.6")],
  // into / out of the tray: the tray is a soft arc, the arrow lands on it
  down: [S("M12 3.8v10.4"), S("M7.8 10.4 12 14.6l4.2-4.2"), S("M4.8 17.6c4.6 2.9 9.8 2.9 14.4 0")],
  up: [S("M12 14.6V4.2"), S("M7.8 8.4 12 4.2l4.2 4.2"), S("M4.8 17.6c4.6 2.9 9.8 2.9 14.4 0")],
  pause: [RECT(6.5, 5, 3.8, 14, 1.6, "f"), RECT(6.5, 5, 3.8, 14, 1.6), RECT(13.7, 5, 3.8, 14, 1.6, "f"), RECT(13.7, 5, 3.8, 14, 1.6)],
  play: [F("M8 5.6v12.8a1.2 1.2 0 0 0 1.8 1l10-6.4a1.2 1.2 0 0 0 0-2l-10-6.4A1.2 1.2 0 0 0 8 5.6Z"), S("M8 5.6v12.8a1.2 1.2 0 0 0 1.8 1l10-6.4a1.2 1.2 0 0 0 0-2l-10-6.4A1.2 1.2 0 0 0 8 5.6Z")],
  x: [S("M6.5 6.5l11 11"), S("M17.5 6.5l-11 11")],
  // the front sheet is whole, the back one only its corner
  copy: [S("M8.5 7.5V5.6A1.6 1.6 0 0 1 10.1 4h8.3A1.6 1.6 0 0 1 20 5.6v8.3a1.6 1.6 0 0 1-1.6 1.6h-1.9"), RECT(4, 8.5, 12, 12, 2, "f"), RECT(4, 8.5, 12, 12, 2)],
  trash: [F(BIN), S(BIN), S("M4.5 8h15"), S("M9.5 8V5.5A1.5 1.5 0 0 1 11 4h2a1.5 1.5 0 0 1 1.5 1.5V8"), S("M10.4 11.5v5"), S("M13.6 11.5v5")],
  check: [S("M4.8 12.8 9.6 17.4 19.4 6.8")],
  // two orbits that just touch, the spark where they meet
  link: [S("M10.2 13.8a3.9 3.9 0 0 0 5.5 0l2.9-2.9a3.9 3.9 0 0 0-5.5-5.5l-1 1"), S("M13.8 10.2a3.9 3.9 0 0 0-5.5 0l-2.9 2.9a3.9 3.9 0 0 0 5.5 5.5l1-1"), DOT(12, 12, 1.3)],
  plus: [S("M12 5.5v13"), S("M5.5 12h13")],
  back: [S("M19.5 17C18.6 12 14.6 9.2 6 9.4"), S("M9.6 5.4 5.6 9.4l4 4")],
  refresh: [S("M19.6 12.6a7.6 7.6 0 1 1-2.5-6.4"), S("M19.4 4v4.4H15")],
  shield: [F(SHIELD), S(SHIELD), S("M9 12.2 11.2 14.4 15.2 10")],
  // "go" is a toss: rises in an arc and lands up-right
  go: [S("M5.5 18.5C6.3 11.6 10 7.6 17.5 7"), S("M13.8 3.9 17.6 7l-3.3 3.6")],
  // out of the box, on an arc
  // disclosure caret: a soft arc, not a corner
  chevron: [S("M6.2 9.6c2 2.5 3.7 4 5.8 5.4 2.1-1.4 3.8-2.9 5.8-5.4")],
  // the link between two machines: two arcs and the spark where they meet
  signal: [S("M4.8 9.4a10 10 0 0 1 14.4 0"), S("M7.9 12.6a5.7 5.7 0 0 1 8.2 0"), DOT(12, 16.6, 1.6)],
  // a note: ring, stem, spark on top
  info: [RING(12, 12, 8.4), S("M12 11v5.2"), DOT(12, 7.9, 1.1)],
  share: [S("M8.2 11H6.6A1.6 1.6 0 0 0 5 12.6v5.8A1.6 1.6 0 0 0 6.6 20h10.8a1.6 1.6 0 0 0 1.6-1.6v-1.3"), S("M10 15.2C10.4 9.8 13.4 6.6 19 6"), S("M15.6 3 19.1 6l-3 3.5"), DOT(10, 15.4, 1.4)],
  // v0.5 "Danh bạ máy" ------------------------------------------------------------------
  // a machine's owner: head ring, shoulders as one soft arc, the spark is the presence light
  contact: [F("M5.2 19.4c.8-3.7 3.4-5.7 6.8-5.7s6 2 6.8 5.7Z"), RING(12, 8.3, 3.5), S("M5.2 19.4c.8-3.7 3.4-5.7 6.8-5.7s6 2 6.8 5.7"), DOT(18.4, 5.2, 1.45)],
  // the same figure, half-turned, with a plus: someone you are about to add
  invite: [RING(9.8, 8.4, 3.3), S("M3.6 19.4c.7-3.4 3-5.3 6.2-5.3 1.4 0 2.6.3 3.6 1"), S("M18 12.3v6.2"), S("M14.9 15.4h6.2")],
  // laptop: lid with a spark for the camera, base is a soft arc
  laptop: [RECT(5.2, 5, 13.6, 9.4, 1.9, "f"), RECT(5.2, 5, 13.6, 9.4, 1.9), S("M2.8 18.4c3 1.3 15.4 1.3 18.4 0"), DOT(12, 9.7, 1.3)],
  // desktop: screen on a short neck
  desktop: [RECT(3.6, 4.4, 16.8, 11.2, 2, "f"), RECT(3.6, 4.4, 16.8, 11.2, 2), S("M12 15.6v3.6"), S("M8.6 19.6h6.8"), DOT(12, 10, 1.3)],
  // the link is cut: the signal arcs with a slash through them
  offline: [S("M4.8 9.4a10 10 0 0 1 3.4-2.3"), S("M19.2 9.4a10 10 0 0 0-5.4-3"), S("M7.9 12.6a5.7 5.7 0 0 1 2.2-1.4"), S("M16.1 12.6a5.7 5.7 0 0 0-1.6-1.2"), DOT(12, 16.6, 1.6), S("M4.6 4.6l14.8 14.8")],
  // waiting: a ring and one hand, the spark where the hand points
  clock: [RING(12, 12, 8.4), S("M12 7.4V12l3.1 1.9"), DOT(12, 12, 1.1)],
  // qr: three finders and the data spark
  qr: [RECT(4, 4, 6.4, 6.4, 1.6), RECT(13.6, 4, 6.4, 6.4, 1.6), RECT(4, 13.6, 6.4, 6.4, 1.6), DOT(16.8, 16.8, 1.7), S("M13.8 20h6.2"), S("M20 13.8v.4")],
} satisfies Record<string, Part[]>;

export type TossIconName = keyof typeof TOSS_ICONS;
export type IconWeight = "light" | "regular" | "bold" | "fill" | "duotone";

const STROKE: Record<IconWeight, number> = { light: 1.35, regular: 1.7, bold: 2.15, duotone: 1.7, fill: 2 };
const BODY: Record<IconWeight, number> = { light: 0, regular: 0, bold: 0, duotone: 0.2, fill: 0.34 };

export function TossIcon({ name, className = "w-4 h-4", weight = "regular" }: { name: TossIconName; className?: string; weight?: IconWeight }) {
  const parts = TOSS_ICONS[name];
  const sw = STROKE[weight];
  const body = BODY[weight];
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden fill="none" stroke="currentColor" strokeWidth={sw} strokeLinecap="round" strokeLinejoin="round">
      {parts.map((p, i) => {
        if (p.k === "f" && body === 0) return null;
        const fillProps = p.k === "f" ? { fill: "currentColor", stroke: "none", opacity: body } : p.k === "dot" ? { fill: "currentColor", stroke: "none" } : {};
        if (p.d) return <path key={i} d={p.d} {...fillProps} />;
        if (p.c) return <circle key={i} cx={p.c[0]} cy={p.c[1]} r={p.k === "dot" ? p.c[2] + (sw - 1.7) * 0.3 : p.c[2]} {...fillProps} />;
        if (p.r) return <rect key={i} x={p.r[0]} y={p.r[1]} width={p.r[2]} height={p.r[3]} rx={p.r[4]} transform={p.tr} {...fillProps} />;
        return null;
      })}
    </svg>
  );
}
