// Local e2e for the link page. Serves ../ with the headers from vercel.json so CSP is exercised.
// Run: PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers npm test   (in web/tests)
import http from 'node:http';
import { readFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { chromium } from 'playwright';
import { makeHandler } from '../api/s.js';
import FIX from './fixtures.json' with { type: 'json' };

const web = join(dirname(fileURLToPath(import.meta.url)), '..');
const shots = join(web, 'screenshots');
mkdirSync(shots, { recursive: true });
execFileSync('node', [join(web, 'scripts/csp-hash.mjs'), '--check'], { stdio: 'inherit' });

const cfg = JSON.parse(readFileSync(join(web, 'vercel.json'), 'utf8'));
const headers = Object.fromEntries(cfg.headers.find((h) => h.source === '/(.*)').headers.map((h) => [h.key, h.value]));
// v0.5.2 short links: /api/s backed by memory, seeded with real signed fixtures.
const mem = new Map();
const api = makeHandler({ load: async (c) => mem.get(c) ?? null, save: async (c, r) => { mem.set(c, r); } });
async function shorten(t) {
  const res = { statusCode: 0, setHeader() {}, end(s) { this.out = s; } };
  await api({ method: 'POST', url: '/api/s', headers: { host: 'x' }, body: { t } }, res);
  return JSON.parse(res.out).code;
}
const server = http.createServer((req, res) => {
  if (req.url.startsWith('/api/s')) return void api(req, res);
  res.writeHead(200, { ...headers, 'Content-Type': 'text/html; charset=utf-8' });
  res.end(readFileSync(join(web, 'index.html')));
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const base = `http://127.0.0.1:${server.address().port}/`;

// Deterministic fake ticket: toss1 + 330 base32 chars.
const b32 = 'abcdefghijklmnopqrstuvwxyz234567';
let ticket = 'toss1';
for (let i = 0; i < 330; i++) ticket += b32[(i * 7 + (i >> 3)) % 32];

const browser = await chromium.launch(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {});
const results = [];
async function check(name, fn) {
  try { await fn(); results.push(['ok', name]); }
  catch (e) { results.push(['FAIL', name, e.message]); }
}
async function open(path, { ua, viewport = { width: 420, height: 900 }, colorScheme = 'light' } = {}) {
  const ctx = await browser.newContext({ viewport, colorScheme, userAgent: ua, permissions: ['clipboard-read', 'clipboard-write'] });
  const page = await ctx.newPage();
  const problems = [];
  page.on('console', (m) => { if (m.type() === 'error') problems.push(m.text()); });
  page.on('pageerror', (e) => problems.push(String(e)));
  const requests = [];
  page.on('request', (r) => requests.push(r.url()));
  await page.goto(base + path, { waitUntil: 'commit' }); // headless chromium never settles goto() once toss:// is requested
  await page.waitForSelector('main .card:not([hidden])', { state: 'attached' });
  return { ctx, page, problems, requests };
}
const vis = (page, id) => page.locator('#' + id).isVisible();

await check('landing without hash', async () => {
  const { ctx, page, problems } = await open('', { ua: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/120 Safari/537.36' });
  assert(await vis(page, 'view-landing'));
  assert(!(await vis(page, 'view-link')));
  const links = await page.locator('#view-landing a.btn').evaluateAll((as) => as.map((a) => a.href));
  assert.deepEqual(links.sort(), [
    'https://github.com/minhld6688-droid/toss/releases/latest/download/Toss-setup.exe',
    'https://github.com/minhld6688-droid/toss/releases/latest/download/Toss.dmg',
  ]);
  await page.screenshot({ path: join(shots, 'landing.png'), fullPage: true });
  assert.deepEqual(problems, []);
  await ctx.close();
});

await check('valid ticket: fallback after 1500ms, no external requests, no CSP errors', async () => {
  const { ctx, page, problems, requests } = await open('#' + ticket, { ua: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/120 Safari/537.36' });
  assert(await vis(page, 'view-link'));
  assert(await vis(page, 'state-opening'));
  assert.equal(await page.locator('iframe').getAttribute('src'), 'toss://t/' + ticket);
  await page.waitForTimeout(1200);
  assert(!(await vis(page, 'state-fallback')), 'fallback must not show before 1500ms');
  await page.waitForTimeout(700);
  assert(await vis(page, 'state-fallback'));
  assert(await page.locator('#state-fallback .hint', { hasText: 'Toss chưa mở?' }).isVisible());
  assert(await page.getByRole('button', { name: 'Mở lại trong Toss' }).isVisible());
  assert(await page.getByRole('link', { name: 'Tải Toss cho Mac (.dmg)' }).isVisible());
  assert(await page.getByRole('link', { name: 'Tải Toss cho Windows (.exe)' }).isVisible());
  // Windows UA => Windows button first and highlighted.
  assert.equal(await page.locator('#state-fallback a.btn').first().getAttribute('data-platform'), 'win');
  await page.getByRole('button', { name: 'Copy ticket' }).click();
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), ticket);
  await page.screenshot({ path: join(shots, 'link.png'), fullPage: true });
  assert.deepEqual(requests.filter((u) => !u.startsWith(base) && !u.startsWith('data:') && !u.startsWith('toss:')), []);
  assert.deepEqual(problems.filter((p) => /Content Security Policy|Refused/i.test(p)), []);
  await ctx.close();
});

await check('left page (blur/hidden) within 1500ms => opened state, help on demand', async () => {
  const { ctx, page } = await open('#' + ticket);
  await page.evaluate(() => window.dispatchEvent(new Event('blur')));
  await page.waitForTimeout(1800);
  assert(await vis(page, 'state-opened'));
  assert(!(await vis(page, 'state-fallback')));
  await page.getByRole('button', { name: 'Toss chưa mở?' }).click();
  assert(await vis(page, 'state-fallback'));
  await ctx.close();
});

await check('tolerates toss:// prefix, https prefix, uppercase, percent-encoding', async () => {
  for (const frag of ['toss://t/' + ticket, 'https://x.example/#' + ticket, ticket.toUpperCase(), encodeURIComponent('toss://t/' + ticket)]) {
    const { ctx, page } = await open('#' + frag);
    assert(await vis(page, 'view-link'), 'valid: ' + frag.slice(0, 30));
    assert.equal(await page.locator('iframe').getAttribute('src'), 'toss://t/' + ticket);
    await ctx.close();
  }
});

// v0.5: a pairing invite (tossinv1…) redirects to toss://i/<invite>, never to the ticket scheme.
let invite = 'tossinv1';
for (let i = 0; i < 420; i++) invite += b32[(i * 5 + (i >> 2)) % 32];

await check('invite: opens toss://i/, invite wording, copy button copies the invite', async () => {
  for (const frag of ['#' + invite, '#toss://i/' + invite, '#https://x.example/#' + invite, '#' + invite.toUpperCase()]) {
    const { ctx, page, problems } = await open(frag, { ua: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/120 Safari/537.36' });
    assert(await vis(page, 'view-link'), 'valid invite: ' + frag.slice(0, 30));
    assert.equal(await page.locator('iframe').getAttribute('src'), 'toss://i/' + invite);
    assert(await page.locator('#invite-intro').isVisible());
    assert.deepEqual(problems.filter((p) => /Content Security Policy|Refused/i.test(p)), []);
    if (frag === '#' + invite) {
      await page.waitForTimeout(1800);
      assert(await vis(page, 'state-fallback'));
      await page.getByRole('button', { name: 'Copy lời mời' }).click();
      assert.equal(await page.evaluate(() => navigator.clipboard.readText()), invite);
      await page.screenshot({ path: join(shots, 'invite.png'), fullPage: true });
    }
    await ctx.close();
  }
  for (const bad of ['tossinv1short', 'tossinv1' + 'a'.repeat(60) + '1', 'tossinv1' + 'a'.repeat(4000)]) {
    const { ctx, page } = await open('#' + bad);
    assert(await vis(page, 'view-invalid'), 'invalid invite: ' + bad.slice(0, 20));
    assert.equal(await page.locator('iframe').count(), 0);
    await ctx.close();
  }
});

await check('invalid tickets', async () => {
  const bad = ['abc', 'toss1short', 'toss1' + 'a'.repeat(44), 'toss2' + 'a'.repeat(100), 'toss1' + 'a'.repeat(60) + '!', 'toss1' + 'a'.repeat(60) + '1', 'toss1' + 'a'.repeat(2000)];
  for (const frag of bad) {
    const { ctx, page } = await open('#' + frag);
    assert(await vis(page, 'view-invalid'), 'invalid: ' + frag.slice(0, 30));
    assert(await page.getByText('Link không hợp lệ').isVisible());
    assert.equal(await page.locator('iframe').count(), 0);
    await ctx.close();
  }
  const { ctx, page } = await open('#toss1' + 'a'.repeat(60));
  assert(await vis(page, 'view-link'));
  await ctx.close();
  const inv = await open('#abc');
  await inv.page.screenshot({ path: join(shots, 'invalid.png'), fullPage: true });
  await inv.ctx.close();
});

await check('mobile UA shows desktop-only note; dark scheme renders', async () => {
  const { ctx, page } = await open('#' + ticket, { ua: 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 Mobile/15E148 Safari/604.1', colorScheme: 'dark', viewport: { width: 390, height: 844 } });
  await page.waitForTimeout(1700);
  assert(await page.locator('#state-fallback [data-mobile-note]').isVisible());
  const sw = await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth);
  assert(sw, 'no horizontal scroll at 390px');
  await page.screenshot({ path: join(shots, 'link-mobile-dark.png'), fullPage: true });
  await ctx.close();
});

await check('short link: resolves via /api/s, shows the job, opens toss://t/<ticket>', async () => {
  const code = await shorten(FIX.ticket);
  const { ctx, page, problems, requests } = await open(code, { ua: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/120 Safari/537.36' });
  await page.waitForSelector('iframe', { state: 'attached' });
  assert.equal(await page.locator('iframe').getAttribute('src'), 'toss://t/' + FIX.ticket);
  assert.equal(await page.locator('#job-name').textContent(), FIX.job);
  assert.match(await page.locator('#job-sub').textContent(), /Tải về và gửi lại · không hết hạn/);
  assert.equal(await page.title(), FIX.job + ' · Toss');
  await page.waitForTimeout(1700);
  assert(await vis(page, 'state-fallback'));
  await page.screenshot({ path: join(shots, 'short-link.png'), fullPage: true });
  assert.deepEqual(requests.filter((u) => !u.startsWith(base) && !u.startsWith('data:') && !u.startsWith('toss:')), []);
  assert.deepEqual(problems.filter((p) => /Content Security Policy|Refused/i.test(p)), []);
  await ctx.close();
});

await check('short invite link opens toss://i/<invite> with the inviter name', async () => {
  const code = await shorten(FIX.invite);
  const { ctx, page } = await open(code.toUpperCase());
  await page.waitForSelector('iframe', { state: 'attached' });
  assert.equal(await page.locator('iframe').getAttribute('src'), 'toss://i/' + FIX.invite);
  assert.equal(await page.locator('#job-name').textContent(), 'Lời mời từ ' + FIX.inviteName);
  await ctx.close();
});

await check('unknown short code => "Không tìm thấy link"', async () => {
  const { ctx, page } = await open('aaaaaaaaa');
  await page.waitForSelector('#view-invalid:not([hidden])');
  assert(await page.getByText('Không tìm thấy link').isVisible());
  assert.equal(await page.locator('iframe').count(), 0);
  await ctx.close();
});

await browser.close();
server.close();
let failed = 0;
for (const [s, n, m] of results) { console.log(s === 'ok' ? 'PASS' : 'FAIL', n, m ?? ''); if (s !== 'ok') failed++; }
process.exit(failed ? 1 : 0);
