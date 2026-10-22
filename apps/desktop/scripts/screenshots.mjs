// Mock-UI screenshots: node scripts/screenshots.mjs  (starts its own Vite dev server in mock mode)
import { createServer } from "vite";
import { chromium } from "playwright";
import { mkdirSync, existsSync, readdirSync } from "node:fs";
import { join } from "node:path";

process.env.VITE_MOCK = "1";
const out = new URL("../screenshots/", import.meta.url).pathname;
mkdirSync(out, { recursive: true });

const server = await createServer({ server: { port: 1421, strictPort: false, host: "127.0.0.1" }, logLevel: "error" });
await server.listen();
const base = server.resolvedUrls.local[0];

let executablePath;
const root = process.env.PLAYWRIGHT_BROWSERS_PATH || "/opt/pw-browsers";
if (existsSync(root)) {
  const d = readdirSync(root).find((x) => x.startsWith("chromium-"));
  if (d) executablePath = join(root, d, "chrome-linux", "chrome");
}
const browser = await chromium.launch({ executablePath, args: ["--no-sandbox"] });
const DEMO = "toss://t/toss1" + "mfrggzdfmz2gq5dsnfxgo3tpnzsxg5dpobuwg2lombqxg6jr";

async function shot(page, name) { await page.waitForTimeout(250); await page.screenshot({ path: join(out, name) }); console.log("wrote", name); }

for (const scheme of ["light", "dark"]) {
  const ctx = await browser.newContext({ viewport: { width: 400, height: 600 }, deviceScaleFactor: 2, colorScheme: scheme, permissions: ["clipboard-read", "clipboard-write"] });
  const page = await ctx.newPage();
  const sfx = scheme === "dark" ? "-dark" : "";
  await page.goto(base + "?host=1&remote=1");
  await page.getByText("Dán link hoặc kéo folder vào đây").waitFor();

  // open a ticket -> sheet
  await page.getByPlaceholder(/dán link/i).fill(DEMO);
  await page.getByText("Kéo folder vào để trả").waitFor();
  if (scheme === "light") await shot(page, "04-ticket-sheet.png");
  await page.getByRole("button", { name: "Đóng" }).click();
  await shot(page, `01-home${sfx}.png`);

  // start get from the sheet -> transfers
  await page.getByText("Demo - 12A3").first().click();
  await page.getByRole("button", { name: "Nhận", exact: true }).click();
  await page.waitForTimeout(3500);
  await shot(page, `02-transfers${sfx}.png`);

  // settings
  await page.getByRole("button", { name: "Cài đặt" }).click();
  await shot(page, `03-settings${sfx}.png`);

  // v0.4 diagnostics: Settings → Kết nối (relay case: symmetric NAT, no port mapping, firewall missing), then the connect test
  await page.getByText("Khả năng nối trực tiếp").waitFor();
  await page.getByRole("heading", { name: "Kết nối", exact: true }).evaluate((el) => el.scrollIntoView({ block: "start" }));
  await shot(page, `11-settings-connection${sfx}.png`);
  await page.getByRole("button", { name: /Thử nối tới máy tổng/ }).click();
  await page.getByText(/nối sau/).waitFor({ timeout: 8000 });
  await page.evaluate(() => document.querySelector("main").scrollBy(0, 330));
  await shot(page, `11-settings-connection-test${sfx}.png`);
  if (scheme === "light") {
    await page.getByRole("button", { name: "Nâng cao" }).click();
    await page.waitForTimeout(700);
    await page.evaluate(() => document.querySelector("main").scrollBy(0, 190));
    await shot(page, "11-settings-connection-advanced.png");
    await page.getByRole("button", { name: "Nâng cao" }).click();
    await page.waitForTimeout(600);
  }
  await page.evaluate(() => document.querySelector("main").scrollTo(0, 0));
  if (scheme === "light") {
    // create a link and show it
    await page.getByPlaceholder("VD: 12A3 - RAW").first().fill("12A3 - RAW");
    await page.getByRole("button", { name: "Duyệt" }).first().click();
    await page.getByRole("button", { name: "2026", exact: true }).click();
    await page.getByRole("button", { name: "12A3", exact: true }).click();
    await shot(page, "05-settings-browse.png");
    await page.getByRole("button", { name: "Chọn thư mục này" }).click();
    await page.getByRole("button", { name: "Tạo link" }).first().click();
    await page.getByText(/Link cho/).first().waitFor();
    await page.getByText(/Link cho/).first().evaluate((el) => el.scrollIntoView({ block: "center" }));
    await shot(page, "06-settings-link.png");
    await page.getByText("Máy tổng từ xa", { exact: true }).evaluate((el) => el.scrollIntoView({ block: "start" }));
    await page.waitForTimeout(500);
    await shot(page, "08-settings-remote.png");
  }
  // v0.3 quick share: shell emits `share {path}` (right-click menu) -> Share view, link already created + copied
  {
    const sp = await ctx.newPage();
    await sp.goto(base + "?share=1");
    await sp.getByText("Đã copy vào clipboard").waitFor({ timeout: 8000 });
    await sp.waitForTimeout(500);
    await shot(sp, `09-share${sfx}.png`);
    if (scheme === "light") {
      await sp.locator("select").selectOption("30");
      await sp.getByRole("checkbox").check();
      await shot(sp, "09-share-redo.png");
    }
    await sp.close();
  }
  if (scheme === "light") {
    // settings: auto-receive / context menu toggles + list of shared folders
    await page.getByText("Folder đang chia sẻ", { exact: true }).evaluate((el) => el.scrollIntoView({ block: "start" }));
    await page.waitForTimeout(400);
    await shot(page, "10-settings-shares.png");
  }
  if (scheme === "light") {
    await page.goto(base + "?wizard=1");
    await page.getByText("Tên máy của bạn").waitFor();
    await shot(page, "07-wizard.png");
    // host machine whose firewall blocks UDP: one extra step before "Xong rồi"
    await page.goto(base + "?wizard=1&host=1");
    await page.getByText("Tên máy của bạn").waitFor();
    await page.getByRole("button", { name: "Tiếp tục" }).click();
    await page.getByText("Thư mục nhận file").waitFor();
    await page.getByRole("button", { name: "Tiếp tục" }).click();
    await page.getByRole("button", { name: /Cho phép qua firewall/ }).waitFor();
    await shot(page, "07-wizard-firewall.png");
  }
  await ctx.close();
}
await browser.close();
await server.close();
