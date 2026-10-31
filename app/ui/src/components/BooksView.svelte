<script lang="ts">
  // Media → Books: the owner's e-books (kept on this computer), a reader that can read them aloud,
  // and Gilgamesh underneath to talk about the page. See lib/books.svelte.ts.
  import { onMount } from 'svelte';
  import { open as pickFile, ask as confirmDialog } from '@tauri-apps/plugin-dialog';
  import { store } from '../lib/state.svelte';
  import { books, BOOK_EXTENSIONS, type Book } from '../lib/books.svelte';
  import { formatBytes, timeAgo } from '../lib/util';
  import BookReader from './BookReader.svelte';
  import BookChat from './BookChat.svelte';
  import Menu from './Menu.svelte';
  import { BookOpen, Plus, Loader, ArrowLeft, List, Play, Pause, MessageCircleQuestion, Type, Trash2, DatabaseZap, Check, MoreHorizontal, Upload } from '@lucide/svelte';

  let reader = $state<ReturnType<typeof BookReader> | null>(null);
  let chat = $state<ReturnType<typeof BookChat> | null>(null);
  let tocOpen = $state(false);
  let textMenu = $state(false);
  let adding = $state<string | null>(null);
  let filter = $state('');

  const book = $derived(books.find(books.openId));
  const sorted = $derived(
    [...books.list]
      .filter((b) => !filter.trim() || `${b.title} ${b.author ?? ''}`.toLowerCase().includes(filter.trim().toLowerCase()))
      .sort((a, b) => (b.opened ?? b.added) - (a.opened ?? a.added)),
  );

  onMount(() => {
    books.load().catch((e) => store.error(e, 'Couldn’t load your books'));
  });

  async function addBooks() {
    const picked = await pickFile({ multiple: true, filters: [{ name: 'E-books', extensions: BOOK_EXTENSIONS }] });
    const list = Array.isArray(picked) ? picked : picked ? [picked] : [];
    if (list.length) await books.import(list.map(String));
  }

  async function toVault(b: Book) {
    adding = b.id;
    try {
      await books.addToVault(b.id);
    } catch (e) {
      store.error(e, 'Couldn’t add it to the vault');
    } finally {
      adding = null;
    }
  }

  async function remove(b: Book) {
    const ok = await confirmDialog(`Remove “${b.title}” from Books? The copy RAD keeps on this computer is deleted${b.vault ? '; the one in your vault stays' : ''}.`, { title: 'Remove book', kind: 'warning' });
    if (ok) await books.remove(b.id).catch((e) => store.error(e, 'Couldn’t remove the book'));
  }

  const pct = (b: Book) => Math.round((b.progress?.percent ?? 0) * 100);
  const hue = (s: string) => [...s].reduce((h, c) => (h * 31 + c.charCodeAt(0)) % 360, 7);
</script>

<div class="books">
  <header class="drag">
    {#if book}
      <button class="icon-btn no-drag" title="Back to your books" onclick={() => books.close()}><ArrowLeft size={16} /></button>
      <button class="icon-btn no-drag" class:on={tocOpen} title="Contents" onclick={() => (tocOpen = !tocOpen)}><List size={16} /></button>
      <div class="ttl">
        <h1 class="truncate">{book.title}</h1>
        {#if book.author}<span class="muted small truncate">{book.author}</span>{/if}
      </div>
      <span class="spacer"></span>
      <div class="no-drag rel">
        <button class="icon-btn" class:on={textMenu} title="Text and page" onclick={() => (textMenu = !textMenu)}><Type size={16} /></button>
        {#if textMenu}
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
          <div class="pop" onclick={(e) => e.stopPropagation()}>
            <div class="row">
              <span>Size</span>
              <button class="btn sm" onclick={() => books.saveSettings({ fontSize: Math.max(13, books.settings.fontSize - 1) })}>A−</button>
              <span class="num">{books.settings.fontSize}</span>
              <button class="btn sm" onclick={() => books.saveSettings({ fontSize: Math.min(32, books.settings.fontSize + 1) })}>A+</button>
            </div>
            <div class="row">
              <span>Font</span>
              <div class="seg">
                <button class:on={books.settings.font === 'serif'} onclick={() => books.saveSettings({ font: 'serif' })}>Serif</button>
                <button class:on={books.settings.font === 'sans'} onclick={() => books.saveSettings({ font: 'sans' })}>Sans</button>
              </div>
            </div>
            <div class="row">
              <span>Page</span>
              <div class="seg">
                {#each [['auto', 'Auto'], ['paper', 'Paper'], ['sepia', 'Sepia'], ['night', 'Night']] as [k, l] (k)}
                  <button class:on={books.settings.theme === k} onclick={() => books.saveSettings({ theme: k as any })}>{l}</button>
                {/each}
              </div>
            </div>
            <label class="row check"><input type="checkbox" checked={books.settings.spread} onchange={(e) => books.saveSettings({ spread: e.currentTarget.checked })} /> Two pages side by side on a wide window</label>
          </div>
        {/if}
      </div>
      <div class="seg no-drag" title="Who reads aloud">
        <button class:on={books.settings.voice === 'gilgamesh'} title="Gilgamesh reads, in his voice, and his lips move" onclick={() => books.saveSettings({ voice: 'gilgamesh' })}>Gilgamesh</button>
        <button class:on={books.settings.voice === 'narrator'} title="The voice chosen in Settings → Voice" onclick={() => books.saveSettings({ voice: 'narrator' })}>Narrator</button>
      </div>
      <select class="input sm no-drag speed" title="Reading speed" value={String(books.settings.speed)} onchange={(e) => books.saveSettings({ speed: Number(e.currentTarget.value) })}>
        {#each [0.75, 0.9, 1, 1.1, 1.25, 1.5] as s (s)}<option value={String(s)}>{s}×</option>{/each}
      </select>
      <button class="btn sm primary no-drag play" title="{books.playing ? 'Pause' : 'Read aloud from here'} (space)" onclick={() => books.toggle()}>
        {#if books.playing && books.loading}<Loader size={14} class="spin" />{:else if books.playing}<Pause size={14} />{:else}<Play size={14} />{/if}
        {books.playing ? 'Pause' : 'Read aloud'}
      </button>
      <button class="btn sm no-drag" title="Pause and ask Gilgamesh about this page" onclick={() => chat?.focus()}><MessageCircleQuestion size={14} /> Ask Gilgamesh</button>
    {:else}
      <BookOpen size={17} />
      <h1>Books</h1>
      <span class="muted small">{books.list.length ? `${books.list.length} on this computer` : ''}</span>
      <span class="spacer"></span>
      {#if books.list.length > 6}<input class="input sm no-drag search" placeholder="Find a book…" bind:value={filter} />{/if}
      <button class="btn sm primary no-drag" disabled={books.importing} onclick={addBooks}>{#if books.importing}<Loader size={13} class="spin" />{:else}<Plus size={13} />{/if} Add books</button>
    {/if}
  </header>

  {#if book}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="read" onclick={() => (textMenu = false)}>
      {#key book.id}<BookReader {book} bind:this={reader} bind:tocOpen />{/key}
      {#if books.playing && books.sentence}
        <div class="saying truncate" title={books.sentence}>{books.sentence}</div>
      {/if}
    </div>
    {#key book.id}<BookChat {book} bind:this={chat} />{/key}
  {:else if !books.loaded}
    <div class="center"><Loader size={20} class="spin" /></div>
  {:else if !books.list.length}
    <div class="center">
      <div class="empty">
        <div class="glyph"><BookOpen size={28} /></div>
        <h2>Your books, read to you</h2>
        <p>Add e-books you own (PDF, EPUB, MOBI, AZW, AZW3, FB2 or plain text). They're kept on this computer. Read them here, have Gilgamesh read them aloud, and stop on any page to ask him about it.</p>
        <button class="btn primary" disabled={books.importing} onclick={addBooks}>{#if books.importing}<Loader size={14} class="spin" />{:else}<Upload size={14} />{/if} Add books</button>
        <p class="muted small">Or drop the files on this window. Kindle books bought from Amazon are usually DRM-locked; RAD opens DRM-free books only.</p>
      </div>
    </div>
  {:else}
    <div class="grid">
      {#each sorted as b (b.id)}
        <div class="card">
          <button class="cover" title="Open" onclick={() => books.openBook(b.id)} style="--h:{hue(b.title)}">
            {#if b.cover}<img src={b.cover} alt="" />{:else}<span class="gen"><strong>{b.title}</strong>{#if b.author}<small>{b.author}</small>{/if}</span>{/if}
            <span class="fmt">{b.format.toUpperCase()}</span>
            {#if b.vault}<span class="rag" title="In your vault (searchable from any chat)"><Check size={11} /> RAG</span>{/if}
          </button>
          <div class="meta">
            <button class="name truncate" title={b.title} onclick={() => books.openBook(b.id)}>{b.title}</button>
            <Menu
              items={[
                { label: b.vault ? 'In your vault (RAG)' : 'Add to RAG', icon: b.vault ? Check : DatabaseZap, disabled: !!b.vault || adding === b.id, onclick: () => toVault(b) },
                null,
                { label: 'Remove', icon: Trash2, danger: true, onclick: () => remove(b) },
              ]}
            >
              {#snippet trigger(toggle: () => void)}<button class="icon-btn sm" title="More" onclick={toggle}><MoreHorizontal size={14} /></button>{/snippet}
            </Menu>
          </div>
          <div class="sub muted truncate">{b.author ?? formatBytes(b.size)}</div>
          <div class="prog" title={b.progress ? `${pct(b)}% read` : 'Not started'}><i style="width:{pct(b)}%"></i></div>
          <div class="sub muted small">{b.progress ? `${pct(b)}% · ${timeAgo(b.opened ?? b.added)}` : 'Not started'}</div>
          {#if !b.vault}
            <button class="btn sm ghost addrag" disabled={adding === b.id} title="Upload it to your vault, so every chat can search it" onclick={() => toVault(b)}>
              {#if adding === b.id}<Loader size={12} class="spin" />{:else}<DatabaseZap size={12} />{/if} Add to RAG
            </button>
          {/if}
        </div>
      {/each}
      <button class="card add" onclick={addBooks} disabled={books.importing}>
        {#if books.importing}<Loader size={22} class="spin" />{:else}<Plus size={22} />{/if}
        <span>Add books</span>
      </button>
    </div>
  {/if}
</div>

<style>
  .books {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  header {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    height: var(--titlebar-h);
    padding: 0 12px 0 calc(14px + var(--lead));
    border-bottom: 1px solid var(--border);
  }
  h1 {
    margin: 0;
    font-size: 15px;
    font-weight: 650;
  }
  .ttl {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
    flex: 0 1 auto;
  }
  .small {
    font-size: 12px;
  }
  .spacer {
    flex: 1;
  }
  .on {
    color: var(--accent);
  }
  .rel {
    position: relative;
  }
  .pop {
    position: absolute;
    right: 0;
    top: calc(100% + 6px);
    z-index: 20;
    width: 290px;
    padding: 10px;
    border-radius: var(--r);
    background: var(--surface);
    border: 1px solid var(--border-strong);
    box-shadow: var(--shadow);
    display: flex;
    flex-direction: column;
    gap: 10px;
    font-size: 12.5px;
  }
  .pop .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .pop .row > span:first-child {
    width: 38px;
    color: var(--text-3);
  }
  .pop .num {
    width: 22px;
    text-align: center;
  }
  .check {
    color: var(--text-2);
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    overflow: hidden;
  }
  .seg button {
    background: none;
    border: 0;
    padding: 4px 9px;
    font-size: 12px;
    color: var(--text-2);
    cursor: pointer;
  }
  .seg button + button {
    border-left: 1px solid var(--border);
  }
  .seg button.on {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .speed {
    width: 64px;
  }
  .play {
    min-width: 108px;
  }
  .search {
    width: 180px;
  }
  .read {
    flex: 1;
    min-height: 0;
    display: flex;
    position: relative;
  }
  .saying {
    position: absolute;
    left: 50%;
    bottom: 34px;
    transform: translateX(-50%);
    max-width: min(720px, 80%);
    padding: 5px 12px;
    border-radius: 999px;
    font-size: 12px;
    background: var(--glass);
    backdrop-filter: blur(8px);
    border: 1px solid var(--border);
    color: var(--text-2);
    pointer-events: none;
  }
  .center {
    flex: 1;
    display: grid;
    place-items: center;
    padding: 24px;
  }
  .empty {
    max-width: 480px;
    text-align: center;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
  }
  .empty h2 {
    margin: 4px 0 0;
    font-size: 19px;
  }
  .empty p {
    margin: 0;
    color: var(--text-2);
    line-height: 1.5;
  }
  .glyph {
    width: 60px;
    height: 60px;
    border-radius: 18px;
    display: grid;
    place-items: center;
    color: var(--accent);
    background: var(--accent-soft);
  }
  .grid {
    flex: 1;
    overflow: auto;
    padding: 20px;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 22px 18px;
    align-content: start;
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
  }
  .cover {
    position: relative;
    aspect-ratio: 2 / 3;
    border-radius: 6px;
    overflow: hidden;
    border: 1px solid var(--border);
    padding: 0;
    cursor: pointer;
    background: linear-gradient(160deg, hsl(var(--h) 45% 42%), hsl(calc(var(--h) + 40) 50% 26%));
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.12);
    transition: transform var(--t) var(--ease);
  }
  .cover:hover {
    transform: translateY(-2px);
  }
  .cover img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .gen {
    position: absolute;
    inset: 0;
    padding: 16px 12px;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 8px;
    color: white;
    text-align: center;
    font-family: 'Iowan Old Style', Palatino, Georgia, serif;
  }
  .gen strong {
    font-size: 15px;
    line-height: 1.25;
  }
  .gen small {
    opacity: 0.8;
  }
  .fmt,
  .rag {
    position: absolute;
    top: 6px;
    font-size: 9.5px;
    font-weight: 700;
    letter-spacing: 0.04em;
    padding: 2px 6px;
    border-radius: 5px;
    background: rgba(0, 0, 0, 0.55);
    color: white;
    display: inline-flex;
    align-items: center;
    gap: 2px;
  }
  .fmt {
    left: 6px;
  }
  .rag {
    right: 6px;
    background: color-mix(in oklab, var(--success) 85%, black);
  }
  .meta {
    display: flex;
    align-items: center;
    gap: 2px;
    margin-top: 6px;
  }
  .name {
    flex: 1;
    min-width: 0;
    text-align: left;
    background: none;
    border: 0;
    padding: 0;
    font-weight: 600;
    font-size: 13px;
    color: var(--text);
    cursor: pointer;
  }
  .sub {
    font-size: 12px;
  }
  .prog {
    height: 3px;
    border-radius: 2px;
    background: var(--surface-3);
    overflow: hidden;
    margin-top: 4px;
  }
  .prog i {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  .addrag {
    align-self: flex-start;
    margin-top: 2px;
    padding-left: 0;
  }
  .card.add {
    aspect-ratio: 2 / 3;
    border: 1.5px dashed var(--border-strong);
    border-radius: 6px;
    background: none;
    color: var(--text-3);
    align-items: center;
    justify-content: center;
    gap: 6px;
    cursor: pointer;
    font-size: 13px;
  }
  .card.add:hover {
    color: var(--accent);
    border-color: var(--accent);
  }
</style>
