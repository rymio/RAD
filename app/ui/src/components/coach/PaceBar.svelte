<script lang="ts">
  // Speaking pace in syllables per second, with the reference's pace and a ±15% target zone.
  interface Props {
    reference: number | null;
    you: number | null;
  }
  let { reference, you }: Props = $props();

  const lo = 2;
  const hi = 8;
  const pos = (v: number) => `${((Math.min(hi, Math.max(lo, v)) - lo) / (hi - lo)) * 100}%`;

  let verdict = $derived.by(() => {
    if (reference === null || you === null || reference <= 0) return "";
    const r = you / reference;
    if (r < 0.85) return `${Math.round((1 - r) * 100)}% slower`;
    if (r > 1.15) return `${Math.round((r - 1) * 100)}% faster`;
    return "On pace";
  });
</script>

<div class="pace" role="group" aria-label="Speaking pace">
  <div class="head">
    <span>Pace</span>
    <span class="verdict" class:ok={verdict === "On pace"}>{verdict}</span>
  </div>
  <div class="track">
    {#if reference !== null}
      <div class="zone" style:left={pos(reference * 0.85)} style:right="calc(100% - {pos(reference * 1.15)})"></div>
      <div class="ref" style:left={pos(reference)} title="Reference pace"></div>
    {/if}
    {#if you !== null}
      <div class="you" style:left={pos(you)}></div>
    {/if}
  </div>
  <div class="scale">
    <span>slower</span>
    <span>
      {#if you !== null}{you.toFixed(1)}{:else}–{/if} syllables/s
      {#if reference !== null}<span class="refnum">(reference {reference.toFixed(1)})</span>{/if}
    </span>
    <span>faster</span>
  </div>
</div>

<style>
  .pace {
    min-width: 260px;
    flex: 1;
  }
  .head {
    display: flex;
    justify-content: space-between;
    font-weight: 600;
    margin-bottom: 6px;
  }
  .verdict {
    font-weight: 500;
    color: var(--you);
  }
  .verdict.ok {
    color: var(--close);
  }
  .track {
    position: relative;
    height: 12px;
    border-radius: 6px;
    background: var(--rule);
  }
  .zone {
    position: absolute;
    top: 0;
    bottom: 0;
    background: var(--reference-wash);
    border-radius: 6px;
  }
  .ref {
    position: absolute;
    top: -4px;
    bottom: -4px;
    width: 3px;
    margin-left: -1.5px;
    background: var(--reference);
    border-radius: 2px;
  }
  .you {
    position: absolute;
    top: 50%;
    width: 16px;
    height: 16px;
    margin: -8px 0 0 -8px;
    border-radius: 50%;
    background: var(--you);
    box-shadow: 0 0 0 3px var(--paper);
    transition: left 160ms ease-out;
  }
  .scale {
    display: flex;
    justify-content: space-between;
    font-size: var(--t-small);
    color: var(--ink-soft);
    margin-top: 5.25px;
  }
  .refnum {
    color: var(--reference);
  }
</style>
