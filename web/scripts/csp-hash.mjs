// Recomputes the sha256 of the inline <script> in index.html and writes it into
// vercel.json's Content-Security-Policy. Run after every edit of the script:
//   node scripts/csp-hash.mjs          (update)
//   node scripts/csp-hash.mjs --check  (exit 1 if stale; used by tests)
import { readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const html = readFileSync(join(root, 'index.html'), 'utf8');
const scripts = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)];
if (scripts.length !== 1) throw new Error(`expected exactly 1 inline script, found ${scripts.length}`);
const hash = 'sha256-' + createHash('sha256').update(scripts[0][1]).digest('base64');

const vpath = join(root, 'vercel.json');
const cfg = JSON.parse(readFileSync(vpath, 'utf8'));
const hdr = cfg.headers[0].headers.find((h) => h.key === 'Content-Security-Policy');
const next = hdr.value.replace(/script-src 'sha256-[^']*'/, `script-src '${hash}'`);
if (process.argv.includes('--check')) {
  if (next !== hdr.value) { console.error('CSP hash is stale; run node scripts/csp-hash.mjs'); process.exit(1); }
  console.log('CSP hash ok');
} else {
  hdr.value = next;
  writeFileSync(vpath, JSON.stringify(cfg, null, 2) + '\n');
  console.log('updated', hash);
}
