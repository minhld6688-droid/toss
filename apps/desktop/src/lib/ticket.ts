// Detect a Toss ticket inside whatever the user pasted:
//   toss1abc…            raw ticket
//   toss://t/toss1abc…   deep link
//   https://host/#toss1abc…   web link
// The shell validates for real (parse_ticket); this only finds the candidate.
const RE = /toss1[a-z2-7]{16,}/i;

export function extractTicket(text: string): string | null {
  const m = text.match(RE);
  return m ? m[0].toLowerCase() : null;
}
