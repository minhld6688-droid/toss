// v0.8 screens (nearby, incoming, text, one-screen wizard, 3-section settings): node scripts/screenshots-v08.mjs
// Runs the mock UI (VITE_MOCK=1) in Chromium at the 400x600 window size, light and dark -> screenshots/30-*.png …
import { createServer } from "vite";
import { chromium } from "playwright";
import { mkdirSync, existsSync, readdirSync } from "node:fs";
import { join } from "node:path";

process.env.VITE_MOCK = "1";
const out = new URL("../screenshots/", import.meta.url).pathname;
mkdirSync(out, { recursive: true });
const server = await createServer({ server: { port: 1423, strictPort: false, host: "127.0.0.1" }, logLevel: "error" });
await server.listen();
const base = server.resolvedUrls.local[0];
let executablePath;
const root = process.env.PLAYWRIGHT_BROWSERS_PATH || "/opt/pw-browsers";
if (existsSync(root)) {
  const d = readdirSync(root).find((x) => /^chromium-\d/.test(x));
  if (d) executablePath = [join(root, d, "chrome-linux64", "chrome"), join(root, d, "chrome-linux", "chrome")].find(existsSync);
}
const browser = await chromium.launch({ executablePath, args: ["--no-sandbox"] });
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

  // Home: drop zone, Gần đây (live: Galaxy appears after 2.5 s), Danh bạ
  await open(Q, "Kéo vào đây để gửi");
  await shot(p, `30-home${sfx}.png`, 3200);
  await p.getByText("Có link hoặc lời mời?").evaluate((el) => el.scrollIntoView({ block: "end" }));
  await shot(p, `31-home-scrolled${sfx}.png`);
  await p.getByRole("button", { name: /Toss dùng thế nào/ }).click();
  await p.getByText("Cùng mạng:").evaluate((el) => el.scrollIntoView({ block: "center" }));
  await shot(p, `32-howto${sfx}.png`);

  // Home with nothing around
  await open(`${Q}&nearby=0&contacts=0`, "Chưa thấy máy nào cùng mạng");
  await shot(p, `33-home-empty${sfx}.png`);

  // a device sheet
  await open(Q, "iPhone của Hùng");
  await p.getByRole("button", { name: /iPhone của Hùng/ }).click();
  await p.getByRole("button", { name: "Gửi file…" }).last().waitFor();
  await shot(p, `34-peer-sheet${sfx}.png`);

  // "Gửi tới ai?" for files, and the browser share
  await open(`${Q}&send=1`, "Chia sẻ qua trình duyệt");
  await shot(p, `35-send-picker${sfx}.png`, 1500);
  await p.getByRole("dialog").evaluate((el) => { el.scrollTop = el.scrollHeight; });
  await shot(p, `35b-send-picker-bottom${sfx}.png`, 600);
  await p.getByText("Chia sẻ qua trình duyệt").click();
  await p.getByText("Đang chia sẻ").last().waitFor();
  await p.getByRole("dialog").evaluate((el) => { el.scrollTop = el.scrollHeight; });
  await shot(p, `36-send-webshare${sfx}.png`, 1400);

  // "Gửi chữ tới ai?"
  await open(Q, "Kéo vào đây để gửi");
  await p.getByRole("button", { name: "Gửi chữ" }).click();
  await p.getByText("Gửi chữ tới").first().waitFor();
  await p.getByLabel("Nội dung cần gửi").fill("Mật khẩu wifi studio: toss-2007-vui");
  await shot(p, `37-send-text${sfx}.png`, 1400);

  // incoming asks (Toss stranger + LocalSend queued), and a text message
  await open(`${Q}&incoming=2`, "muốn gửi cho bạn");
  await shot(p, `38-incoming${sfx}.png`, 1500);
  await open(`${Q}&text=1`, "gửi chữ");
  await shot(p, `39-text-message${sfx}.png`, 1300);

  // Transfers: LocalSend rows, Gửi lại, Đã nhận
  await open(Q, "Kéo vào đây để gửi");
  await p.getByRole("button", { name: "Chuyển file" }).click();
  await p.getByText("Đã nhận", { exact: true }).waitFor();
  await shot(p, `40-transfers${sfx}.png`, 1200);
  await p.getByText("Đã nhận", { exact: true }).evaluate((el) => el.scrollIntoView({ block: "start" }));
  await shot(p, `41-transfers-received${sfx}.png`);

  // Settings: 3 sections + collapsed Chẩn đoán & nâng cao
  await p.getByRole("button", { name: "Cài đặt" }).click();
  await p.getByRole("heading", { name: "Máy này" }).waitFor();
  await shot(p, `42-settings${sfx}.png`, 1200);
  await p.getByRole("heading", { name: "Nhận", exact: true }).evaluate((el) => el.scrollIntoView({ block: "start" }));
  await shot(p, `43-settings-receive${sfx}.png`);
  await p.getByRole("button", { name: /Chẩn đoán/ }).click();
  await p.getByText("Kết nối", { exact: true }).first().waitFor();
  await p.getByRole("button", { name: /Chẩn đoán/ }).evaluate((el) => el.scrollIntoView({ block: "start" }));
  await shot(p, `44-settings-advanced${sfx}.png`, 1200);

  // one-screen wizard, firewall banner
  await open("quiet=1&wizard=1", "Chào mừng đến");
  await shot(p, `45-wizard${sfx}.png`, 1200);
  await open("quiet=1&fw=missing", "Cho phép Toss qua firewall");
  await shot(p, `46-home-firewall${sfx}.png`, 1200);

  await p.close();
  await ctx.close();
}
await browser.close();
await server.close();
if (errors.length) { console.error("page errors:\n" + errors.join("\n")); process.exit(1); }
