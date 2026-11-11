// 0.9 studio pipeline screens (Links tab, Máy studio settings, "Lưu vào" in the link sheet, backup receipt):
//   node scripts/screenshots-v09.mjs [outDir]
// Runs the mock UI (VITE_MOCK=1) in Chromium at the 400x600 window size, light and dark.
import { createServer } from "vite";
import { chromium } from "playwright";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

process.env.VITE_MOCK = "1";
const out = process.argv[2] || new URL("../screenshots/", import.meta.url).pathname;
mkdirSync(out, { recursive: true });
const server = await createServer({ server: { port: 1424, strictPort: false, host: "127.0.0.1" }, logLevel: "error" });
await server.listen();
const base = server.resolvedUrls.local[0];
const browser = await chromium.launch({ args: ["--no-sandbox"] });
const errors = [];
async function shot(page, name, wait = 900) { await page.waitForTimeout(wait); await page.screenshot({ path: join(out, name) }); console.log("wrote", name); }
const Q = "quiet=1&fw=ok";

for (const scheme of ["light", "dark"]) {
  const ctx = await browser.newContext({ viewport: { width: 400, height: 600 }, deviceScaleFactor: 2, colorScheme: scheme });
  const sfx = scheme === "dark" ? "-dark" : "";
  const p = await ctx.newPage();
  p.on("pageerror", (e) => errors.push(String(e)));
  p.on("console", (m) => { if (m.type() === "error") errors.push(m.text()); });
  const open = async (qs, ready) => { await p.goto(`${base}?${qs}`); await p.getByText(ready).first().waitFor(); };

  // Links: jobs with their shooters, live download, backup links
  await open(`${Q}&studio=1`, "Kéo vào đây để gửi");
  await p.getByRole("button", { name: "Links" }).click();
  await p.getByText("Chép cả job").first().waitFor();
  await shot(p, `40-links${sfx}.png`, 1200);
  await p.getByText("Backup về studio").first().evaluate((el) => el.scrollIntoView({ block: "start" }));
  await shot(p, `41-links-backup${sfx}.png`);
  // copy a whole job: the toast confirms
  await p.getByRole("button", { name: /Chép cả job/ }).first().evaluate((el) => el.scrollIntoView({ block: "center" }));
  await p.getByRole("button", { name: /Chép cả job/ }).first().click();
  await shot(p, `42-links-copied${sfx}.png`, 500);

  // the jobs drive is unplugged
  await open(`${Q}&studio=1&studioerr=${encodeURIComponent("Không thấy D:\\Jobs (ổ chưa cắm?)")}`, "Kéo vào đây để gửi");
  await p.getByRole("button", { name: "Links" }).click();
  await p.getByText("ổ chưa cắm").first().waitFor();
  await shot(p, `43-links-error${sfx}.png`);

  // Settings: Máy studio
  await open(`${Q}&studio=1`, "Kéo vào đây để gửi");
  await p.getByRole("button", { name: "Cài đặt" }).click();
  await p.getByText("Chế độ máy studio").first().evaluate((el) => el.scrollIntoView({ block: "center" }));
  await shot(p, `44-settings-studio${sfx}.png`);

  // An editor clicks a link: the sheet asks where to save, last folder first
  await open(`${Q}&deeplink=1&auto=0&lastdir=${encodeURIComponent("/Volumes/SSD Editor/Jobs/11-2026")}`, "Lưu vào");
  await shot(p, `45-sheet-dest${sfx}.png`);

  // A confirmed backup can be freed (two steps)
  await open(Q, "Kéo vào đây để gửi");
  await p.getByRole("button", { name: "Chuyển file" }).click();
  await p.getByText("Backup · Ngọc Anh").first().evaluate((el) => el.scrollIntoView({ block: "start" }));
  await shot(p, `46-receipt${sfx}.png`);
  await p.getByRole("button", { name: "Giải phóng trên máy này" }).click();
  await shot(p, `47-receipt-confirm${sfx}.png`, 500);
  await p.getByRole("button", { name: "Xoá khỏi máy này" }).click();
  await p.getByText("Đã giải phóng").first().waitFor();
  await shot(p, `48-receipt-freed${sfx}.png`, 600);
  await ctx.close();
}

await browser.close();
await server.close();
if (errors.length) { console.error("page errors:\n" + errors.join("\n")); process.exit(1); }
