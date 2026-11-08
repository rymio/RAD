<script lang="ts">
  // The sentence to read, word by word. After an attempt each word carries its grade:
  // colour plus an underline style (solid, dashed, wavy) so colour is never the only cue.
  import type { RefWord, WordResult } from "../../lib/coach/api";

  interface Props {
    text: string;
    words: RefWord[];
    results: WordResult[] | null;
    dim: boolean;
    onPlay: (index: number, mode: "compare" | "reference") => void;
  }
  let { text, words, results, dim, onPlay }: Props = $props();

  function result(i: number): WordResult | undefined {
    return results?.find((r) => r.index === i);
  }

  function label(w: RefWord, r: WordResult | undefined): string {
    const parts = [w.display];
    if (r?.missed) parts.push("not heard");
    else if (r?.score != null) parts.push(`${Math.round(r.score * 100)}% like the reference`);
    if (w.ipa) parts.push(`reference /${w.ipa}/`);
    if (r?.comparable) parts.push("click to hear reference, yours, reference");
    else if (w.playable) parts.push("click to hear the reference");
    return parts.join(", ");
  }
</script>

<p class="sentence" class:dim>
  {#if words.length}
    {#each words as w (w.index)}
      {@const r = result(w.index)}
      <button
        class="word {r?.missed ? 'missed' : (r?.grade ?? '')}"
        class:playable={w.playable}
        disabled={!w.playable}
        title={label(w, r)}
        aria-label={label(w, r)}
        onclick={() => onPlay(w.index, r?.comparable ? "compare" : "reference")}
      >{w.display}</button>{" "}
    {/each}
  {:else}
    {text}
  {/if}
</p>

<style>
  .sentence {
    font-family: var(--serif);
    font-size: var(--t-sentence);
    font-variation-settings: "opsz" 72;
    line-height: 1.32;
    letter-spacing: -0.01em;
    margin: 0 0 22.5px;
    max-width: 24em;
    text-wrap: balance;
    transition: opacity 200ms;
  }
  .sentence.dim {
    opacity: 0.45;
  }
  .word {
    font: inherit;
    letter-spacing: inherit;
    color: inherit;
    background: none;
    border: 0;
    padding: 0;
    margin: 0;
    cursor: default;
    text-decoration-thickness: 0.07em;
    text-underline-offset: 0.18em;
    border-radius: 4px;
  }
  .word.playable {
    cursor: pointer;
  }
  .word.playable:hover {
    background: color-mix(in srgb, var(--reference) 12%, transparent);
  }
  .word:disabled {
    opacity: 1;
  }
  .good {
    text-decoration: underline solid var(--close);
  }
  .fair {
    text-decoration: underline dashed var(--you);
    color: color-mix(in srgb, var(--ink) 80%, var(--you));
  }
  .poor {
    text-decoration: underline wavy var(--off);
    text-decoration-thickness: 0.05em;
    color: var(--off);
  }
  .missed {
    text-decoration: line-through var(--off);
    opacity: 0.55;
  }
</style>
