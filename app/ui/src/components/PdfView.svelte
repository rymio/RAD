<script lang="ts">
  import { onMount } from 'svelte';
  import * as pdfjs from 'pdfjs-dist';
  import workerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url';
  import { Loader, Minus, Plus } from '@lucide/svelte';

  pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

  let { url, page = 1 }: { url: string; page?: number } = $props();
  let container: HTMLDivElement | undefined = $state();
  let loading = $state(true);
  let error = $state('');
  let pages = $state(0);
  let scale = $state(1.2);
  let doc: pdfjs.PDFDocumentProxy | null = null;

  async function render() {
    if (!doc || !container) return;
    container.innerHTML = '';
    const ratio = window.devicePixelRatio || 1;
    const width = container.clientWidth - 24;
    for (let i = 1; i <= doc.numPages; i++) {
      const p = await doc.getPage(i);
      const base = p.getViewport({ scale: 1 });
      const vp = p.getViewport({ scale: (width / base.width) * (scale / 1.2) });
      const wrap = document.createElement('div');
      wrap.className = 'pdf-page';
      wrap.dataset.page = String(i);
      const canvas = document.createElement('canvas');
      canvas.width = vp.width * ratio;
      canvas.height = vp.height * ratio;
      canvas.style.width = `${vp.width}px`;
      canvas.style.height = `${vp.height}px`;
      wrap.appendChild(canvas);
      const label = document.createElement('div');
      label.className = 'pdf-num';
      label.textContent = `${i}`;
      wrap.appendChild(label);
      container.appendChild(wrap);
      const ctx = canvas.getContext('2d')!;
      ctx.scale(ratio, ratio);
      await p.render({ canvasContext: ctx, viewport: vp, canvas } as any).promise;
      if (i === page) wrap.scrollIntoView({ block: 'start' });
    }
  }

  onMount(() => {
    (async () => {
      try {
        const data = await (await fetch(url)).arrayBuffer();
        doc = await pdfjs.getDocument({ data }).promise;
        pages = doc.numPages;
        loading = false;
        await render();
        container?.querySelector(`[data-page="${page}"]`)?.scrollIntoView({ block: 'start' });
      } catch (e: any) {
        error = e?.message ?? String(e);
        loading = false;
      }
    })();
    return () => {
      (doc as any)?.destroy?.();
    };
  });

  function zoom(d: number) {
    scale = Math.min(3, Math.max(0.6, scale + d));
    render();
  }
</script>

<div class="pdf">
  <div class="bar">
    <span class="muted">{pages ? `${pages} page${pages > 1 ? 's' : ''}` : ''}{page > 1 ? ` · opened at p. ${page}` : ''}</span>
    <span class="spacer"></span>
    <button class="icon-btn sm" onclick={() => zoom(-0.2)} aria-label="Zoom out"><Minus size={14} /></button>
    <button class="icon-btn sm" onclick={() => zoom(0.2)} aria-label="Zoom in"><Plus size={14} /></button>
  </div>
  {#if loading}<div class="state"><Loader size={18} class="spin" /> Decrypting & rendering…</div>{/if}
  {#if error}<div class="state err">{error}</div>{/if}
  <div class="pages" bind:this={container}></div>
</div>

<style>
  .pdf {
    display: flex;
    flex-direction: column;
    min-height: 0;
    flex: 1;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 6px 10px;
    font-size: 12px;
    border-bottom: 1px solid var(--border);
  }
  .pages {
    flex: 1;
    overflow: auto;
    padding: 12px;
    background: var(--surface-3);
  }
  .pages :global(.pdf-page) {
    position: relative;
    margin: 0 auto 12px;
    width: fit-content;
    box-shadow: var(--shadow);
    border-radius: 4px;
    overflow: hidden;
    background: white;
  }
  .pages :global(.pdf-num) {
    position: absolute;
    right: 6px;
    bottom: 6px;
    font-size: 10px;
    padding: 1px 6px;
    border-radius: 6px;
    background: rgba(0, 0, 0, 0.55);
    color: white;
  }
  .state {
    display: flex;
    gap: 8px;
    align-items: center;
    justify-content: center;
    padding: 30px;
    color: var(--text-3);
  }
  .err {
    color: var(--danger);
  }
</style>
