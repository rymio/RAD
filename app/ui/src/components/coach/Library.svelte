<script lang="ts">
  import type { DocSummary, TextSummary } from "../../lib/coach/api";

  interface Props {
    texts: TextSummary[];
    documents: DocSummary[];
    activeId: number | null;
    activeDocId: number | null;
    onOpen: (id: number) => void;
    onOpenDocument: (id: number) => void;
    onNew: () => void;
    onImportPdf: () => void;
    onModels: () => void;
    coachName: string;
    calibrated: boolean;
    onCalibrate: () => void;
  }
  let {
    texts,
    documents,
    activeId,
    activeDocId,
    onOpen,
    onOpenDocument,
    onNew,
    onImportPdf,
    onModels,
    coachName,
    calibrated,
    onCalibrate,
  }: Props = $props();

  function when(ts: number | null): string {
    if (!ts) return "Not practised yet";
    const days = Math.floor((Date.now() / 1000 - ts) / 86400);
    if (days <= 0) return "Practised today";
    if (days === 1) return "Practised yesterday";
    return `Practised ${days} days ago`;
  }
</script>

<nav class="rail" aria-label="Your texts">
  <div class="brand">
    <svg viewBox="0 0 40 24" width="40" height="24" aria-hidden="true">
      <path d="M2 15 C8 4, 13 6, 18 12 S28 22, 38 4" fill="none" stroke="var(--reference-wash)" stroke-width="7" stroke-linecap="round" />
      <path d="M2 16 C8 6, 13 8, 18 13 S28 22, 38 5" fill="none" stroke="var(--you)" stroke-width="2.5" stroke-linecap="round" />
    </svg>
    <span>Aussie Accent Coach</span>
  </div>

  <div class="add">
    <button class="cbtn new" onclick={onNew}>New text</button>
    <button class="cbtn new" onclick={onImportPdf}>Open PDF…</button>
  </div>

  <ul>
    {#each documents as d (d.id)}
      <li>
        <button class="text" class:active={d.id === activeDocId} onclick={() => onOpenDocument(d.id)} aria-current={d.id === activeDocId}>
          <span class="title"><span class="doc" aria-hidden="true">📄</span> {d.title}</span>
          <span class="meta">PDF, {d.page_count} page{d.page_count === 1 ? "" : "s"}, {when(d.practised_at).toLowerCase()}</span>
        </button>
      </li>
    {/each}
    {#each texts as t (t.id)}
      <li>
        <button class="text" class:active={t.id === activeId} onclick={() => onOpen(t.id)} aria-current={t.id === activeId}>
          <span class="title">{t.title}</span>
          <span class="meta">{t.sentence_count} sentence{t.sentence_count === 1 ? "" : "s"}, {when(t.practised_at).toLowerCase()}</span>
        </button>
      </li>
    {/each}
  </ul>

  <button class="footer" onclick={onCalibrate}>
    <span>Your voice</span>
    <span class="meta">{calibrated ? "Calibrated. Read again to update" : "Not set up yet: needed for vowel feedback"}</span>
  </button>
  <button class="footer second" onclick={onModels}>
    <span>Models</span>
    <span class="meta">Coach: {coachName === "rules" ? "measured feedback" : coachName}</span>
  </button>
</nav>

<style>
  .rail {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--rail);
    padding: 14px 11.25px 11.25px;
    gap: 11.25px;
    border-right: 1px solid var(--rule);
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 8.25px;
    font-weight: 650;
    padding: 0 7.5px;
    -webkit-user-select: none;
    user-select: none;
  }
  .add {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px;
    margin: 0 3.75px;
  }
  .new {
    justify-content: center;
    white-space: nowrap;
    padding-inline: 8px;
  }
  .doc {
    font-size: 0.85em;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow-y: auto;
    flex: 1;
  }
  .text,
  .footer {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    width: 100%;
    text-align: left;
    background: none;
    border: 0;
    border-radius: 10px;
    padding: 8.25px 10.5px;
    gap: 0.75px;
  }
  .text:hover,
  .footer:hover {
    background: color-mix(in srgb, var(--ink) 6%, transparent);
  }
  .text.active {
    background: var(--paper-raised);
    box-shadow: inset 3px 0 0 var(--reference);
  }
  .title {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
  }
  .meta {
    font-size: var(--t-small);
    color: var(--ink-soft);
  }
  .footer {
    border-top: 1px solid var(--rule);
    border-radius: 0;
    padding-top: 11.25px;
  }
  .footer.second {
    border-top: 0;
    padding-top: 6px;
  }
</style>
