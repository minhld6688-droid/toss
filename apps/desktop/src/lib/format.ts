export function fmtBytes(n: number): string {
  if (!isFinite(n) || n <= 0) return "0 MB";
  const GB = 1024 ** 3, MB = 1024 ** 2, KB = 1024;
  if (n >= GB) return `${(n / GB).toFixed(n >= 10 * GB ? 1 : 2)} GB`;
  if (n >= MB) return `${(n / MB).toFixed(n >= 100 * MB ? 0 : 1)} MB`;
  if (n >= KB) return `${(n / KB).toFixed(0)} KB`;
  return `${n} B`;
}

export function fmtSpeed(bps: number): string {
  return bps > 0 ? `${fmtBytes(bps)}/s` : "—";
}

export function fmtEta(s: number | null): string {
  if (s == null || !isFinite(s)) return "";
  if (s < 60) return `còn ${Math.max(1, Math.round(s))} giây`;
  if (s < 3600) return `còn ${Math.round(s / 60)} phút`;
  const h = Math.floor(s / 3600), m = Math.round((s % 3600) / 60);
  return `còn ${h} giờ ${m} phút`;
}

export function fmtExpiry(exp: number, now = Date.now() / 1000): { text: string; expired: boolean } {
  if (!exp) return { text: "Không hết hạn", expired: false };
  const left = exp - now;
  if (left <= 0) return { text: "Đã hết hạn", expired: true };
  const d = Math.floor(left / 86400);
  if (d >= 1) return { text: `Còn ${d} ngày`, expired: false };
  const h = Math.floor(left / 3600);
  if (h >= 1) return { text: `Còn ${h} giờ`, expired: false };
  return { text: `Còn ${Math.max(1, Math.round(left / 60))} phút`, expired: false };
}

export function fmtDate(unix: number): string {
  if (!unix) return "";
  const d = new Date(unix * 1000);
  const pad = (x: number) => String(x).padStart(2, "0");
  return `${pad(d.getDate())}/${pad(d.getMonth() + 1)} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** "vừa xong" / "5 phút trước" / "2 giờ trước" / "3 ngày trước"; 0 = never seen. */
export function fmtAgo(unix: number, now = Date.now() / 1000): string {
  if (!unix) return "chưa thấy online";
  const s = Math.max(0, now - unix);
  if (s < 60) return "vừa xong";
  if (s < 3600) return `${Math.round(s / 60)} phút trước`;
  if (s < 86400) return `${Math.floor(s / 3600)} giờ trước`;
  return `${Math.floor(s / 86400)} ngày trước`;
}

export const PLATFORM_LABEL: Record<string, string> = { macos: "macOS", windows: "Windows", linux: "Linux", ios: "iOS", android: "Android", web: "Trình duyệt" };

/** Bytes of a string as UTF-8 (text messages are limited to 60 KiB). */
export function utf8Bytes(s: string): number {
  return new TextEncoder().encode(s).length;
}

/** "3 file · 12 MB" */
export function fmtItems(files: number, bytes: number): string {
  return `${files} file · ${fmtBytes(bytes)}`;
}

/** Two-letter avatar text: first letters of the first two words ("Từ Đạt – Mac" → "TĐ"), or the first two letters. */
export function initials(name: string): string {
  const w = name.replace(/[–—-]/g, " ").split(/\s+/).filter(Boolean);
  if (w.length === 0) return "?";
  if (w.length === 1) return Array.from(w[0]).slice(0, 2).join("").toUpperCase();
  return (Array.from(w[0])[0] + Array.from(w[1])[0]).toUpperCase();
}

export function shortId(id: string, n = 8): string {
  return id.length <= n ? id : `${id.slice(0, n)}…`;
}

export function baseName(p: string): string {
  const parts = p.split(/[\\/]+/).filter(Boolean);
  return parts[parts.length - 1] ?? p;
}

/** "abcdefghij…uvwxyz": keep the head and the tail of a long string (paths, links, tickets). */
export function midTruncate(s: string, head = 24, tail = 10): string {
  return s.length <= head + tail + 1 ? s : `${s.slice(0, head)}…${s.slice(s.length - tail)}`;
}
