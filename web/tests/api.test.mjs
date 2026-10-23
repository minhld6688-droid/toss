// Short-link API (web/api/s.js) against real tickets/invites minted by toss-cli.
// Run: node --test tests/api.test.mjs   (TOSS_TICKET / TOSS_INVITE / TOSS_ADMIN override the fixtures)
import test from 'node:test';
import assert from 'node:assert/strict';
import { makeHandler } from '../api/s.js';
import { check, codeFor } from '../api/_ticket.js';
import FIX from './fixtures.json' with { type: 'json' };

const TICKET = process.env.TOSS_TICKET || FIX.ticket;
const INVITE = process.env.TOSS_INVITE || FIX.invite;
const ADMIN = process.env.TOSS_ADMIN || FIX.admin;

function memStore() {
  const m = new Map();
  return { m, load: async (c) => m.get(c) ?? null, save: async (c, r) => { m.set(c, r); } };
}

async function call(h, method, url, body) {
  const req = { method, url, headers: { host: 'toss-link.vercel.app' }, body };
  const res = {
    statusCode: 0, headers: {}, out: '',
    setHeader(k, v) { this.headers[k.toLowerCase()] = v; },
    end(s) { this.out = s; },
  };
  await h(req, res);
  return { status: res.statusCode, json: JSON.parse(res.out), headers: res.headers };
}

test('ticket meta and signature', () => {
  const c = check(TICKET.toUpperCase());
  assert.equal(c.kind, 'ticket');
  assert.equal(c.meta.job, FIX.job);
  assert.equal(c.meta.get, true);
  assert.equal(c.meta.put, true);
  assert.equal(c.meta.exp, 0); // fixture minted with --days 0
});

test('invite meta', () => {
  const c = check(INVITE);
  assert.equal(c.kind, 'invite');
  assert.equal(c.meta.name, FIX.inviteName);
});

test('rejects tampering, admin links and junk', () => {
  // flip one char inside the body (job name area) → signature fails or parse fails
  const i = 60;
  const bad = TICKET.slice(0, i) + (TICKET[i] === 'a' ? 'b' : 'a') + TICKET.slice(i + 1);
  assert.throws(() => check(bad));
  assert.throws(() => check(ADMIN), /admin/);
  assert.throws(() => check('toss1abc'));
  assert.throws(() => check('https://evil.example/x'));
  assert.throws(() => check('toss1' + 'a'.repeat(5000)), /too long/);
});

test('POST then GET round-trips; same text → same code', async () => {
  const store = memStore();
  const h = makeHandler(store);
  const a = await call(h, 'POST', '/api/s', { t: TICKET });
  assert.equal(a.status, 200);
  assert.match(a.json.code, /^[a-z2-7]{9}$/);
  assert.equal(a.json.url, `https://toss-link.vercel.app/${a.json.code}`);
  assert.equal(a.json.code, codeFor(TICKET));
  const b = await call(h, 'POST', '/api/s', JSON.stringify({ t: TICKET }));
  assert.equal(b.json.code, a.json.code);
  assert.equal(store.m.size, 1);

  const g = await call(h, 'GET', `/api/s?c=${a.json.code.toUpperCase()}`);
  assert.equal(g.status, 200);
  assert.equal(g.json.t, TICKET);
  assert.equal(g.json.meta.job, FIX.job);
  assert.equal(g.headers['cache-control'], 'no-store');

  const inv = await call(h, 'POST', '/api/s', { t: INVITE });
  assert.equal(inv.status, 200);
  const gi = await call(h, 'GET', `/api/s?c=${inv.json.code}`);
  assert.equal(gi.json.meta.kind, 'invite');
});

test('errors: bad code, missing, expired, clash, admin, method', async () => {
  const store = memStore();
  const h = makeHandler(store);
  assert.equal((await call(h, 'GET', '/api/s?c=abc')).status, 400);
  assert.equal((await call(h, 'GET', '/api/s?c=aaaaaaaaa')).status, 404);
  store.m.set('bbbbbbbbb', { t: TICKET, exp: 1 });
  assert.equal((await call(h, 'GET', '/api/s?c=bbbbbbbbb')).status, 410);
  store.m.set(codeFor(TICKET), { t: INVITE, exp: 0 });
  assert.equal((await call(h, 'POST', '/api/s', { t: TICKET })).status, 409);
  assert.equal((await call(h, 'POST', '/api/s', { t: ADMIN })).status, 400);
  assert.equal((await call(h, 'POST', '/api/s', 'not json')).status, 400);
  assert.equal((await call(h, 'DELETE', '/api/s')).status, 405);
});
