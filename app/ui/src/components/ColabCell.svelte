<script lang="ts">
  // One cell of a Colab notebook: code (an editor with highlighting over a plain textarea, then
  // what it printed and showed) or markdown (rendered, with maths; double-click to edit).
  import hljs from 'highlight.js/lib/common';
  import { colab, type Cell, type Output } from '../lib/colab.svelte';
  import { renderMath, safeHtml, safeSvg, errorText, stripAnsi } from '../lib/colab/notebook';
  import { escapeHtml } from '../lib/markdown';
  import { Play, Square, ChevronUp, ChevronDown, Trash2, Type, Code, Loader, ScanFace, Lightbulb, Wrench, Plus } from '@lucide/svelte';

  let { id, n, onask }: { id: string; n: number; onask: (cell: Cell, what: 'explain' | 'error' | 'improve') => void } = $props();

  // The cell is the notebook's own (in the store), so editing it changes the notebook.
  const cell = $derived(colab.cell(id)!);

  const nb = $derived(colab.nb!);
  const lang = $derived(nb.lang);
  const selected = $derived(colab.selected === cell.id);
  const editing = $derived(cell.type === 'code' || colab.editing.has(cell.id));
  const running = $derived(cell.state === 'running');
  const queued = $derived(cell.state === 'queued');
  const error = $derived(cell.outputs.find((o): o is Extract<Output, { kind: 'error' }> => o.kind === 'error') ?? null);
  /** The lines the compiler or traceback points at. */
  const badLines = $derived(new Set<number>(error ? [...(error.errors?.map((e) => e.line).filter((l): l is number => !!l) ?? []), ...(error.line ? [error.line] : [])] : []));

  let ta = $state<HTMLTextAreaElement | null>(null);

  const highlighted = $derived.by(() => {
    if (cell.type !== 'code') return escapeHtml(cell.source);
    try {
      // evcxr's `:dep` lines aren't Rust: shown as comments.
      return hljs.highlight(cell.source, { language: lang === 'rust' ? 'rust' : 'python', ignoreIllegals: true }).value;
    } catch {
      return escapeHtml(cell.source);
    }
  });

  const INDENT = '    ';

  function onKey(e: KeyboardEvent) {
    const t = e.currentTarget as HTMLTextAreaElement;
    if (e.key === 'Enter' && (e.shiftKey || e.metaKey || e.ctrlKey || e.altKey)) {
      e.preventDefault();
      colab.run(cell.id);
      if (e.shiftKey || e.altKey) {
        // Shift: on to the next cell (a new one at the end); Alt: always a new one.
        const i = colab.index(cell.id);
        const next = !e.altKey ? nb.cells[i + 1] : null;
        if (next) colab.selected = next.id;
        else colab.addCell('code', '', cell.id);
        focusSelected();
      }
      return;
    }
    if (e.key === 'Escape') {
      t.blur();
      if (cell.type === 'markdown') colab.setEditing(cell.id, false);
      return;
    }
    if (e.key === 'Tab') {
      e.preventDefault();
      const { selectionStart: a, selectionEnd: b, value } = t;
      const lineStart = value.lastIndexOf('\n', a - 1) + 1;
      if (a !== b || e.shiftKey) {
        // Indent (or dedent) every selected line.
        const end = b;
        const block = value.slice(lineStart, end);
        const changed = e.shiftKey ? block.replace(/^( {1,4}|\t)/gm, '') : block.replace(/^/gm, INDENT);
        set(t, value.slice(0, lineStart) + changed + value.slice(end), lineStart, lineStart + changed.length);
      } else set(t, value.slice(0, a) + INDENT + value.slice(b), a + INDENT.length);
      return;
    }
    if (e.key === 'Enter' && cell.type === 'code') {
      // Keep the indent, one more after a ":" (Python) or an opening bracket.
      e.preventDefault();
      const { selectionStart: a, selectionEnd: b, value } = t;
      const line = value.slice(value.lastIndexOf('\n', a - 1) + 1, a);
      let indent = line.match(/^\s*/)![0];
      if (/[:{([]\s*$/.test(line) && !(lang === 'rust' && line.trimEnd().endsWith(':'))) indent += INDENT;
      set(t, value.slice(0, a) + '\n' + indent + value.slice(b), a + 1 + indent.length);
    }
  }

  function set(t: HTMLTextAreaElement, value: string, a: number, b = a) {
    cell.source = value;
    colab.touch();
    queueMicrotask(() => t.setSelectionRange(a, b));
  }

  function focusSelected() {
    queueMicrotask(() => (document.querySelector(`[data-cell="${colab.selected}"] textarea`) as HTMLTextAreaElement | null)?.focus());
  }

  $effect(() => {
    // A cell selected from the keyboard (or a new one) gets the caret.
    if (selected && editing && ta && document.activeElement?.tagName !== 'TEXTAREA') ta.focus({ preventScroll: false });
  });

  function image(o: Extract<Output, { kind: 'data' }>) {
    return o.data['image/png'] ? `data:image/png;base64,${o.data['image/png'].replace(/\s/g, '')}` : o.data['image/jpeg'] ? `data:image/jpeg;base64,${o.data['image/jpeg'].replace(/\s/g, '')}` : null;
  }

  const lineH = 19.5;
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="cell" class:sel={selected} class:md={cell.type === 'markdown'} class:err={cell.state === 'error'} data-cell={cell.id} onclick={() => (colab.selected = cell.id)}>
  <div class="gutter">
    {#if cell.type === 'code'}
      {#if running}
        <button class="run on" title="Stop (interrupt)" onclick={(e) => { e.stopPropagation(); colab.interrupt(); }}><Square size={11} /></button>
      {:else}
        <button class="run" title="Run (⇧↩)" disabled={queued} onclick={(e) => { e.stopPropagation(); colab.run(cell.id); }}>{#if queued}<Loader size={12} class="spin" />{:else}<Play size={12} />{/if}</button>
      {/if}
      <span class="count">[{running ? '*' : (cell.count ?? ' ')}]</span>
    {/if}
  </div>

  <div class="body">
    {#if editing}
      <div class="ed" class:code={cell.type === 'code'} style:--lh="{lineH}px">
        {#each [...badLines] as l (l)}<div class="badline" style:top="{(l - 1) * lineH + 8}px"></div>{/each}
        <pre class="hl" aria-hidden="true"><code class="hljs">{@html highlighted}{'\n'}</code></pre>
        <textarea
          bind:this={ta}
          value={cell.source}
          spellcheck="false"
          autocapitalize="off"
          autocomplete="off"
          placeholder={cell.type === 'markdown' ? 'Text, with **markdown** and maths: $\\sum_i x_i^2$' : lang === 'rust' ? 'Rust code… (⇧↩ runs it)' : 'Python code… (⇧↩ runs it)'}
          oninput={(e) => { cell.source = e.currentTarget.value; colab.touch(); }}
          onkeydown={onKey}
          onfocus={() => (colab.selected = cell.id)}
          onblur={() => { if (cell.type === 'markdown' && cell.source.trim()) colab.setEditing(cell.id, false); }}
        ></textarea>
      </div>
    {:else}
      <div class="rendered md selectable" ondblclick={() => colab.setEditing(cell.id, true)} title="Double-click to edit">{@html renderMath(cell.source)}</div>
    {/if}

    {#if cell.type === 'code' && (cell.outputs.length || running)}
      <div class="outputs">
        {#each cell.outputs as o, i (i)}
          {#if o.kind === 'stream'}
            <pre class="out" class:stderr={o.name === 'stderr'}>{stripAnsi(o.text)}</pre>
          {:else if o.kind === 'build'}
            <details class="build" open={running && i === cell.outputs.length - 1}>
              <summary>{running && i === cell.outputs.length - 1 ? 'Building' : 'Built'}: {o.text.trim().split('\n').at(-1)?.trim()}</summary>
              <pre class="out">{o.text}</pre>
            </details>
          {:else if o.kind === 'data'}
            {@const img = image(o)}
            {#if img}
              <img class="pic" src={img} alt="Output" />
            {:else if o.data['image/svg+xml']}
              <div class="svg">{@html safeSvg(o.data['image/svg+xml'])}</div>
            {:else if o.data['text/html']}
              <div class="html">{@html safeHtml(o.data['text/html'])}</div>
            {:else if o.data['text/markdown'] || o.data['text/latex']}
              <div class="md rendered">{@html renderMath(o.data['text/markdown'] ?? o.data['text/latex'])}</div>
            {:else if o.data['text/plain'] !== undefined}
              <pre class="out" class:value={o.result}>{o.data['text/plain']}</pre>
            {/if}
          {:else if o.kind === 'error'}
            <div class="error">
              <div class="etitle">{o.errKind === 'compile' ? `Didn’t compile (${o.errors?.length ?? 0} error${o.errors?.length === 1 ? '' : 's'})` : o.errKind === 'interrupted' ? 'Stopped' : o.ename ? `${o.ename}: ${o.message}` : o.message}</div>
              {#if o.errKind !== 'interrupted' || o.errors?.length}
                <pre class="out">{errorText(o)}</pre>
              {:else}
                <p class="note">{o.message}</p>
              {/if}
              {#if o.errKind !== 'interrupted'}
                <div class="eact">
                  <button class="chip" onclick={(e) => { e.stopPropagation(); onask(cell, 'error'); }}><ScanFace size={12} /> Ask Gilgamesh why</button>
                </div>
              {/if}
            </div>
          {/if}
        {/each}
        {#if running && !cell.outputs.length}<div class="wait"><Loader size={12} class="spin" /> {colab.kernel === 'starting' ? (lang === 'rust' ? 'Starting Rust (the first cell builds for a few seconds)…' : 'Starting Python…') : 'Running…'}</div>{/if}
      </div>
      {#if cell.ms != null && !running}<div class="time">{cell.ms < 1000 ? `${cell.ms} ms` : `${(cell.ms / 1000).toFixed(1)} s`}</div>{/if}
    {/if}
  </div>

  {#if selected}
    <div class="tools">
      <button class="icon-btn sm" title="Ask Gilgamesh about this cell" onclick={(e) => { e.stopPropagation(); onask(cell, 'explain'); }}><Lightbulb size={13} /></button>
      {#if cell.type === 'code'}<button class="icon-btn sm" title="Ask Gilgamesh to improve it" onclick={(e) => { e.stopPropagation(); onask(cell, 'improve'); }}><Wrench size={13} /></button>{/if}
      <button class="icon-btn sm" title={cell.type === 'code' ? 'Make it a text cell' : 'Make it a code cell'} onclick={(e) => { e.stopPropagation(); colab.setType(cell.id, cell.type === 'code' ? 'markdown' : 'code'); }}>{#if cell.type === 'code'}<Type size={13} />{:else}<Code size={13} />{/if}</button>
      <button class="icon-btn sm" title="Move up" disabled={n === 0} onclick={(e) => { e.stopPropagation(); colab.moveCell(cell.id, -1); }}><ChevronUp size={13} /></button>
      <button class="icon-btn sm" title="Move down" disabled={n === nb.cells.length - 1} onclick={(e) => { e.stopPropagation(); colab.moveCell(cell.id, 1); }}><ChevronDown size={13} /></button>
      <button class="icon-btn sm" title="Add a cell below" onclick={(e) => { e.stopPropagation(); colab.addCell('code', '', cell.id); focusSelected(); }}><Plus size={13} /></button>
      <button class="icon-btn sm" title="Delete the cell" onclick={(e) => { e.stopPropagation(); colab.deleteCell(cell.id); }}><Trash2 size={13} /></button>
    </div>
  {/if}
</div>

<style>
  .cell {
    position: relative;
    display: flex;
    gap: 6px;
    padding: 6px 8px 6px 4px;
    border-radius: var(--r);
    border: 1px solid transparent;
  }
  .cell.sel {
    border-color: var(--border-strong);
    background: var(--surface);
    box-shadow: var(--shadow-sm);
  }
  .gutter {
    width: 44px;
    flex: none;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    padding-top: 6px;
  }
  .run {
    width: 26px;
    height: 26px;
    border-radius: 50%;
    border: 1px solid var(--border-strong);
    background: var(--surface-2);
    color: var(--text-2);
    display: grid;
    place-items: center;
    cursor: pointer;
  }
  .run:hover:not(:disabled) {
    background: var(--accent);
    color: var(--on-accent);
    border-color: transparent;
  }
  .run.on {
    background: var(--danger);
    color: #fff;
    border-color: transparent;
  }
  .count {
    font: 11px var(--mono);
    color: var(--text-3);
    white-space: pre;
  }
  .body {
    flex: 1;
    min-width: 0;
  }
  .ed {
    position: relative;
    border-radius: var(--r-sm);
    background: var(--code-bg);
    border: 1px solid var(--border);
  }
  .ed:focus-within {
    border-color: var(--accent-ring);
  }
  .md .ed {
    background: var(--surface);
  }
  .hl,
  textarea {
    margin: 0;
    padding: 8px 10px;
    font: 13px/var(--lh) var(--mono);
    tab-size: 4;
    white-space: pre-wrap;
    word-break: break-word;
    overflow-wrap: anywhere;
  }
  .hl {
    pointer-events: none;
    min-height: calc(var(--lh) + 16px);
    background: transparent;
  }
  .hl code {
    font: inherit;
    padding: 0;
    background: transparent;
  }
  textarea {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    resize: none;
    border: 0;
    outline: none;
    background: transparent;
    color: transparent;
    caret-color: var(--text);
    overflow: hidden;
  }
  .ed:not(.code) .hl {
    color: var(--text);
    font-family: var(--font);
    font-size: 13.5px;
  }
  .ed:not(.code) textarea {
    font-family: var(--font);
    font-size: 13.5px;
  }
  textarea::placeholder {
    color: var(--text-4);
  }
  textarea::selection {
    background: var(--accent-ring);
    color: transparent;
  }
  .badline {
    position: absolute;
    left: 0;
    right: 0;
    height: var(--lh);
    background: var(--danger-soft);
    border-left: 2px solid var(--danger);
    pointer-events: none;
  }
  .rendered {
    padding: 4px 10px;
    min-height: 24px;
    cursor: text;
  }
  .outputs {
    margin-top: 6px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 640px;
    overflow: auto;
  }
  .out {
    margin: 0;
    padding: 2px 10px;
    font: 12.5px/1.5 var(--mono);
    white-space: pre-wrap;
    word-break: break-word;
    color: var(--text);
    user-select: text;
    -webkit-user-select: text;
  }
  .out.stderr {
    color: var(--warning);
  }
  .out.value {
    color: var(--text-2);
  }
  .pic {
    max-width: 100%;
    align-self: flex-start;
    border-radius: var(--r-sm);
    background: #fff;
  }
  .svg,
  .html {
    max-width: 100%;
    overflow: auto;
    padding: 2px 10px;
    font-size: 12.5px;
  }
  .svg :global(svg),
  .html :global(svg) {
    max-width: 100%;
    height: auto;
    background: #fff;
    border-radius: var(--r-sm);
  }
  .html :global(table) {
    border-collapse: collapse;
    font: 12px var(--mono);
  }
  .html :global(th),
  .html :global(td) {
    border: 1px solid var(--border-strong);
    padding: 3px 8px;
    text-align: right;
  }
  .html :global(thead th) {
    background: var(--surface-2);
  }
  .build summary {
    cursor: pointer;
    font: 11.5px var(--mono);
    color: var(--text-3);
    padding: 2px 10px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .build .out {
    color: var(--text-3);
    font-size: 11.5px;
    max-height: 160px;
    overflow: auto;
  }
  .error {
    border-left: 3px solid var(--danger);
    background: var(--danger-soft);
    border-radius: 0 var(--r-sm) var(--r-sm) 0;
    padding: 6px 0 8px;
  }
  .etitle {
    padding: 0 10px 4px;
    font-weight: 600;
    font-size: 13px;
    color: var(--danger);
  }
  .error .out {
    font-size: 12px;
  }
  .note {
    margin: 0;
    padding: 0 10px;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .eact {
    padding: 6px 10px 0;
  }
  .wait {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 10px;
    font-size: 12px;
    color: var(--text-3);
  }
  .time {
    text-align: right;
    font: 10.5px var(--mono);
    color: var(--text-4);
    padding-right: 4px;
  }
  .tools {
    position: absolute;
    top: -13px;
    right: 10px;
    display: flex;
    gap: 1px;
    padding: 2px;
    border: 1px solid var(--border-strong);
    border-radius: var(--r-sm);
    background: var(--surface);
    box-shadow: var(--shadow-sm);
  }
</style>
