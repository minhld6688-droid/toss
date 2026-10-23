// Decoding and checking of Toss tickets (toss1…) and invites (tossinv1…) for the
// short-link API. Formats: crates/toss-core/src/ticket.rs and contacts.rs (postcard:
// body then a 64-byte ed25519 signature over postcard(body)).

import { createHash, createPublicKey, verify } from 'node:crypto';

const B32 = 'abcdefghijklmnopqrstuvwxyz234567';
const SPKI_ED25519 = Buffer.from('302a300506032b6570032100', 'hex');

/** Longest text we store (tickets with 8 addresses are ~600 chars). */
export const MAX_TEXT = 4000;
/** Short code: 9 base32 chars = 45 bits. */
export const CODE_LEN = 9;
export const CODE_RE = /^[a-z2-7]{9}$/;

export function b32decode(s) {
  let bits = 0, val = 0;
  const out = [];
  for (const ch of s) {
    const i = B32.indexOf(ch);
    if (i < 0) throw new Error('bad base32');
    val = (val << 5) | i;
    bits += 5;
    if (bits >= 8) {
      bits -= 8;
      out.push((val >>> bits) & 0xff);
    }
  }
  return Buffer.from(out);
}

export function b32encode(buf) {
  let bits = 0, val = 0, out = '';
  for (const b of buf) {
    val = (val << 8) | b;
    bits += 8;
    while (bits >= 5) {
      bits -= 5;
      out += B32[(val >>> bits) & 31];
    }
  }
  if (bits > 0) out += B32[(val << (5 - bits)) & 31];
  return out;
}

/** Deterministic code: same text → same link, so re-sharing a job keeps its link. */
export function codeFor(text) {
  return b32encode(createHash('sha256').update(text).digest()).slice(0, CODE_LEN);
}

class Reader {
  constructor(buf) { this.b = buf; this.i = 0; }
  byte() {
    if (this.i >= this.b.length) throw new Error('short');
    return this.b[this.i++];
  }
  bytes(n) {
    if (this.i + n > this.b.length) throw new Error('short');
    const s = this.b.subarray(this.i, this.i + n);
    this.i += n;
    return s;
  }
  varint() {
    let v = 0n, shift = 0n;
    for (;;) {
      const b = this.byte();
      v |= BigInt(b & 0x7f) << shift;
      if (!(b & 0x80)) return v;
      shift += 7n;
      if (shift > 63n) throw new Error('varint');
    }
  }
  bool() { const b = this.byte(); if (b > 1) throw new Error('bool'); return b === 1; }
  str() {
    const n = Number(this.varint());
    if (n > 2048) throw new Error('str');
    return this.bytes(n).toString('utf8');
  }
  strs() {
    const n = Number(this.varint());
    if (n > 16) throw new Error('vec');
    const out = [];
    for (let k = 0; k < n; k++) out.push(this.str());
    return out;
  }
}

function sigOk(body, sig, pk) {
  const key = createPublicKey({ key: Buffer.concat([SPKI_ED25519, pk]), format: 'der', type: 'spki' });
  return verify(null, body, key, sig);
}

/**
 * Parses and verifies a ticket or invite. Returns
 * `{ text, kind: 'ticket'|'invite', exp, meta }` or throws with a short reason.
 */
export function check(input) {
  if (typeof input !== 'string') throw new Error('missing');
  const text = input.trim().toLowerCase();
  if (text.length > MAX_TEXT) throw new Error('too long');
  let kind, raw;
  if (/^tossinv1[a-z2-7]+$/.test(text)) { kind = 'invite'; raw = text.slice(8); }
  else if (/^toss1[a-z2-7]+$/.test(text)) { kind = 'ticket'; raw = text.slice(5); }
  else throw new Error('not a toss link');

  const bytes = b32decode(raw);
  if (bytes.length < 64 + 34) throw new Error('short');
  const body = bytes.subarray(0, bytes.length - 64);
  const sig = bytes.subarray(bytes.length - 64);
  const r = new Reader(body);
  const v = r.byte();
  let pk, exp, meta;
  if (kind === 'ticket') {
    if (v !== 2) throw new Error('version');
    r.bytes(16);
    pk = Buffer.from(r.bytes(32));
    const job = r.str(); r.str(); r.str(); r.str();
    const get = r.bool(), put = r.bool(), admin = r.bool();
    exp = Number(r.varint());
    r.str(); r.strs();
    if (admin) throw new Error('admin links stay long');
    meta = { kind, job, get, put, exp };
  } else {
    if (v !== 1) throw new Error('version');
    pk = Buffer.from(r.bytes(32));
    const name = r.str();
    const platform = r.str();
    r.bytes(16);
    exp = Number(r.varint());
    r.str(); r.strs();
    meta = { kind, name, platform, exp };
  }
  if (r.i !== body.length) throw new Error('trailing bytes');
  if (!sigOk(body, sig, pk)) throw new Error('bad signature');
  if (exp && exp <= Math.floor(Date.now() / 1000)) throw new Error('expired');
  return { text, kind, exp, meta };
}
