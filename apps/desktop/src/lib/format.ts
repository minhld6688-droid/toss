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

export function shortId(id: string, n = 8): string {
  return id.length <= n ? id : `${id.slice(0, n)}…`;
}

export function baseName(p: string): string {
  const parts = p.split(/[\\/]+/).filter(Boolean);
  return parts[parts.length - 1] ?? p;
}
