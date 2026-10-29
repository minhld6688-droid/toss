// Publishes the interesting lines of cargo output files as one ::error annotation each.
// Job logs need a GitHub login to read; annotations are public. Usage: node annotate.mjs a.txt b.txt
import { existsSync, readFileSync } from "node:fs";
const re = /^error|: error|panicked|FAILED|failed|-->|^---- |assert|left:|right:/;
for (const f of process.argv.slice(2)) {
  if (!existsSync(f)) continue;
  const lines = readFileSync(f, "utf8").replace(/\r/g, "").split("\n").filter((l) => re.test(l)).slice(0, 60);
  if (lines.length) console.log(`::error title=${f}::` + lines.join("\n").replace(/%/g, "%25").replace(/\n/g, "%0A"));
}
