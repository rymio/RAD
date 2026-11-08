// Typed wrappers over the Rust commands. Field names match the serde output exactly.
import { invoke } from "@tauri-apps/api/core";

export type SourceKind = "narration" | "system_voice" | "cloned";

export interface TextSummary {
  id: number;
  title: string;
  sentence_count: number;
  practised_at: number | null;
}

export interface RefInfo {
  kind: SourceKind;
  label: string;
}

export interface DocInfo {
  id: number;
  title: string;
  /** The PDF's page number. */
  page_no: number;
  /** Position among the pages with text, 0-based. */
  index: number;
  count: number;
  prev: number | null;
  next: number | null;
}

export interface DocSummary {
  id: number;
  title: string;
  page_count: number;
  practised_at: number | null;
}

export interface TextDetail {
  id: number;
  title: string;
  sentences: string[];
  reference: RefInfo | null;
  document: DocInfo | null;
}

export interface VoiceInfo {
  name: string;
  gender: "male" | "female" | null;
  quality: "compact" | "enhanced" | "premium";
}

export interface NeuralVoice {
  id: number;
  name: string;
}

export interface Bootstrap {
  texts: TextSummary[];
  documents: DocSummary[];
  voices: VoiceInfo[];
  voice: string;
  neural_voices: NeuralVoice[];
  neural_ready: boolean;
  coach: string;
  llm_supported: boolean;
  learner_hz: number | null;
  data_dir: string;
  session: number;
  speech: SpeechStatus;
  calibrated: boolean;
}

export type Point2 = [number, number];
export type VowelGrade = "good" | "fair" | "poor";

export type VowelStatus =
  | { state: "ready" }
  | { state: "needs_words" }
  | { state: "needs_calibration" }
  | { state: "needs_reference"; shared: string[]; missing: string[]; prepared: boolean };

export interface VowelComparison {
  word_index: number;
  word: string;
  set: string;
  keyword: string;
  target: boolean;
  reference: Point2[];
  you: Point2[];
  distance: number;
  grade: VowelGrade;
  detail: string;
  hint: string | null;
}

export interface RhoticComparison {
  word_index: number;
  word: string;
  reference_ratio: number;
  you_ratio: number;
  flagged: boolean;
  detail: string;
  hint: string | null;
}

export interface VowelReport {
  status: VowelStatus;
  vowels: VowelComparison[];
  rhotic: RhoticComparison[];
}

export interface Cloud {
  set: string;
  keyword: string;
  centre: Point2;
  spread: Point2;
  onset: Point2 | null;
  offset: Point2 | null;
  count: number;
}

export interface ChartWord {
  word_index: number;
  word: string;
  set: string;
  keyword: string;
  target: boolean;
  reference: Point2[];
  you: Point2[] | null;
  grade: VowelGrade | null;
  hint: string | null;
}

export interface VowelChartData {
  status: VowelStatus;
  reference: Cloud[];
  yours: Cloud[];
  words: ChartWord[];
  rhotic: RhoticComparison[];
  analysed: number;
  sentences: number;
}

export interface Coverage {
  have: [string, string, number][];
  missing: [string, string][];
  ready: boolean;
}

export interface CalibrationInfo {
  passage: string[];
  taken: number[];
  coverage: Coverage;
  calibrated: boolean;
  available: boolean;
}

export interface TakeResult {
  index: number;
  words: { word: string; keyword: string; measured: boolean }[];
  info: CalibrationInfo;
}

export type EngineState =
  | { state: "not_built" }
  | { state: "missing_model" }
  | { state: "ready" }
  | { state: "failed"; detail: string };

export interface SpeechStatus {
  words: EngineState;
  vad: EngineState;
  phonemes: EngineState;
}

export interface RefWord {
  index: number;
  display: string;
  ipa: string | null;
  playable: boolean;
}

export interface PhoneScore {
  phone: string;
  start_s: number;
  end_s: number;
  score: number;
}

export interface WordResult {
  index: number;
  display: string;
  score: number | null;
  grade: "good" | "fair" | "poor" | null;
  missed: boolean;
  phones: PhoneScore[];
  comparable: boolean;
}

export interface SentenceRef {
  index: number;
  text: string;
  duration_s: number;
  onset_s: number;
  contour: (number | null)[];
  intensity: number[];
  articulation_sps: number;
  final_rise: boolean;
  words: RefWord[];
  scoring: boolean;
}

export interface LiveFrame {
  t: number;
  f0_hz: number | null;
  st: number | null;
  db: number;
  speaking: boolean;
}

export interface LivePayload {
  frames: LiveFrame[];
  onset: number | null;
  rate_sps: number | null;
}

export interface PlaybackPayload {
  which: "reference" | "attempt" | "word";
  position_s: number;
  duration_s: number;
  done: boolean;
}

export interface RateReport {
  syllables: number;
  phonation_s: number;
  span_s: number;
  articulation_sps: number;
  speaking_sps: number;
  pause_count: number;
  mean_pause_s: number;
}

export interface OverlayPoint {
  t: number;
  reference_st: number | null;
  attempt_st: number | null;
}

export interface TempoPoint {
  t: number;
  ratio: number;
}

export interface Comparison {
  contour_corr: number | null;
  range_ratio: number;
  articulation_ratio: number;
  speaking_ratio: number;
  attempt_rate: RateReport;
  reference_rate: RateReport;
  final_movement_st: number | null;
  reference_final_movement_st: number | null;
  final_rise: boolean;
  reference_final_rise: boolean;
  overlay: OverlayPoint[];
  tempo: TempoPoint[];
  alignment_cost: number;
}

export interface Issue {
  word: string | null;
  feature: string;
  severity: number;
  detail: string;
}

export interface CoachReply {
  output: {
    spoken: string;
    focus: { word: string | null; feature: string }[];
    next: "repeat_sentence" | "next_sentence" | "drill_word";
  };
  source: "llm" | "rules";
  rejected_reason: string | null;
}

export interface PageSentence {
  index: number;
  /** Where the sentence was read in the whole recording, seconds; null when skipped. */
  span: [number, number] | null;
  /** Each word's span in the whole recording. */
  word_spans: ([number, number] | null)[];
  result: AttemptResult | null;
  /** Words when there is no result (skipped, or could not be assessed). */
  words: WordResult[];
  error: string | null;
}

export interface PageResult {
  duration_s: number;
  sentences: PageSentence[];
  heard: string;
}

export interface PageProgress {
  stage: string;
  done: number;
  total: number;
}

export interface AttemptResult {
  sentence: number;
  comparison: Comparison;
  issues: Issue[];
  coach: CoachReply;
  words: WordResult[];
  heard: string | null;
  vowels: VowelReport;
}

export type ModelStatus =
  | { state: "missing" }
  | { state: "partial"; bytes: number }
  | { state: "ready"; bytes: number; verified: boolean };

export interface ModelInfo {
  id: string;
  name: string;
  role: "asr" | "phonemes" | "vad" | "coach" | "tts";
  size_mb: number;
  milestone: string;
  status: ModelStatus;
  note: string;
}

export interface ModelProgress {
  id: string;
  downloaded: number;
  total: number | null;
  done: boolean;
  error: string | null;
}

export const api = {
  /** Starts Coach in RAD (its database and speech engines) the first time it is opened. */
  open: () => invoke<void>("coach_open"),
  bootstrap: () => invoke<Bootstrap>("coach_bootstrap"),
  createText: (title: string, body: string) => invoke<TextDetail>("coach_create_text", { title, body }),
  openText: (id: number) => invoke<TextDetail>("coach_open_text", { id }),
  deleteText: (id: number) => invoke<TextSummary[]>("coach_delete_text", { id }),
  importPdf: (path: string) => invoke<TextDetail>("coach_import_pdf", { path }),
  openDocument: (id: number) => invoke<TextDetail>("coach_open_document", { id }),
  deleteDocument: (id: number) => invoke<DocSummary[]>("coach_delete_document", { id }),
  attachNarration: (path: string) => invoke<TextDetail>("coach_attach_narration", { path }),
  useSystemVoice: (voice: string) => invoke<TextDetail>("coach_use_system_voice", { voice }),
  listVoices: () => invoke<VoiceInfo[]>("coach_list_voices"),
  previewVoice: (voice: string) => invoke<void>("coach_preview_voice", { voice }),
  openVoiceSettings: () => invoke<void>("coach_open_voice_settings"),
  addNeuralVoice: (name: string, path: string, consent: string) =>
    invoke<NeuralVoice[]>("coach_add_neural_voice", { name, path, consent }),
  deleteNeuralVoice: (id: number) => invoke<NeuralVoice[]>("coach_delete_neural_voice", { id }),
  useNeuralVoice: (id: number) => invoke<TextDetail>("coach_use_neural_voice", { id }),
  previewNeuralVoice: (id: number) => invoke<void>("coach_preview_neural_voice", { id }),
  loadSentence: (index: number) => invoke<SentenceRef>("coach_load_sentence", { index }),
  play: (which: "reference" | "attempt" | "page", sentence?: number) =>
    invoke<number>("coach_play", { which, sentence: sentence ?? null }),
  stopPlayback: () => invoke<void>("coach_stop_playback"),
  startRecording: (page = false) => invoke<string>("coach_start_recording", { page }),
  stopPageRecording: () => invoke<PageResult>("coach_stop_page_recording"),
  playPageReference: () =>
    invoke<{ duration_s: number; sentences: [number, number][] }>("coach_play_page_reference"),
  stopRecording: () => invoke<AttemptResult>("coach_stop_recording"),
  speak: (text: string) => invoke<void>("coach_speak", { text }),
  modelsStatus: () => invoke<ModelInfo[]>("coach_models_status"),
  downloadModel: (id: string) => invoke<void>("coach_download_model", { id }),
  loadCoach: (id: string) => invoke<string>("coach_load_coach", { id }),
  reloadSpeech: () => invoke<SpeechStatus>("coach_reload_speech"),
  playWord: (index: number, mode: "compare" | "reference", sentence?: number) =>
    invoke<number>("coach_play_word", { index, mode, sentence: sentence ?? null }),
  vowelChart: () => invoke<VowelChartData>("coach_vowel_chart"),
  prepareReference: () => invoke<VowelChartData>("coach_prepare_reference"),
  calibrationInfo: () => invoke<CalibrationInfo>("coach_calibration_info"),
  calibrationTake: (index: number) => invoke<TakeResult>("coach_calibration_take", { index }),
  calibrationSave: () => invoke<CalibrationInfo>("coach_calibration_save"),
  calibrationClear: () => invoke<CalibrationInfo>("coach_calibration_clear"),
};

/** Tauri rejects with the Rust error string. */
export function message(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}

/** Short names for coach issue features, as shown in the UI. */
export const issueNames: Record<string, string> = {
  PACE: "Pace",
  PAUSES: "Pauses",
  INTONATION: "Tune",
  FINAL_RISE: "Ending",
  PITCH_RANGE: "Pitch range",
  WORD_SOUND: "Word",
  MISSED_WORDS: "Skipped",
  FACE: "'day' vowel",
  PRICE: "'my' vowel",
  GOAT: "'no' vowel",
  MOUTH: "'now' vowel",
  GOOSE: "'who' vowel",
  NON_RHOTIC: "R",
};
