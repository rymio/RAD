<script lang="ts">
  // The book in front of the owner. PDFs: one page at a time (pdf.js), with a text layer so words
  // can be selected. Every other format: its chapters as sanitised HTML, laid out in CSS columns
  // and turned a page (one or two columns) at a time.
  //
  // It tells books.svelte.ts what's on the page and hands out the text to read aloud; while
  // reading, the paragraph being read is marked and the pages turn by themselves.
  import { onMount, untrack } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import DOMPurify from 'dompurify';
  import * as pdfjs from 'pdfjs-dist';
  import workerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url';
  import { store } from '../lib/state.svelte';
  import { books, sentences, type Book, type Piece, type Reader } from '../lib/books.svelte';
  import { ChevronLeft, ChevronRight, Loader } from '@lucide/svelte';

  pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

  let { book, tocOpen = $bindable(false) }: { book: Book; tocOpen?: boolean } = $props();

  const isPdf = book.format === 'pdf';
  let stage = $state<HTMLDivElement | null>(null);
  let flow = $state<HTMLDivElement | null>(null);
  let pdfBox = $state<HTMLDivElement | null>(null);
  let loading = $state(true);
  let error = $state('');

  // ---- where the owner is
  /** Reflowable: the chapter shown; PDF: always 0. */
  let ch = $state(0);
  /** Page within the chapter (reflowable) or the PDF page, from 0. */
  let pg = $state(0);
  let pages = $state(1);
  /** Where the reading aloud is (the last sentence shown). */
  let mark: { ch: number; b: number; s: number } | null = null;

  // ---------------------------------------------------------------- reflowable books

  interface Chap {
    path: string;
    title: string;
    html: string;
    /** The text of each block (paragraph, heading, list item…), by data-b. */
    blocks: string[];
  }
  interface Content {
    chapters: { path: string; html: string }[];
    toc: { title: string; href: string; depth: number }[];
    resources: Record<string, string>;
  }
  let content: Content | null = null;
  let chaps = $state.raw<(Chap | null)[]>([]);
  let toc = $state<{ title: string; ch: number; anchor: string | null; depth: number }[]>([]);
  const BLOCKS = 'p,h1,h2,h3,h4,h5,h6,li,blockquote,pre,dd,dt,td,th,figcaption,caption,div,section,article,aside,address';
  /** Anchors (ids) → chapter, for links across chapters (MOBI's filepos anchors). */
  const anchorChapter = new Map<string, number>();

  const dirOf = (p: string) => (p.includes('/') ? p.slice(0, p.lastIndexOf('/') + 1) : '');
  function resolve(dir: string, href: string): string {
    const parts = href.startsWith('/') ? [] : dir.split('/').filter(Boolean);
    for (const x of decodeURIComponent(href).split('/')) {
      if (x === '..') parts.pop();
      else if (x && x !== '.') parts.push(x);
    }
    return parts.join('/');
  }

  /** A chapter, sanitised and split into blocks (made the first time it's needed). */
  function prep(i: number): Chap {
    const have = chaps[i];
    if (have) return have;
    const c = content!.chapters[i];
    const dir = dirOf(c.path);
    const doc = new DOMParser().parseFromString(c.html, 'text/html');
    const res = (src: string) => content!.resources[src] ?? content!.resources[resolve(dir, src)] ?? '';
    // An SVG that only wraps a picture (a common cover) becomes the picture.
    for (const svg of [...doc.querySelectorAll('svg')]) {
      const im = svg.querySelector('image');
      const href = im?.getAttribute('xlink:href') ?? im?.getAttribute('href');
      if (href) {
        const img = doc.createElement('img');
        img.setAttribute('src', href);
        svg.replaceWith(img);
      }
    }
    for (const img of doc.querySelectorAll('img')) {
      const src = img.getAttribute('src') ?? '';
      const url = res(src);
      if (url) img.setAttribute('src', url);
      else img.remove();
    }
    for (const a of doc.querySelectorAll('a[href]')) {
      const h = a.getAttribute('href') ?? '';
      if (/^(https?|mailto):/i.test(h)) a.setAttribute('data-link', h);
      else if (h.startsWith('#')) a.setAttribute('data-link', h);
      else a.setAttribute('data-link', h.includes('#') ? `${resolve(dir, h.split('#')[0])}#${h.split('#')[1]}` : resolve(dir, h));
      a.setAttribute('href', '#');
    }
    for (const el of doc.querySelectorAll('[id]')) {
      el.setAttribute('data-anchor', el.id);
      anchorChapter.set(el.id, i);
    }
    const clean = DOMPurify.sanitize(doc.body?.innerHTML ?? '', {
      FORBID_TAGS: ['style', 'link', 'meta', 'form', 'input', 'button', 'textarea', 'select', 'iframe', 'object', 'embed', 'video', 'audio', 'script', 'mbp:pagebreak'],
      FORBID_ATTR: ['style', 'class', 'id', 'width', 'height', 'align', 'bgcolor', 'color', 'face', 'size', 'background'],
    });
    const t = document.createElement('template');
    t.innerHTML = clean;
    const blocks: string[] = [];
    for (const el of t.content.querySelectorAll<HTMLElement>(BLOCKS)) {
      if (el.querySelector(BLOCKS)) continue;
      const text = (el.textContent ?? '').replace(/\s+/g, ' ').trim();
      if (!text) continue;
      el.dataset.b = String(blocks.length);
      blocks.push(text);
    }
    const heading = t.content.querySelector('h1,h2,h3')?.textContent?.replace(/\s+/g, ' ').trim();
    const fromToc = content!.toc.find((e) => e.href.split('#')[0] === c.path)?.title;
    // No heading tag (MOBI often has none): a short first line reads like one.
    const firstLine = blocks[0] && blocks[0].length <= 80 ? blocks[0] : '';
    const cover = !blocks.length && t.content.querySelector('img') ? 'Cover' : '';
    const chap = { path: c.path, title: fromToc || heading?.slice(0, 80) || firstLine || cover || `Part ${i + 1}`, html: t.innerHTML, blocks };
    chaps[i] = chap;
    return chap;
  }

  // ---- layout: columns as wide as a page (two side by side when the window is wide)
  const GAP = 72;
  let step = 1;
  let cols = $state(1);

  function measure() {
    if (!flow || !stage) return;
    const W = stage.clientWidth - 2 * padX();
    // The page window's padding (28 px top and bottom) and the footer (28 px).
    const H = stage.clientHeight - 84;
    cols = books.settings.spread && W > 1000 ? 2 : 1;
    const colW = (W - (cols - 1) * GAP) / cols;
    flow.style.width = `${W}px`;
    flow.style.height = `${H}px`;
    flow.style.columnWidth = `${colW}px`;
    flow.style.columnGap = `${GAP}px`;
    flow.style.setProperty('--img-h', `${H - 20}px`);
    step = W + GAP;
    let right = 0;
    for (const el of [...flow.querySelectorAll<HTMLElement>('[data-b], img')].slice(-3)) for (const r of el.getClientRects()) right = Math.max(right, xOf(r) + r.width);
    pages = Math.max(1, Math.ceil((right - 1) / step));
  }
  /** Where a box is in the column flow, from its start (the same whichever page is shown, and
   *  while the page is still sliding). */
  const xOf = (r: DOMRect) => r.left - flow!.getBoundingClientRect().left;
  const padX = () => (stage && stage.clientWidth > 700 ? 64 : 28);

  /** The page a block (or a sentence of it) is on. */
  function pageOf(el: Element, part = 0): number {
    const rs = [...el.getClientRects()];
    if (!rs.length || !flow) return pg;
    const r = rs[Math.min(rs.length - 1, Math.floor(part * rs.length))];
    return Math.max(0, Math.floor((xOf(r) + 2) / step));
  }
  const blockEl = (b: number) => flow?.querySelector(`[data-b="${b}"]`) ?? null;

  function place() {
    if (flow) flow.style.transform = `translateX(${-pg * step}px)`;
  }

  async function showChapter(i: number, at: 'start' | 'end' | { block: number } | { anchor: string } = 'start') {
    if (!flow || !content) return;
    i = Math.max(0, Math.min(content.chapters.length - 1, i));
    const c = prep(i);
    if (i !== ch || !flow.childElementCount) {
      ch = i;
      flow.innerHTML = c.html;
      pg = 0;
      place();
      await Promise.all([...flow.querySelectorAll('img')].map((im) => im.decode().catch(() => {})));
    }
    measure();
    if (at === 'end') pg = pages - 1;
    else if (at === 'start') pg = 0;
    else if ('block' in at) {
      const el = blockEl(at.block);
      pg = el ? pageOf(el) : 0;
    } else {
      const el = flow.querySelector(`[data-anchor="${CSS.escape(at.anchor)}"]`);
      pg = el ? pageOf(el) : 0;
    }
    pg = Math.min(pg, pages - 1);
    place();
    saved();
  }

  /** The blocks on the page now. */
  function blocksOnPage(): number[] {
    if (!flow) return [];
    const out: number[] = [];
    const [from, to] = [pg * step, pg * step + step - GAP];
    for (const el of flow.querySelectorAll<HTMLElement>('[data-b]')) {
      if ([...el.getClientRects()].some((r) => xOf(r) + r.width > from + 2 && xOf(r) < to)) out.push(Number(el.dataset.b));
    }
    return out;
  }

  function textOfPage(): string {
    if (!flow) return '';
    const c = chaps[ch];
    return blocksOnPage().map((b) => c?.blocks[b] ?? '').join('\n\n');
  }

  // ---------------------------------------------------------------- PDF

  let pdf: pdfjs.PDFDocumentProxy | null = null;
  let zoom = $state(1);
  const pdfText = new Map<number, string>();
  let rendering: Promise<void> = Promise.resolve();

  async function pageText(n: number): Promise<string> {
    const have = pdfText.get(n);
    if (have !== undefined) return have;
    const p = await pdf!.getPage(n + 1);
    const tc = await p.getTextContent();
    const text = tc.items
      .map((it: any) => (it.str ?? '') + (it.hasEOL ? '\n' : ''))
      .join('')
      // Words broken at line ends join again; lines join into sentences.
      .replace(/(\w)-\n(\w)/g, '$1$2')
      .replace(/\s*\n\s*/g, ' ')
      .replace(/\s+/g, ' ')
      .trim();
    pdfText.set(n, text);
    return text;
  }

  function renderPdf() {
    rendering = rendering.then(async () => {
      if (!pdf || !pdfBox || !stage) return;
      const p = await pdf.getPage(pg + 1);
      const base = p.getViewport({ scale: 1 });
      const W = stage.clientWidth - 2 * padX();
      const H = stage.clientHeight - 40;
      const scale = Math.min(W / base.width, H / base.height) * zoom;
      const vp = p.getViewport({ scale });
      const ratio = window.devicePixelRatio || 1;
      const canvas = document.createElement('canvas');
      canvas.width = Math.floor(vp.width * ratio);
      canvas.height = Math.floor(vp.height * ratio);
      canvas.style.width = `${Math.floor(vp.width)}px`;
      canvas.style.height = `${Math.floor(vp.height)}px`;
      const ctx = canvas.getContext('2d')!;
      ctx.scale(ratio, ratio);
      await p.render({ canvasContext: ctx, viewport: vp, canvas } as any).promise;
      const layer = document.createElement('div');
      layer.className = 'textLayer';
      const page = document.createElement('div');
      page.className = 'pdf-page';
      page.style.setProperty('--total-scale-factor', String(scale));
      page.style.setProperty('--scale-round-x', '1px');
      page.style.setProperty('--scale-round-y', '1px');
      page.style.width = canvas.style.width;
      page.style.height = canvas.style.height;
      page.append(canvas, layer);
      pdfBox.replaceChildren(page);
      // The page's text (and the one before) are ready for Gilgamesh when he's asked about it.
      await Promise.all([pageText(pg), pg > 0 ? pageText(pg - 1) : null]).catch(() => {});
      try {
        await new pdfjs.TextLayer({ textContentSource: await p.getTextContent(), container: layer, viewport: vp }).render();
      } catch {
        // Just no selecting on this page.
      }
    });
    return rendering;
  }

  // ---------------------------------------------------------------- turning pages

  function goto(next: number) {
    if (isPdf) {
      pg = Math.max(0, Math.min(pages - 1, next));
      void renderPdf();
      saved();
      return;
    }
    if (next >= pages) {
      if (content && ch < content.chapters.length - 1) void showChapter(ch + 1, 'start');
      return;
    }
    if (next < 0) {
      if (ch > 0) void showChapter(ch - 1, 'end');
      return;
    }
    pg = next;
    place();
    saved();
  }

  function turn(by: 1 | -1) {
    goto(pg + by);
  }

  function saved() {
    if (isPdf) {
      books.saveProgress(book.id, { chapter: 0, block: 0, page: pg, percent: pages > 1 ? pg / (pages - 1) : 0 });
      return;
    }
    const n = content?.chapters.length ?? 1;
    const first = blocksOnPage()[0] ?? 0;
    books.saveProgress(book.id, { chapter: ch, block: first, page: pg, percent: Math.min(1, (ch + (pages > 1 ? pg / pages : 0)) / n) });
  }

  // ---------------------------------------------------------------- reading aloud

  function marked(c: number, b: number) {
    flow?.querySelector('.reading')?.classList.remove('reading');
    if (c === ch) blockEl(b)?.classList.add('reading');
  }

  /** A sentence is being read: mark it, and turn to its page if the reading has moved on. */
  async function showPiece(c: number, b: number, s: number, of: number) {
    mark = { ch: c, b, s };
    if (isPdf) {
      if (c !== pg) goto(c);
      return;
    }
    if (c !== ch) await showChapter(c, { block: b });
    marked(c, b);
    const el = blockEl(b);
    if (!el) return;
    const p = pageOf(el, of > 1 ? s / of : 0);
    if (p !== pg) goto(p);
  }

  async function* pieces(fromTop: boolean): AsyncGenerator<Piece> {
    if (isPdf) {
      const startS = !fromTop && mark && mark.ch === pg ? mark.s : 0;
      for (let p = pg; p < pages; p++) {
        const ss = sentences(await pageText(p));
        for (let s = p === pg ? startS : 0; s < ss.length; s++) yield { text: ss[s], show: () => void showPiece(p, 0, s, ss.length) };
      }
      return;
    }
    if (!content) return;
    // From where the reading stopped if that's on this page, else from the first paragraph that
    // starts on it (or the one that runs across it).
    let start = { ch, b: 0, s: 0 };
    const on = blocksOnPage();
    const mEl = mark && mark.ch === ch ? blockEl(mark.b) : null;
    if (!fromTop && mark && mEl && on.includes(mark.b)) start = { ...mark };
    else if (on.length) {
      const begins = on.find((b) => {
        const el = blockEl(b);
        return el && pageOf(el) === pg;
      });
      if (begins !== undefined) start = { ch, b: begins, s: 0 };
      else {
        // The page is inside one long paragraph: start about where the page begins in it.
        const el = blockEl(on[0])!;
        const rs = [...el.getClientRects()];
        const i = rs.findIndex((r) => xOf(r) + r.width > pg * step + 2);
        const n = sentences(chaps[ch]!.blocks[on[0]]).length;
        start = { ch, b: on[0], s: rs.length ? Math.floor((Math.max(0, i) / rs.length) * n) : 0 };
      }
    }
    for (let c = start.ch; c < content.chapters.length; c++) {
      const C = prep(c);
      for (let b = c === start.ch ? start.b : 0; b < C.blocks.length; b++) {
        const ss = sentences(C.blocks[b]);
        for (let s = c === start.ch && b === start.b ? start.s : 0; s < ss.length; s++) yield { text: ss[s], show: () => void showPiece(c, b, s, ss.length) };
      }
    }
  }

  // ---------------------------------------------------------------- links, keys, size

  function onClick(e: MouseEvent) {
    const a = (e.target as HTMLElement).closest('a[data-link]') as HTMLAnchorElement | null;
    if (!a) return;
    e.preventDefault();
    const link = a.dataset.link!;
    if (/^(https?|mailto):/i.test(link)) return store.runAction({ kind: 'open_url', url: link });
    void follow(link);
  }

  async function follow(link: string) {
    if (!content) return;
    const [path, frag] = link.includes('#') ? [link.split('#')[0], link.split('#')[1]] : [link, null];
    let c = path ? content.chapters.findIndex((x) => x.path === path) : -1;
    if (c < 0 && frag) {
      // A bare "#id": this chapter, or wherever it is.
      if (!anchorChapter.has(frag)) for (let i = 0; i < content.chapters.length && !anchorChapter.has(frag); i++) prep(i);
      c = anchorChapter.get(frag) ?? -1;
    }
    if (c < 0) return;
    books.pause();
    await showChapter(c, frag ? { anchor: frag } : 'start');
  }

  function onKey(e: KeyboardEvent) {
    const t = e.target as HTMLElement;
    if (t.closest('input, textarea, [contenteditable]')) return;
    if (e.key === 'ArrowRight' || e.key === 'PageDown') books.turn(1);
    else if (e.key === 'ArrowLeft' || e.key === 'PageUp') books.turn(-1);
    else if (e.key === ' ' && !e.metaKey) books.toggle();
    else return;
    e.preventDefault();
  }

  /** Re-lays out the pages, keeping the first paragraph on the page in view. */
  function relayout() {
    if (isPdf) return void renderPdf();
    if (!flow || !content) return;
    const first = blocksOnPage()[0];
    measure();
    const el = first !== undefined ? blockEl(first) : null;
    pg = Math.min(pages - 1, el ? pageOf(el) : pg);
    place();
  }

  // Font size and spread change the layout.
  $effect(() => {
    void books.settings.fontSize;
    void books.settings.font;
    void books.settings.spread;
    untrack(() => requestAnimationFrame(relayout));
  });
  $effect(() => {
    void zoom;
    if (isPdf) untrack(() => void renderPdf());
  });

  // ---------------------------------------------------------------- the reader, for books.svelte.ts

  const chapterTitle = () => (isPdf ? '' : (chaps[ch]?.title ?? ''));
  const reader: Reader = {
    // The library's copy: its progress and title stay current.
    get book() {
      return books.find(book.id) ?? book;
    },
    pageText: () => (isPdf ? (pdfText.get(pg) ?? '') : textOfPage()),
    before: () => {
      if (isPdf) return pdfText.get(pg - 1) ?? '';
      const on = blocksOnPage();
      const c = chaps[ch];
      return on.length && on[0] > 0 ? (c?.blocks.slice(Math.max(0, on[0] - 3), on[0]).join(' ') ?? '') : '';
    },
    where: () =>
      isPdf
        ? `page ${pg + 1} of ${pages}`
        : `${chapterTitle() ? `“${chapterTitle()}” (part ${ch + 1} of ${content?.chapters.length ?? 1})` : `part ${ch + 1}`}, page ${pg + 1} of ${pages} in it`,
    pieces,
    turn,
  };

  export function jump(i: number, anchor: string | null) {
    books.pause();
    void showChapter(i, anchor ? { anchor } : 'start');
  }
  export function jumpPage(p: number) {
    books.pause();
    goto(p);
  }
  export const label = () => (isPdf ? `Page ${pg + 1} of ${pages}` : `${chapterTitle()} · ${pg + 1} / ${pages}`);

  onMount(() => {
    let dead = false;
    const ro = new ResizeObserver(() => requestAnimationFrame(relayout));
    (async () => {
      try {
        if (isPdf) {
          const data = await invoke<ArrayBuffer>('books_file', { id: book.id });
          pdf = await pdfjs.getDocument({ data: new Uint8Array(data) }).promise;
          if (dead) return;
          pages = pdf.numPages;
          pg = Math.min(pages - 1, book.progress?.page ?? 0);
          loading = false;
          await renderPdf();
          // First time: the cover and the title come from the PDF itself.
          if (!book.cover) {
            const c = pdfBox?.querySelector('canvas');
            if (c && pg === 0) void books.update(book.id, { cover: c.toDataURL('image/jpeg', 0.8) }).catch(() => {});
          }
          if (!book.progress) {
            const meta: any = await pdf.getMetadata().catch(() => null);
            const t = meta?.info?.Title?.trim();
            const a = meta?.info?.Author?.trim();
            if ((t && t.length > 2 && !/^(untitled|microsoft word)/i.test(t)) || a) void books.update(book.id, { ...(t ? { title: t } : {}), ...(a ? { author: a } : {}) }).catch(() => {});
          }
          void pageText(pg);
        } else {
          content = await invoke<Content>('books_content', { id: book.id });
          if (dead) return;
          chaps = content.chapters.map(() => null);
          // The table of contents: the book's own, else the chapters' headings.
          const items = content.toc.length
            ? content.toc.map((e) => {
                const [path, frag] = e.href.split('#');
                return { title: e.title, ch: Math.max(0, content!.chapters.findIndex((c) => c.path === path)), anchor: frag ?? null, depth: e.depth };
              })
            : content.chapters.map((_, i) => ({ title: prep(i).title, ch: i, anchor: null, depth: 0 }));
          toc = items;
          loading = false;
          await new Promise((r) => requestAnimationFrame(r));
          const p = book.progress;
          await showChapter(p?.chapter ?? 0, p ? { block: p.block } : 'start');
        }
        if (stage) ro.observe(stage);
        books.attach(reader);
      } catch (e: any) {
        error = e?.message ?? String(e);
        loading = false;
      }
    })();
    return () => {
      dead = true;
      ro.disconnect();
      books.attach(null);
      void (pdf as any)?.destroy?.();
    };
  });

  const sizes = $derived(`--fs:${books.settings.fontSize}px;--font-book:${books.settings.font === 'serif' ? "'Iowan Old Style', 'Palatino', Georgia, 'Times New Roman', serif" : 'var(--font)'}`);
</script>

<svelte:window onkeydown={onKey} />

<div class="reader" data-theme-book={books.settings.theme} style={sizes}>
  {#if tocOpen && toc.length}
    <nav class="toc">
      {#each toc as t, i (i)}
        <button class="truncate" class:cur={t.ch === ch} style="padding-left:{12 + t.depth * 14}px" onclick={() => { jump(t.ch, t.anchor); tocOpen = false; }}>{t.title}</button>
      {/each}
    </nav>
  {/if}
  <div class="stage" bind:this={stage}>
    {#if loading}<div class="state"><Loader size={18} class="spin" /> Opening the book…</div>{/if}
    {#if error}<div class="state err">{error}</div>{/if}
    {#if isPdf}
      <div class="pdf" bind:this={pdfBox}></div>
    {:else}
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
      <div class="page-window" style="padding: 28px {stage && stage.clientWidth > 700 ? 64 : 28}px" onclick={onClick}>
        <div class="flow" class:two={cols === 2} bind:this={flow}></div>
      </div>
    {/if}
    <button class="turn prev" title="Previous page (←)" onclick={() => books.turn(-1)}><ChevronLeft size={22} /></button>
    <button class="turn next" title="Next page (→)" onclick={() => books.turn(1)}><ChevronRight size={22} /></button>
  </div>
  <div class="foot">
    <span class="where truncate">{isPdf ? `Page ${pg + 1} of ${pages}` : `${chaps[ch]?.title ?? ''}`}</span>
    {#if isPdf}
      <span class="zoom">
        <button class="icon-btn sm" title="Smaller" onclick={() => (zoom = Math.max(0.5, zoom - 0.1))}>−</button>
        <span>{Math.round(zoom * 100)}%</span>
        <button class="icon-btn sm" title="Bigger" onclick={() => (zoom = Math.min(3, zoom + 0.1))}>+</button>
      </span>
    {:else}
      <span class="muted">{pg + 1} / {pages}</span>
    {/if}
    <div class="progress" title="{Math.round((book.progress?.percent ?? 0) * 100)}% read"><i style="width:{(book.progress?.percent ?? 0) * 100}%"></i></div>
  </div>
</div>

<style>
  .reader {
    --paper: #fbfaf7;
    --ink: #1d1b16;
    --ink-2: #6b665a;
    --mark: rgba(124, 108, 242, 0.16);
    flex: 1;
    min-height: 0;
    display: flex;
    position: relative;
    background: var(--paper);
    color: var(--ink);
  }
  .reader[data-theme-book='sepia'] {
    --paper: #f4ecd8;
    --ink: #3b2f1e;
    --ink-2: #7a6a50;
    --mark: rgba(170, 120, 40, 0.2);
  }
  .reader[data-theme-book='night'] {
    --paper: #15151a;
    --ink: #d8d6cf;
    --ink-2: #8d8a80;
    --mark: rgba(124, 108, 242, 0.28);
  }
  :global(:root[data-theme='dark']) .reader[data-theme-book='auto'] {
    --paper: #17171c;
    --ink: #dcdad3;
    --ink-2: #8f8c82;
    --mark: rgba(124, 108, 242, 0.28);
  }
  .stage {
    flex: 1;
    min-width: 0;
    position: relative;
    overflow: hidden;
  }
  .page-window {
    position: absolute;
    inset: 0 0 28px 0;
    overflow: hidden;
  }
  .flow {
    column-fill: auto;
    font-family: var(--font-book);
    font-size: var(--fs);
    line-height: 1.6;
    color: var(--ink);
    transition: transform 0.28s var(--ease);
    hyphens: auto;
    -webkit-hyphens: auto;
    text-align: justify;
    overflow-wrap: break-word;
  }
  .flow :global(p) {
    margin: 0 0 0.7em;
  }
  .flow :global(h1),
  .flow :global(h2),
  .flow :global(h3) {
    font-weight: 600;
    line-height: 1.25;
    text-align: left;
    break-after: avoid;
    margin: 0.4em 0 0.8em;
  }
  .flow :global(h1) {
    font-size: 1.6em;
  }
  .flow :global(h2) {
    font-size: 1.35em;
  }
  .flow :global(h3) {
    font-size: 1.15em;
  }
  .flow :global(img) {
    display: block;
    max-width: 100%;
    max-height: var(--img-h);
    margin: 0.6em auto;
    object-fit: contain;
    break-inside: avoid;
  }
  .flow :global(blockquote) {
    margin: 0.6em 1.2em;
    color: var(--ink-2);
    font-style: italic;
  }
  .flow :global(pre) {
    white-space: pre-wrap;
    font-size: 0.85em;
  }
  .flow :global(a) {
    color: var(--accent);
    text-decoration: none;
  }
  .flow :global(.verse) {
    margin: 0;
    text-align: left;
  }
  .flow :global(.reading) {
    background: var(--mark);
    border-radius: 4px;
    box-shadow: 0 0 0 4px var(--mark);
  }
  .pdf {
    position: absolute;
    inset: 0 0 28px 0;
    display: grid;
    place-items: center;
    overflow: auto;
  }
  .pdf :global(.pdf-page) {
    position: relative;
    box-shadow: var(--shadow);
    background: white;
  }
  .pdf :global(canvas) {
    display: block;
  }
  .pdf :global(.textLayer) {
    position: absolute;
    inset: 0;
    overflow: clip;
    line-height: 1;
    transform-origin: 0 0;
    --min-font-size: 1;
    --text-scale-factor: calc(var(--total-scale-factor) * var(--min-font-size));
    --min-font-size-inv: calc(1 / var(--min-font-size));
  }
  .pdf :global(.textLayer span),
  .pdf :global(.textLayer br) {
    color: transparent;
    position: absolute;
    white-space: pre;
    cursor: text;
    transform-origin: 0% 0%;
    user-select: text;
    -webkit-user-select: text;
    --font-height: 0;
    font-size: calc(var(--text-scale-factor) * var(--font-height));
    --scale-x: 1;
    --rotate: 0deg;
    transform: rotate(var(--rotate)) scaleX(var(--scale-x)) scale(var(--min-font-size-inv));
  }
  .pdf :global(.textLayer .markedContent) {
    display: contents;
  }
  .pdf :global(.textLayer ::selection) {
    background: rgba(0, 90, 255, 0.25);
  }
  .turn {
    position: absolute;
    top: 0;
    bottom: 28px;
    width: 48px;
    display: grid;
    place-items: center;
    color: var(--ink-2);
    opacity: 0;
    transition: opacity var(--t);
    background: none;
    border: 0;
    cursor: pointer;
  }
  .turn:hover {
    opacity: 1;
  }
  .prev {
    left: 0;
  }
  .next {
    right: 0;
  }
  .foot {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 28px;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 0 18px;
    font-size: 11.5px;
    color: var(--ink-2);
  }
  .where {
    flex: 1;
    min-width: 0;
  }
  .zoom {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .progress {
    width: 120px;
    height: 3px;
    border-radius: 2px;
    background: color-mix(in oklab, var(--ink) 12%, transparent);
    overflow: hidden;
  }
  .progress i {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  .toc {
    width: 270px;
    flex: none;
    overflow: auto;
    border-right: 1px solid color-mix(in oklab, var(--ink) 10%, transparent);
    padding: 8px 0;
    display: flex;
    flex-direction: column;
  }
  .toc button {
    text-align: left;
    background: none;
    border: 0;
    color: var(--ink);
    font-size: 13px;
    padding: 6px 12px;
    cursor: pointer;
  }
  .toc button:hover {
    background: color-mix(in oklab, var(--ink) 6%, transparent);
  }
  .toc .cur {
    color: var(--accent);
    font-weight: 600;
  }
  .state {
    position: absolute;
    inset: 0;
    display: flex;
    gap: 8px;
    align-items: center;
    justify-content: center;
    color: var(--ink-2);
  }
  .err {
    color: var(--danger);
    padding: 30px;
    text-align: center;
  }
</style>
