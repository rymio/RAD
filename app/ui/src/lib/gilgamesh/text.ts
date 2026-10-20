// Gilgamesh's spoken text: answers start with a mood tag such as [happy] (the face shows it;
// it's hidden wherever the text is shown or read), and are spoken sentence by sentence.

export type Mood = 'neutral' | 'happy' | 'amused' | 'concerned' | 'surprised' | 'thinking';
export const MOODS: Mood[] = ['neutral', 'happy', 'amused', 'concerned', 'surprised', 'thinking'];

const TAG = new RegExp(`\\[(${MOODS.join('|')})\\]`, 'gi');

/** The mood a sentence asks for (its last tag), and the sentence without tags. */
export function moodOf(s: string): { mood: Mood | null; text: string } {
  let mood: Mood | null = null;
  const text = s.replace(TAG, (_, m: string) => ((mood = m.toLowerCase() as Mood), '')).replace(/\s{2,}/g, ' ').trim();
  return { mood, text };
}

export const stripMoods = (s: string) => (s.includes('[') ? s.replace(TAG, '').replace(/^\s+/, '') : s);

/** Splits off the finished sentences at the start of `buf`; the rest stays for later. */
export function takeSentences(buf: string, final: boolean): { sentences: string[]; rest: string } {
  const sentences: string[] = [];
  let rest = buf;
  for (;;) {
    // A sentence ends at . ! ? … followed by a space (so “coursemat.au” stays whole), or a new line.
    const m = /[.!?…]+["')\]]*\s|\n+/.exec(rest);
    if (!m) break;
    const end = m.index + m[0].length;
    const s = rest.slice(0, end).trim();
    rest = rest.slice(end);
    if (s) sentences.push(s);
  }
  // A long first piece is spoken from a comma, so the first words come sooner.
  if (!sentences.length && rest.length > 140) {
    const c = rest.lastIndexOf(', ', 140);
    if (c > 40) {
      sentences.push(rest.slice(0, c + 1).trim());
      rest = rest.slice(c + 2);
    }
  }
  if (final && rest.trim()) {
    sentences.push(rest.trim());
    rest = '';
  }
  return { sentences, rest };
}
