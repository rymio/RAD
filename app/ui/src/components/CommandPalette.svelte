<script lang="ts">
  import { store } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { anchorLabel, debounce, docKind } from '../lib/util';
  import type { Citation } from '../lib/types';
  import DocIcon from './DocIcon.svelte';
  import { Search, MessageSquare, SquarePen, Settings, Lock, StickyNote, Camera, Loader, CornerDownLeft, Globe, SquareTerminal, FolderOpen, CalendarDays, Mail, Compass, Music, Clapperboard, CircleHelp, Network, Monitor, Shield, ScanFace, Activity, Bot, Palette, BookOpen } from '@lucide/svelte';
  import { rad } from '../lib/api';

  type Item = { id: string; group: string; label: string; sub?: string; icon?: any; color?: string; doc?: Citation; run: () => void };

  let q = $state('');
  let sel = $state(0);
  let hits = $state<Citation[]>([]);
  let searching = $state(false);
  let input: HTMLInputElement | undefined = $state();

  $effect(() => {
    if (store.paletteOpen) {
      q = '';
      hits = [];
      sel = 0;
      requestAnimationFrame(() => input?.focus());
    }
  });

  const search = debounce(async (text: string) => {
    if (text.trim().length < 2) {
      hits = [];
      searching = false;
      return;
    }
    try {
      hits = await api.get<Citation[]>(`/v1/search?q=${encodeURIComponent(text)}&limit=8`);
    } catch {
      hits = [];
    } finally {
      searching = false;
    }
  }, 220);

  function onInput() {
    sel = 0;
    searching = q.trim().length >= 2;
    search(q);
  }

  function close() {
    store.paletteOpen = false;
  }

  const items = $derived.by<Item[]>(() => {
    const s = q.toLowerCase().trim();
    const out: Item[] = [];
    const actions: Item[] = [
      { id: 'a-new', group: 'Actions', label: 'New chat', icon: SquarePen, run: () => store.newChat() },
      { id: 'a-gilgamesh', group: 'Actions', label: 'Talk with Gilgamesh', icon: ScanFace, run: () => store.navigate({ kind: 'gilgamesh' }) },
      { id: 'a-monitor', group: 'Actions', label: 'Web Monitoring', icon: Activity, run: () => store.navigate({ kind: 'monitor' }) },
      { id: 'a-bots', group: 'Actions', label: 'Bots', icon: Bot, run: () => store.navigate({ kind: 'bots' }) },
      { id: 'a-books', group: 'Actions', label: 'Books (read your e-books)', icon: BookOpen, run: () => store.navigate({ kind: 'books' }) },
      { id: 'a-photocraft', group: 'Actions', label: 'PhotoCraft (draw and edit pictures)', icon: Palette, run: () => store.navigate({ kind: 'photocraft' }) },
      { id: 'a-web', group: 'Actions', label: 'Web Browser', icon: Globe, run: () => store.navigate({ kind: 'browser' }) },
      { id: 'a-chromium', group: 'Actions', label: 'Chromium', icon: Compass, run: () => store.navigate({ kind: 'chromium' }) },
      { id: 'a-tor', group: 'Actions', label: 'TOR', icon: Shield, run: () => store.navigate({ kind: 'tor' }) },
      { id: 'a-term', group: 'Actions', label: 'Terminal', icon: SquareTerminal, run: () => store.navigate({ kind: 'terminal' }) },
      { id: 'a-files', group: 'Actions', label: 'Files', icon: FolderOpen, run: () => store.navigate({ kind: 'files' }) },
      { id: 'a-cal', group: 'Actions', label: 'Calendar', icon: CalendarDays, run: () => store.navigate({ kind: 'calendar' }) },
      { id: 'a-mail', group: 'Actions', label: 'Email', icon: Mail, run: () => store.navigate({ kind: 'email' }) },
      { id: 'a-music', group: 'Actions', label: 'Music', icon: Music, run: () => store.navigate({ kind: 'music' }) },
      { id: 'a-video', group: 'Actions', label: 'Video', icon: Clapperboard, run: () => store.navigate({ kind: 'video' }) },
      { id: 'a-rymnet', group: 'Actions', label: 'RYMNET', icon: Network, run: () => store.navigate({ kind: 'rymnet' }) },
      { id: 'a-rymwin', group: 'Actions', label: 'RYMWIN', icon: Monitor, run: () => store.navigate({ kind: 'rymwin' }) },
      { id: 'a-sidebar', group: 'Actions', label: 'Toggle sidebar', icon: Globe, run: () => store.toggleSidebar() },
      { id: 'a-settings', group: 'Actions', label: 'Settings', icon: Settings, run: () => store.navigate({ kind: 'settings' }) },
      { id: 'a-about', group: 'Actions', label: 'About RAD', icon: CircleHelp, run: () => (store.aboutOpen = true) },
      { id: 'a-shot', group: 'Actions', label: 'Capture screenshot', icon: Camera, run: () => rad.captureScreenshot().then((r) => r && store.toast('success', 'Screenshot added')) },
      { id: 'a-lock', group: 'Actions', label: 'Lock RAD', icon: Lock, run: () => store.lock() },
    ];
    for (const p of store.projects.filter((p) => !p.archived)) {
      if (!s || p.name.toLowerCase().includes(s)) out.push({ id: `p${p.id}`, group: 'Projects', label: p.name, sub: `${p.document_count} files · ${p.note_count} notes`, color: p.color, run: () => store.navigate({ kind: 'project', id: p.id }) });
      if (s && p.name.toLowerCase().includes(s)) out.push({ id: `pn${p.id}`, group: 'Actions', label: `New note in ${p.name}`, icon: StickyNote, run: () => store.navigate({ kind: 'project', id: p.id }) });
    }
    for (const c of store.chats) {
      if (!s || c.title.toLowerCase().includes(s)) out.push({ id: `c${c.id}`, group: 'Chats', label: c.title, sub: c.last_message ?? '', icon: MessageSquare, run: () => store.navigate({ kind: 'chat', id: c.id }) });
    }
    const docs: Item[] = hits.map((h) => ({
      id: `d${h.chunk_id}`, group: 'In your vault', label: h.title, sub: `${h.project_name}${anchorLabel(h.anchor) ? ' · ' + anchorLabel(h.anchor) : ''} — ${h.snippet}`, doc: h,
      run: () => store.navigate({ kind: 'project', id: h.project_id, doc: h.document_id }),
    }));
    const ask: Item[] = s ? [{ id: 'ask', group: 'Ask RAD', label: `Ask: “${q.trim()}”`, icon: Search, run: async () => { const c = await store.newChat(); store.sendMessage(c.id, { content: q.trim() }); } }] : [];
    const filteredActions = actions.filter((a) => !s || a.label.toLowerCase().includes(s));
    return [...ask, ...docs, ...out.slice(0, s ? 12 : 8), ...filteredActions];
  });

  const groups = $derived.by(() => {
    const g: { name: string; items: (Item & { index: number })[] }[] = [];
    items.forEach((it, index) => {
      let grp = g.find((x) => x.name === it.group);
      if (!grp) g.push((grp = { name: it.group, items: [] }));
      grp.items.push({ ...it, index });
    });
    return g;
  });

  function onKey(e: KeyboardEvent) {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      sel = Math.min(items.length - 1, sel + 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      sel = Math.max(0, sel - 1);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const it = items[sel];
      if (it) {
        close();
        it.run();
      }
    } else if (e.key === 'Escape') close();
  }
</script>

{#if store.paletteOpen}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="backdrop" onclick={close}>
    <div class="palette" onclick={(e) => e.stopPropagation()}>
      <div class="inp">
        {#if searching}<Loader size={18} class="spin" />{:else}<Search size={18} />{/if}
        <input bind:this={input} bind:value={q} oninput={onInput} onkeydown={onKey} placeholder="Search your vault, jump to a project or chat, or ask…" />
        <span class="kbd">esc</span>
      </div>
      <div class="results">
        {#each groups as g (g.name)}
          <div class="gh">{g.name}</div>
          {#each g.items as it (it.id)}
            <button class="res" class:sel={it.index === sel} onmouseenter={() => (sel = it.index)} onclick={() => (close(), it.run())}>
              {#if it.doc}
                <DocIcon kind={docKind({ mime: it.doc.mime, original_name: it.doc.title, source_kind: it.doc.source_kind })} size={16} />
              {:else if it.color}
                <span class="pdot" style="background:{it.color}"></span>
              {:else if it.icon}
                <it.icon size={16} />
              {/if}
              <div class="txt">
                <div class="l truncate">{it.label}</div>
                {#if it.sub}<div class="s truncate">{it.sub}</div>{/if}
              </div>
              {#if it.index === sel}<CornerDownLeft size={14} />{/if}
            </button>
          {/each}
        {/each}
      </div>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 600;
    display: flex;
    justify-content: center;
    padding-top: 12vh;
    background: rgba(8, 8, 16, 0.32);
    backdrop-filter: blur(2px);
  }
  .palette {
    width: 640px;
    max-width: calc(100vw - 40px);
    max-height: 64vh;
    display: flex;
    flex-direction: column;
    border-radius: 18px;
    background: var(--glass);
    backdrop-filter: blur(36px) saturate(1.7);
    -webkit-backdrop-filter: blur(36px) saturate(1.7);
    border: 1px solid var(--border-strong);
    box-shadow: var(--shadow-lg);
    animation: pop 180ms var(--ease-spring) both;
    overflow: hidden;
  }
  .inp {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 0 16px;
    height: 56px;
    border-bottom: 1px solid var(--border);
    color: var(--text-3);
  }
  .inp input {
    flex: 1;
    border: none;
    outline: none;
    background: transparent;
    font-size: 16px;
    color: var(--text);
    user-select: text;
    -webkit-user-select: text;
  }
  .results {
    overflow: auto;
    padding: 6px;
  }
  .gh {
    padding: 10px 10px 4px;
    font-size: 11px;
    font-weight: 650;
    color: var(--text-3);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .res {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    min-height: 40px;
    padding: 6px 10px;
    border: none;
    border-radius: var(--r-sm);
    background: transparent;
    text-align: left;
    color: var(--text);
  }
  .res :global(svg) {
    color: var(--text-3);
    flex: none;
  }
  .res.sel {
    background: var(--accent-soft);
  }
  .res.sel :global(svg) {
    color: var(--accent);
  }
  .txt {
    flex: 1;
    min-width: 0;
  }
  .l {
    font-size: 13.5px;
    font-weight: 500;
  }
  .s {
    font-size: 12px;
    color: var(--text-3);
  }
  .pdot {
    width: 10px;
    height: 10px;
    border-radius: 3px;
    margin: 0 3px;
    flex: none;
  }
</style>
