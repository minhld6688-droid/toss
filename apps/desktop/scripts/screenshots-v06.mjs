// v0.6 screens (every machine is the same, "Gửi tới ai?", Nâng cao): node scripts/screenshots-v06.mjs
import { createServer } from "vite";
import { chromium } from "playwright";
import { mkdirSync, existsSync, readdirSync } from "node:fs";
import { join } from "node:path";

process.env.VITE_MOCK = "1";
const out = new URL("../screenshots/", import.meta.url).pathname;
mkdirSync(out, { recursive: true });
const server = await createServer({ server: { port: 1422, strictPort: false, host: "127.0.0.1" }, logLevel: "error" });
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
async function shot(page, name) { await page.waitForTimeout(700); await page.screenshot({ path: join(out, name) }); console.log("wrote", name); }

for (const scheme of ["light", "dark"]) {
  const ctx = await browser.newContext({ viewport: { width: 400, height: 600 }, deviceScaleFactor: 2, colorScheme: scheme });
  const sfx = scheme === "dark" ? "-dark" : "";
  const p = await ctx.newPage();
  p.on("pageerror", (e) => errors.push(String(e)));
  await p.goto(base + "?update=1");
  await p.getByText("Kéo folder vào để gửi").waitFor();
  await shot(p, `20-home${sfx}.png`);
  await p.getByRole("button", { name: /Toss dùng thế nào/ }).click();
  await p.getByText("Người lạ:").evaluate((el) => el.scrollIntoView({ block: "center" }));
  await shot(p, `21-howto${sfx}.png`);

  await p.goto(base + "?send=1");
  await p.getByText("Tạo link", { exact: true }).waitFor();
  await shot(p, `22-send-picker${sfx}.png`);

  await p.getByRole("button", { name: "Huỷ" }).click();
  await p.getByRole("button", { name: "Cài đặt" }).click();
  await p.getByRole("heading", { name: "Máy này" }).waitFor();
  await shot(p, `23-settings${sfx}.png`);
  await p.getByRole("button", { name: "Nâng cao" }).click();
  await p.getByText("Folder cố định", { exact: true }).first().waitFor();
  await p.getByText("Folder cố định", { exact: true }).first().evaluate((el) => el.scrollIntoView({ block: "start" }));
  await shot(p, `24-settings-advanced${sfx}.png`);
  await p.close();
  await ctx.close();
}
await browser.close();
await server.close();
if (errors.length) { console.error("page errors:\n" + errors.join("\n")); process.exit(1); }
