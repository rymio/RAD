<script lang="ts">
  // A chat with Gilgamesh about one thing (a circuit in Z80 Lab…): his small face, the microphone,
  // one-click questions and the conversation, which is one of his (kept and recapped). `about`
  // says whether the conversation going on is the one about this thing; `discuss` makes it so.
  import { tick, type Component } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { store } from '../lib/state.svelte';
  import { gilgamesh } from '../lib/gilgamesh/gilgamesh.svelte';
  import { avatarImage } from '../lib/gilgamesh/face';
  import { Head } from '../lib/gilgamesh/head';
  import { ArrowUp, Mic, MicOff, Square, Loader, SquarePen, ScanFace, ChevronDown, ChevronUp } from '@lucide/svelte';

  interface Chip {
    label: string;
    icon?: Component<any>;
    ask: string;
    disabled?: boolean;
  }
  let {
    about,
    discuss,
    chips = [],
    placeholder = 'Ask Gilgamesh…',
    hint = '',
    storeKey = 'rad-gilgamesh-chat',
  }: { about: boolean; discuss: () => Promise<void>; chips?: Chip[]; placeholder?: string; hint?: string; storeKey?: string } = $props();
  const g = gilgamesh;

  let text = $state('');
  let scroller = $state<HTMLDivElement | null>(null);
  let canvas = $state<HTMLCanvasElement | null>(null);
  let ready = $state<boolean | null>(null);

  const saved = (() => {
    try {
      return JSON.parse(localStorage.getItem(storeKey) ?? '{}');
    } catch {
      return {};
    }
  })();
  let collapsed = $state<boolean>(saved.collapsed ?? false);
  let height = $state<number>(saved.height ?? 200);
  $effect(() => {
    try {
      localStorage.setItem(storeKey, JSON.stringify({ collapsed, height }));
    } catch {
      /* private mode */
    }
  });
  const grow = (by: number) => (height = Math.max(110, Math.min(Math.round(window.innerHeight * 0.6), height + by)));

  const turns = $derived(about ? g.turns.filter((t) => !t.quiet) : []);
  const on = $derived(g.phase !== 'off');
  const busy = $derived(g.phase === 'thinking' || g.phase === 'speaking');
  const caption = $derived(g.heard || (busy ? g.status || g.saying : '') || g.error || '');

  $effect(() => {
    invoke<any>('tts_status')
      .then((t) => (ready = !!t?.installed && !!t?.engine))
      .catch(() => (ready = false));
  });

  $effect(() => {
    const a = g.avatar;
    const c = canvas;
    if (!a || !c) return;
    let head: Head | null = null;
    let off = () => {};
    let dead = false;
    avatarImage(a)
      .then((img) => {
        if (dead) return;
        head = new Head(c, a, img);
        off = g.head.add(head);
      })
      .catch(() => {});
    return () => {
      dead = true;
      off();
      head?.destroy();
    };
  });

  $effect(() => {
    void turns.length;
    void turns.at(-1)?.text;
    tick().then(() => scroller?.scrollTo({ top: scroller.scrollHeight }));
  });

  async function ask(q: string) {
    q = q.trim();
    if (!q) return;
    collapsed = false;
    await discuss();
    void g.ask(q);
  }

  function send(e?: Event) {
    e?.preventDefault();
    const q = text;
    text = '';
    void ask(q);
  }

  async function toggleMic() {
    if (on) return g.stop();
    if (!ready) return store.navigate({ kind: 'gilgamesh' });
    await discuss();
    collapsed = false;
    await g.start();
  }
</script>

<div class="chat" class:collapsed style={collapsed ? '' : `height:${height}px`}>
  <div class="bar">
    {#if g.avatar}
      <button class="pic" class:talking={busy} title="Gilgamesh" onclick={() => (collapsed = !collapsed)}><canvas bind:this={canvas}></canvas></button>
    {:else}
      <button class="icon-btn sm" title="Give Gilgamesh a face" onclick={() => store.navigate({ kind: 'gilgamesh' })}><ScanFace size={15} /></button>
    {/if}
    <strong>Gilgamesh</strong>
    <span class="cap truncate" class:you={!!g.heard}>{caption}</span>
    <span class="spacer"></span>
    {#if !collapsed}
      {#each chips as c (c.label)}
        <button class="chip" disabled={c.disabled} onclick={() => ask(c.ask)}>{#if c.icon}<c.icon size={12} />{/if} {c.label}</button>
      {/each}
      {#if about && turns.length}
        <button class="icon-btn sm" title="New talk" onclick={async () => { await g.newConversation(); await discuss(); }}><SquarePen size={13} /></button>
      {/if}
      <button class="icon-btn sm" title="Smaller" onclick={() => grow(-60)}><ChevronDown size={14} /></button>
      <button class="icon-btn sm" title="Bigger" onclick={() => grow(60)}><ChevronUp size={14} /></button>
    {/if}
    {#if busy}<button class="icon-btn sm" title="Stop him" onclick={() => g.interrupt()}><Square size={12} /></button>{/if}
    <button class="mic" class:on title={on ? 'Microphone off' : 'Talk with Gilgamesh'} disabled={g.phase === 'starting'} onclick={toggleMic}>
      {#if g.phase === 'starting'}<Loader size={14} class="spin" />{:else if on}<MicOff size={14} />{:else}<Mic size={14} />{/if}
    </button>
    <button class="icon-btn sm" title={collapsed ? 'Open the chat' : 'Fold the chat away'} onclick={() => (collapsed = !collapsed)}>
      {#if collapsed}<ChevronUp size={14} />{:else}<ChevronDown size={14} />{/if}
    </button>
  </div>
  {#if !collapsed}
    <div class="msgs" bind:this={scroller}>
      {#each turns as t, i (i)}
        <div class="msg" class:you={t.who === 'you'}>
          <p>{t.text}{#if t.who === 'gilgamesh' && !t.text && busy}<Loader size={12} class="spin" />{/if}</p>
          {#if t.actions?.length}<small>{t.actions.map((a) => a.label).join(' · ')}</small>{/if}
        </div>
      {:else}
        <p class="hint">{hint}</p>
      {/each}
    </div>
    <form class="compose" onsubmit={send}>
      <textarea class="input" rows="1" bind:value={text} onkeydown={(e) => e.key === 'Enter' && !e.shiftKey && !e.isComposing && send(e)} {placeholder}></textarea>
      <button class="send" disabled={!text.trim()} title="Send"><ArrowUp size={15} /></button>
    </form>
  {/if}
</div>

<style>
  .chat {
    flex: none;
    display: flex;
    flex-direction: column;
    border-top: 1px solid var(--border);
    background: var(--surface);
    min-height: 0;
  }
  .collapsed {
    height: auto;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 10px;
    min-height: 40px;
    font-size: 12.5px;
  }
  .pic {
    width: 30px;
    height: 30px;
    border-radius: 50%;
    overflow: hidden;
    padding: 0;
    border: 1px solid var(--border);
    background: var(--surface-2);
    flex: none;
    cursor: pointer;
  }
  .pic.talking {
    box-shadow: 0 0 0 2px var(--accent-ring);
  }
  .pic canvas {
    width: 100%;
    height: 100%;
    display: block;
  }
  .cap {
    color: var(--text-3);
    min-width: 0;
    flex: 0 1 auto;
  }
  .cap.you {
    color: var(--text);
    font-style: italic;
  }
  .spacer {
    flex: 1;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    padding: 3px 9px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--surface-2);
    color: var(--text-2);
    cursor: pointer;
    white-space: nowrap;
  }
  .chip:hover:not(:disabled) {
    color: var(--text);
    border-color: var(--border-strong);
  }
  .chip:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .mic {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    border: 1px solid var(--border);
    background: var(--surface-2);
    color: var(--text-2);
    cursor: pointer;
  }
  .mic.on {
    background: var(--accent);
    color: var(--on-accent);
    border-color: transparent;
  }
  .msgs {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 4px 14px 8px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .msg {
    max-width: 78%;
    align-self: flex-start;
  }
  .msg p {
    margin: 0;
    padding: 7px 11px;
    border-radius: 14px;
    background: var(--surface-2);
    font-size: 13.5px;
    line-height: 1.45;
    white-space: pre-wrap;
  }
  .msg.you {
    align-self: flex-end;
  }
  .msg.you p {
    background: var(--accent-soft);
  }
  .msg small {
    display: block;
    margin: 3px 11px 0;
    color: var(--text-3);
    font-size: 11px;
  }
  .hint {
    margin: 4px 0;
    color: var(--text-3);
    font-size: 12.5px;
    line-height: 1.5;
  }
  .compose {
    display: flex;
    gap: 6px;
    padding: 0 10px 10px;
  }
  .compose textarea {
    flex: 1;
    resize: none;
    min-height: 36px;
    max-height: 120px;
  }
  .send {
    width: 36px;
    border-radius: var(--r-sm);
    border: 0;
    background: var(--accent);
    color: var(--on-accent);
    display: grid;
    place-items: center;
    cursor: pointer;
  }
  .send:disabled {
    opacity: 0.4;
    cursor: default;
  }
</style>
