// Detect a Toss ticket inside whatever the user pasted:
//   toss1abc…            raw ticket
//   toss://t/toss1abc…   deep link
//   https://host/#toss1abc…   web link
// The shell validates for real (parse_ticket); this only finds the candidate.
const RE = /toss1[a-z2-7]{16,}/i;

// v0.5 pairing invite: tossinv1… (also inside toss://i/… and https://host/#tossinv1…)
const INV = /tossinv1[a-z2-7]{16,}/i;

export function extractInvite(text: string): string | null {
  const m = text.match(INV);
  return m ? m[0].toLowerCase() : null;
}

// v0.5.2 short link: https://<host>/<9 base32 chars>; the shell resolves it (expand_link).
const SHORT = /(?:https?:\/\/)?[a-z0-9.-]+\.[a-z]{2,}(?::\d+)?\/([a-z2-7]{9})(?![a-z0-9])\/?/i;

export function extractShort(text: string): string | null {
  const m = text.match(SHORT);
  return m ? m[0] : null;
}

export function extractTicket(text: string): string | null {
  const m = text.match(RE);
  return m ? m[0].toLowerCase() : null;
}
