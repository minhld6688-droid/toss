// Short links for Toss: https://<host>/<code> ↔ a signed ticket / invite.
//
//   POST /api/s        {"t": "toss1…" | "tossinv1…"}  → {"code", "url"}
//   GET  /api/s?c=code                                 → {"t", "meta"}
//
// Only tickets/invites with a valid ed25519 signature are stored, so the store can
// hold nothing but real Toss links. The desktop app verifies the signature again
// and the host checks it once more, so this service cannot grant access by itself.
// Storage: Vercel Blob (BLOB_READ_WRITE_TOKEN, set when the store is connected).

import { put, head } from '@vercel/blob';
import { check, codeFor, CODE_RE } from './_ticket.js';

const KEY = (code) => `s/${code}.json`;

function send(res, status, body) {
  res.statusCode = status;
  res.setHeader('Content-Type', 'application/json; charset=utf-8');
  res.setHeader('Cache-Control', 'no-store');
  res.setHeader('X-Content-Type-Options', 'nosniff');
  res.end(JSON.stringify(body));
}

async function readBody(req) {
  if (req.body && typeof req.body === 'object') return req.body;
  if (typeof req.body === 'string') return JSON.parse(req.body);
  let s = '';
  for await (const chunk of req) {
    s += chunk;
    if (s.length > 8192) throw new Error('too long');
  }
  return JSON.parse(s || '{}');
}

/** Vercel Blob storage: `s/<code>.json` = {t, exp, at}. */
export const blobStore = {
  async load(code) {
    let meta;
    try { meta = await head(KEY(code)); } catch { return null; }
    const r = await fetch(meta.url, { cache: 'no-store' });
    if (!r.ok) return null;
    return r.json();
  },
  async save(code, rec) {
    await put(KEY(code), JSON.stringify(rec), {
      access: 'public',
      addRandomSuffix: false,
      allowOverwrite: true,
      contentType: 'application/json',
      cacheControlMaxAge: 60,
    });
  },
};

export function makeHandler(store) {
  const load = (code) => store.load(code);
  return async function handler(req, res) {
    try {
      if (req.method === 'POST') {
        let body;
        try { body = await readBody(req); } catch { return send(res, 400, { error: 'bad json' }); }
        let c;
        try { c = check(body && body.t); } catch (e) { return send(res, 400, { error: String(e.message || e) }); }
        // Same text → same code. On the (2^-45) chance of a clash: reject, the app keeps the long link.
        const code = codeFor(c.text);
        const prev = await load(code);
        if (prev && prev.t !== c.text) return send(res, 409, { error: 'code clash' });
        if (!prev) {
          await store.save(code, { t: c.text, exp: c.exp, at: Math.floor(Date.now() / 1000) });
        }
        const host = req.headers['x-forwarded-host'] || req.headers.host;
        return send(res, 200, { code, url: `https://${host}/${code}` });
      }

      if (req.method === 'GET') {
        const url = new URL(req.url, 'http://x');
        const code = (url.searchParams.get('c') || '').toLowerCase();
        if (!CODE_RE.test(code)) return send(res, 400, { error: 'bad code' });
        const rec = await load(code);
        if (!rec) return send(res, 404, { error: 'not found' });
        if (rec.exp && rec.exp <= Math.floor(Date.now() / 1000)) return send(res, 410, { error: 'expired' });
        let c;
        try { c = check(rec.t); } catch (e) { return send(res, 410, { error: String(e.message || e) }); }
        return send(res, 200, { t: c.text, meta: c.meta });
      }

      res.setHeader('Allow', 'GET, POST');
      return send(res, 405, { error: 'method' });
    } catch (e) {
      console.error('shortlink', e);
      return send(res, 500, { error: 'server' });
    }
  };
}

export default makeHandler(blobStore);
