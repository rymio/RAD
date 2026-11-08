// Coach (Media → Coach): what Gilgamesh needs to know about the owner's accent practice, and the
// handles he works it with. The Coach view (components/coach/CoachApp.svelte) keeps this up to date
// and registers its controls here while it's open.
import { issueNames, type AttemptResult, type SentenceRef, type TextDetail, type VowelChartData } from './coach/api';

export interface CoachControls {
  listen: () => Promise<void>;
  goTo: (index: number) => Promise<void>;
  /** Adds a practice text and opens it. */
  addText: (title: string, body: string) => Promise<void>;
  /** Opens one of the owner's texts or PDFs by (part of) its title; false when none matches. */
  openByTitle: (title: string) => Promise<boolean>;
  setView: (v: 'melody' | 'vowels') => void;
  setMode: (m: 'sentence' | 'page') => Promise<void>;
  hearMe: () => Promise<void>;
}

const pct = (x: number) => `${Math.round(x * 100)}%`;

class CoachStore {
  /** The Coach view is open. */
  open = $state(false);
  /** Coach has started and loaded the owner's texts. */
  ready = $state(false);
  text = $state<TextDetail | null>(null);
  index = $state(0);
  reference = $state<SentenceRef | null>(null);
  result = $state<AttemptResult | null>(null);
  phase = $state<'idle' | 'loading' | 'recording' | 'analysing'>('idle');
  mode = $state<'sentence' | 'page'>('sentence');
  view = $state<'melody' | 'vowels'>('melody');
  chart = $state<VowelChartData | null>(null);
  calibrated = $state(false);
  coachName = $state('rules');
  texts = $state<{ id: number; title: string }[]>([]);
  documents = $state<{ id: number; title: string }[]>([]);
  controls: CoachControls | null = null;

  /** The sentence on screen. */
  get sentence(): string | null {
    return this.text?.sentences[this.index] ?? null;
  }

  /** What Gilgamesh sees of it (only while Coach is open). */
  describe(): string {
    if (!this.open) return '';
    const lines: string[] = [
      "THE OWNER'S ACCENT PRACTICE (open now: Media → Coach). They are practising a General Australian accent by reading aloud. Coach compares each reading with an Australian reference reading: the melody (pitch, DTW-aligned), pace, pauses, the ending's rise or fall, pitch range, each word's sounds (wav2vec2 phonemes, goodness of pronunciation), skipped words (Whisper), the Australian vowels in 'day', 'my', 'no', 'now', 'who' (FACE, PRICE, GOAT, MOUTH, GOOSE, Wells lexical sets) and the non-rhotic R. Its measurements are reliable; build on them, don't contradict them.",
    ];
    const t = this.text;
    if (!t) {
      lines.push(`No text is open. Their texts: ${this.texts.map((x) => `"${x.title}"`).join(', ') || '(none)'}${this.documents.length ? `; PDFs: ${this.documents.map((x) => `"${x.title}"`).join(', ')}` : ''}.`);
      return lines.join('\n');
    }
    const title = t.document ? `${t.document.title}, page ${t.document.page_no}` : t.title;
    lines.push(`Text: "${title}" (${t.sentences.length} sentences). Reference: ${t.reference ? t.reference.label : 'none yet (they need to attach a narration or pick an Australian voice)'}. Practising ${this.mode === 'page' ? 'the whole page in one go' : `sentence by sentence, on sentence ${this.index + 1}: "${this.sentence}"`}. Their voice is ${this.calibrated ? 'calibrated (vowel feedback is on)' : 'not calibrated yet (eight short sentences under "Your voice" turn on vowel feedback)'}. Feedback comes from ${this.coachName === 'rules' ? 'measured rules' : `a local model (${this.coachName})`}.`);
    if (this.phase === 'recording') lines.push('They are reading it aloud right now: keep quiet until they finish.');
    const r = this.result;
    if (r && this.mode === 'sentence' && r.sentence === this.index) {
      const c = r.comparison;
      const parts: string[] = [];
      if (c.contour_corr != null) parts.push(`tune match ${c.contour_corr.toFixed(2)} (1 = same melody)`);
      parts.push(`speaking rate ${c.articulation_ratio.toFixed(2)}× the reference's`);
      parts.push(`pitch range ${c.range_ratio.toFixed(2)}× the reference's`);
      parts.push(`ending ${c.final_rise ? 'rising' : 'falling'} (reference ${c.reference_final_rise ? 'rising' : 'falling'})`);
      parts.push(`${c.attempt_rate.pause_count} pauses (reference ${c.reference_rate.pause_count})`);
      lines.push(`Their last reading of this sentence: ${parts.join(', ')}.`);
      if (r.heard) lines.push(`Whisper heard: "${r.heard}".`);
      lines.push(`Coach said: "${r.coach.output.spoken}"`);
      if (r.issues.length)
        lines.push(`Issues: ${r.issues.map((i) => `${issueNames[i.feature] ?? i.feature}${i.word ? ` ("${i.word}")` : ''}, severity ${i.severity.toFixed(2)}: ${i.detail}`).join('; ')}.`);
      const graded = r.words.filter((w) => w.grade || w.missed);
      if (graded.length)
        lines.push(`Words: ${graded.map((w) => (w.missed ? `${w.display} skipped` : `${w.display} ${w.grade} ${w.score != null ? pct(w.score) : ''}${w.phones.length ? ` [${w.phones.map((p) => `${p.phone} ${pct(p.score)}`).join(' ')}]` : ''}`)).join('; ')}.`);
      const v = r.vowels;
      if (v.vowels.length) lines.push(`Vowels: ${v.vowels.map((x) => `"${x.word}" (${x.keyword}, ${x.set}) ${x.grade}: ${x.detail}${x.hint ? ` Hint: ${x.hint}` : ''}`).join('; ')}.`);
      if (v.rhotic.some((x) => x.flagged)) lines.push(`R: ${v.rhotic.filter((x) => x.flagged).map((x) => `"${x.word}": ${x.detail}`).join('; ')}.`);
      if (v.status.state !== 'ready') lines.push(`(Vowel feedback is off: ${v.status.state.replace(/_/g, ' ')}.)`);
    } else if (this.phase === 'analysing') {
      lines.push('Coach is analysing their reading now.');
    }
    lines.push(
      'You can work Coach for them: [coach: listen] plays the reference reading of the sentence; [coach: next], [coach: previous] or [coach: sentence | 3] move between sentences; [coach: hear] plays back their last reading; [coach: vowels] or [coach: melody] switches the graph; [coach: open | title] opens one of their texts. To give them new practice material (e.g. sentences full of the "now" vowel, or Aussie slang), write the sentences in a fenced text block, one per line, then [coach: practise | a short title]: it becomes a text in Coach, ready to read. When they ask how to sound more Australian, be specific and physical (tongue, lips, jaw, which vowel glides where), using what Coach measured.',
    );
    return lines.join('\n');
  }
}

export const coach = new CoachStore();

/** The sentences Gilgamesh wrote for practice: his last fenced block, else his list items. */
export function practiceText(md: string): string | null {
  const blocks = [...md.matchAll(/```[^\n]*\n([\s\S]*?)```/g)];
  if (blocks.length) return blocks.at(-1)![1].trim() || null;
  const items = md
    .split('\n')
    .map((l) => l.match(/^\s*(?:[-*•]|\d+[.)])\s+(.+)$/)?.[1]?.replace(/[*_`"“”]/g, '').trim())
    .filter((x): x is string => !!x && x.length > 2);
  return items.length ? items.join('\n') : null;
}
