<script lang="ts">
  import { tick } from 'svelte';
  import { renderMarkdown } from '../lib/markdown';
  import type { SandboxAgent } from '../lib/sandboxAgent.svelte';
  import { Sparkles, ArrowUp, Square, ChevronDown, ChevronUp, Minus, Plus, Trash2, Play, SkipForward, CircleCheck, CircleX, Loader, Clock } from '@lucide/svelte';

  let { agent }: { agent: SandboxAgent } = $props();

  const saved = (() => {
    try {
      return JSON.parse(localStorage.getItem('rad-sandbox-agent') ?? '{}');
    } catch {
      return {};
    }
  })();
  let collapsed = $state<boolean>(saved.collapsed ?? false);
  let height = $state<number>(saved.height ?? 220);
  let text = $state('');
  let input: HTMLTextAreaElement | undefined = $state();
  let scroller: HTMLDivElement | undefined = $state();
  let open = $state<Record<number, boolean>>({});

  $effect(() => {
    try {
      localStorage.setItem('rad-sandbox-agent', JSON.stringify({ collapsed, height }));
    } catch {
      /* private mode */
    }
  });

  $effect(() => {
    agent.items.length;
    agent.status;
    tick().then(() => scroller?.scrollTo({ top: scroller.scrollHeight }));
  });

  function send() {
    const t = text.trim();
    if (!t || agent.busy) return;
    text = '';
    autosize();
    agent.ask(t);
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      send();
    }
  }

  function autosize() {
    if (!input) return;
    input.style.height = 'auto';
    input.style.height = Math.min(input.scrollHeight, 120) + 'px';
  }

  function grow(by: number) {
    height = Math.max(100, Math.min(Math.round(window.innerHeight * 0.65), height + by));
  }

  function startResize(e: PointerEvent) {
    const y0 = e.clientY;
    const h0 = height;
    const move = (m: PointerEvent) => (height = Math.max(100, Math.min(window.innerHeight * 0.65, h0 + (y0 - m.clientY))));
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }

  async function expand() {
    collapsed = false;
    await tick();
    input?.focus();
  }

  const examples = ['list the current directory', 'install python and print the version', 'write a python script that prints the first 20 primes and run it'];
</script>

{#if collapsed}
  <section class="dock slim">
    <button class="ask" onclick={expand}>
      <Sparkles size={14} />
      <span class="truncate">{agent.busy ? agent.status || 'Working…' : `Ask the agent to do something in ${agent.name}…`}</span>
    </button>
    <button class="icon-btn sm" title="Show the agent" onclick={expand}><ChevronUp size={15} /></button>
  </section>
{:else}
  <section class="dock">
    <div class="grip" role="separator" aria-orientation="horizontal" onpointerdown={startResize}></div>
    <div class="head">
      <Sparkles size={14} class="spark" />
      <span class="title">Sandbox agent</span>
      <span class="muted sub">runs commands in {agent.name}</span>
      <span class="spacer"></span>
      <label class="askfirst" title="Show each command and wait for Run before it runs">
        <input type="checkbox" bind:checked={agent.askFirst} /> Ask before running
      </label>
      <button class="icon-btn sm" title="Clear the conversation" disabled={agent.busy || !agent.items.length} onclick={() => agent.clear()}><Trash2 size={13} /></button>
      <button class="icon-btn sm" title="Smaller" onclick={() => grow(-60)}><Minus size={13} /></button>
      <button class="icon-btn sm" title="Bigger" onclick={() => grow(60)}><Plus size={13} /></button>
      <button class="icon-btn sm" title="Collapse" onclick={() => (collapsed = true)}><ChevronDown size={15} /></button>
    </div>

    <div class="thread" bind:this={scroller} style="height:{height}px">
      {#if !agent.items.length}
        <div class="empty">
          Tell the agent what to do — it runs the commands in this sandbox and you watch them in the terminal above.
          <div class="ex">
            {#each examples as ex}<button class="chip" onclick={() => ((text = ex), input?.focus())}>{ex}</button>{/each}
          </div>
        </div>
      {/if}
      {#each agent.items as it, i}
        {#if it.kind === 'user'}
          <div class="user">{it.text}</div>
        {:else if it.kind === 'say'}
          <div class="say md selectable">{@html renderMarkdown(it.text)}</div>
        {:else if it.kind === 'error'}
          <div class="err">{it.text}</div>
        {:else}
          <div class="cmd" class:fail={it.status === 'done' && it.exit !== 0}>
            <button class="cl" onclick={() => (open[i] = !open[i])} disabled={it.status !== 'done'}>
              {#if it.status === 'running'}<Loader size={13} class="spin" />
              {:else if it.status === 'waiting'}<Clock size={13} />
              {:else if it.status === 'skipped'}<SkipForward size={13} />
              {:else if it.timedOut}<Clock size={13} />
              {:else if it.exit === 0}<CircleCheck size={13} class="ok" />
              {:else}<CircleX size={13} class="bad" />{/if}
              <code class="selectable">{it.command}</code>
              {#if it.status === 'done'}<span class="ex-code">{it.timedOut ? 'still running' : `exit ${it.exit}`}</span>{/if}
            </button>
            {#if it.status === 'waiting'}
              <div class="approve">
                <button class="btn sm primary" onclick={() => agent.approve(true)}><Play size={12} /> Run</button>
                <button class="btn sm ghost" onclick={() => agent.approve(false)}>Skip</button>
              </div>
            {/if}
            {#if open[i] && it.output !== undefined}
              <pre class="out selectable">{it.output.trim() || '(no output)'}</pre>
            {/if}
          </div>
        {/if}
      {/each}
      {#if agent.busy && agent.status}<div class="status"><Loader size={12} class="spin" /> {agent.status}</div>{/if}
    </div>

    <div class="input-row">
      <textarea
        bind:this={input}
        bind:value={text}
        rows="1"
        placeholder="e.g. list current dir, install numpy and plot something…"
        onkeydown={onKey}
        oninput={autosize}
      ></textarea>
      {#if agent.busy}
        <button class="send stop" title="Stop the agent" onclick={() => agent.stop()}><Square size={13} /></button>
      {:else}
        <button class="send" title="Send (Enter)" disabled={!text.trim()} onclick={send}><ArrowUp size={16} /></button>
      {/if}
    </div>
  </section>
{/if}

<style>
  .dock {
    flex: none;
    display: flex;
    flex-direction: column;
    border-top: 1px solid var(--border);
    background: var(--surface);
  }
  .dock.slim {
    flex-direction: row;
    align-items: center;
    gap: 6px;
    height: 40px;
    padding: 0 10px 0 14px;
  }
  .ask {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 28px;
    padding: 0 12px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--text-3);
    font-size: 13px;
    text-align: left;
  }
  .ask :global(svg),
  .head :global(.spark) {
    color: var(--accent);
    flex: none;
  }
  .grip {
    height: 6px;
    margin-top: -3px;
    cursor: ns-resize;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 10px 4px 14px;
  }
  .title {
    font-weight: 600;
    font-size: 13px;
  }
  .sub {
    font-size: 12px;
  }
  .askfirst {
    display: flex;
    align-items: center;
    gap: 5px;
    margin-right: 6px;
    font-size: 12px;
    color: var(--text-2);
  }
  .thread {
    overflow-y: auto;
    padding: 6px 14px 10px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 13px;
  }
  .empty {
    color: var(--text-3);
    font-size: 12.5px;
  }
  .ex {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 8px;
  }
  .user {
    align-self: flex-end;
    max-width: 80%;
    padding: 6px 11px;
    border-radius: 14px 14px 4px 14px;
    background: var(--user-bubble);
    white-space: pre-wrap;
  }
  .say {
    max-width: 92%;
  }
  .say :global(p) {
    margin: 2px 0;
  }
  .err {
    color: var(--danger);
    font-size: 12.5px;
  }
  .cmd {
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    background: var(--surface-2);
    overflow: hidden;
  }
  .cmd.fail {
    border-color: color-mix(in oklab, var(--danger) 40%, var(--border));
  }
  .cl {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    width: 100%;
    padding: 6px 10px;
    border: none;
    background: transparent;
    color: var(--text);
    text-align: left;
  }
  .cl :global(svg) {
    flex: none;
    margin-top: 2px;
    color: var(--text-3);
  }
  .cl :global(.ok) {
    color: var(--success);
  }
  .cl :global(.bad) {
    color: var(--danger);
  }
  .cl code {
    flex: 1;
    min-width: 0;
    font-family: var(--mono);
    font-size: 12px;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .ex-code {
    flex: none;
    font-size: 11px;
    color: var(--text-3);
  }
  .approve {
    display: flex;
    gap: 6px;
    padding: 0 10px 8px 31px;
  }
  .out {
    margin: 0;
    max-height: 220px;
    overflow: auto;
    padding: 8px 10px;
    border-top: 1px solid var(--border);
    background: #14141b;
    color: #e6e6ef;
    font-family: var(--mono);
    font-size: 11.5px;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-3);
  }
  .input-row {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    margin: 0 14px 12px;
    padding: 5px 5px 5px 13px;
    border-radius: var(--r-lg);
    border: 1px solid var(--border);
    background: var(--surface-2);
  }
  .input-row:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-ring);
  }
  textarea {
    flex: 1;
    min-height: 22px;
    max-height: 120px;
    padding: 5px 0;
    border: none;
    outline: none;
    resize: none;
    background: transparent;
    color: var(--text);
    font: inherit;
    font-size: 13.5px;
  }
  .send {
    display: grid;
    place-items: center;
    flex: none;
    width: 30px;
    height: 30px;
    border: none;
    border-radius: 50%;
    background: var(--accent);
    color: var(--on-accent);
  }
  .send:disabled {
    opacity: 0.35;
  }
  .send.stop {
    background: var(--text);
    color: var(--surface);
  }
</style>
