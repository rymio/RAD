// Gilgamesh's spoken text. Answers carry tags in square brackets that are acted on and never
// shown or read: a mood ([happy]) for the face, a gesture ([nod]) for the head, and actions
// ([remember: …], [open: music]) for RAD. Answers are spoken sentence by sentence.

export const MOODS = [
  'neutral', 'happy', 'amused', 'excited', 'concerned', 'sad', 'surprised', 'curious',
  'skeptical', 'thinking', 'playful', 'sympathetic', 'serious', 'proud',
  'angry', 'annoyed', 'disgusted', 'worried', 'scared', 'confused', 'shy', 'smug', 'bored',
  'sleepy', 'relieved', 'determined', 'tender', 'mischievous', 'disappointed',
] as const;
export type Mood = (typeof MOODS)[number];

export const GESTURES = [
  'nod', 'shake', 'tilt', 'wink', 'laugh', 'eyeroll', 'sigh', 'gasp', 'smirk', 'eyebrows',
  'yawn', 'blush', 'shrug', 'lookaway', 'bow',
] as const;
export type Gesture = (typeof GESTURES)[number];

export const ACTIONS = ['remember', 'note', 'event', 'open', 'search', 'music', 'check', 'bot'] as const;
export type ActionName = (typeof ACTIONS)[number];

export type Tag =
  | { kind: 'mood'; name: Mood }
  | { kind: 'gesture'; name: Gesture }
  | { kind: 'action'; name: ActionName; arg: string };

const NAMES = [...MOODS, ...GESTURES, ...ACTIONS].join('|');
const TAG = new RegExp(`\\[(${NAMES})(?:\\s*:\\s*([^\\]\\n]*))?\\]`, 'gi');
/** A tag still being written at the end of a streamed answer. */
const OPEN_TAG = new RegExp(`\\[(?:${NAMES}|[a-z]{0,14})(?::[^\\]\\n]*)?$`, 'i');

const isMood = (s: string): s is Mood => (MOODS as readonly string[]).includes(s);
const isGesture = (s: string): s is Gesture => (GESTURES as readonly string[]).includes(s);

/** The tags in a piece of text (in order), and the text without them. */
export function parseTags(s: string): { text: string; tags: Tag[] } {
  const tags: Tag[] = [];
  const text = s
    .replace(TAG, (_, n: string, arg?: string) => {
      const name = n.toLowerCase();
      if (isMood(name)) tags.push({ kind: 'mood', name });
      else if (isGesture(name)) tags.push({ kind: 'gesture', name });
      else if (arg?.trim()) tags.push({ kind: 'action', name: name as ActionName, arg: arg.trim() });
      return ' ';
    })
    .replace(/[ \t]{2,}/g, ' ')
    .replace(/ +([.,!?;:])/g, '$1')
    .trim();
  return { text, tags };
}

/** Text for showing or reading: no tags (and no half-written tag at the end while streaming).
 *  Only the tags go, so Markdown (code indentation, links) is left as it is. */
export const stripTags = (s: string) => (s.includes('[') ? s.replace(OPEN_TAG, '').replace(new RegExp(TAG.source + ' ?', 'gi'), '') : s);

/** A Gilgamesh answer: it starts with a mood tag. */
export const isGilgamesh = (s: string) => new RegExp(`^\\s*\\[(${MOODS.join('|')})\\]`, 'i').test(s);

/** Splits off the finished sentences at the start of `buf`; the rest stays for later. */
export function takeSentences(buf: string, final: boolean): { sentences: string[]; rest: string } {
  const sentences: string[] = [];
  let rest = buf;
  const end = /[.!?…]+["')\]]*\s|\n+/g;
  for (;;) {
    // A sentence ends at . ! ? … followed by a space (so “coursemat.au” stays whole), or a new
    // line, but never inside a [tag: …].
    end.lastIndex = 0;
    let cut = -1;
    for (let m = end.exec(rest); m; m = end.exec(rest)) {
      const at = m.index + m[0].length;
      const head = rest.slice(0, at);
      if ((head.match(/\[/g)?.length ?? 0) <= (head.match(/\]/g)?.length ?? 0)) {
        cut = at;
        break;
      }
    }
    if (cut < 0) break;
    const s = rest.slice(0, cut).trim();
    rest = rest.slice(cut);
    if (s) sentences.push(s);
  }
  // A long first piece is spoken from a comma, so the first words come sooner.
  if (!sentences.length && rest.length > 140 && !OPEN_TAG.test(rest.slice(0, 140))) {
    const c = rest.lastIndexOf(', ', 140);
    if (c > 40 && !rest.slice(0, c).includes('[')) {
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

/** Citation markers like [1] or [2][3]: kept in the transcript's data, not shown in its text. */
export const citedNumbers = (s: string) => new Set([...s.matchAll(/\[(\d{1,2})\]/g)].map((m) => Number(m[1])));
export const stripCitations = (s: string) => s.replace(/\s*\[\d{1,2}\](?:\[\d{1,2}\])*/g, '');
