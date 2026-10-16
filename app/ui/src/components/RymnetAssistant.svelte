<script lang="ts">
  import { tick } from 'svelte';
  import { renderMarkdown } from '../lib/markdown';
  import { rymnetAgent as agent } from '../lib/rymnetAgent.svelte';
  import { Sparkles, ArrowUp, Square, Trash2, Loader, Clock, SkipForward, CircleCheck, CircleX, Play } from '@lucide/svelte';

  let { onchange }: { onchange?: () => void } = $props();

  let text = $state('');
  let input: HTMLTextAreaElement | undefined = $state();
  let scroller: HTMLDivElement | undefined = $state();
  let open = $state<Record<number, boolean>>({});

  const EXAMPLES = [
    'How do I get started with RYMWIN?',
    'Show my peers and check they answer',
    'How do I publish a website?',
    'Is my daemon running?',
    'How do I send a file to someone?',
    'Open RYMWIN',
  ];

  $effect(() => {
    agent.onChange = () => onchange?.();
    return () => (agent.onChange = null);
  });

  $effect(() => {
    agent.items.length;
    agent.status;
    tick().then(() => scroller?.scrollTo({ top: scroller.scrollHeight }));
  });

  function send(q = text) {
    q = q.trim();
    if (!q || agent.busy) return;
    text = '';
    autosize();
    agent.ask(q);
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
    input.style.height = Math.min(input.scrollHeight, 140) + 'px';
  }

  const waiting = $derived(agent.items.some((it) => it.kind === 'cmd' && it.status === 'waiting'));
</script>

<section class="assist">
  <div class="head">
    <Sparkles size={14} class="spark" />
    <span class="title">Assistant</span>
    <span class="muted sub truncate">knows the RYMWIN guide · runs <code>rym</code> for you</span>
    <span class="spacer"></span>
    <label class="askfirst" title="Show each command and wait for Run. Removing peers, releasing domains and similar always ask.">
      <input type="checkbox" bind:checked={agent.askFirst} /> Ask first
    </label>
    <button class="icon-btn sm" title="Clear the conversation" disabled={agent.busy || !agent.items.length} onclick={() => agent.clear()}><Trash2 size={13} /></button>
  </div>

  <div class="thread" bind:this={scroller}>
    {#if !agent.items.length}
      <div class="empty">
        Ask anything about Rymnet and RYMWIN, or tell the assistant what to do — join the network, manage peers, claim a <code>.rym</code> domain, publish a site, search. Removing or undoing things always waits for your OK.
        <div class="ex">
          {#each EXAMPLES as q}<button class="chip" onclick={() => send(q)}>{q}</button>{/each}
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
            {#if it.status === 'done'}<span class="ex-code">{it.timedOut ? 'timed out' : `exit ${it.exit ?? '?'}`}</span>{/if}
          </button>
          {#if it.status === 'waiting'}
            <div class="approve">
              {#if it.risky}<span class="risky">This command {it.risky}.</span>{/if}
              <span class="spacer"></span>
              <button class="btn sm ghost" onclick={() => agent.approve(false)}><SkipForward size={12} /> Skip</button>
              <button class="btn sm primary" onclick={() => agent.approve(true)}><Play size={12} /> Run</button>
            </div>
          {/if}
          {#if open[i] && it.output !== undefined}<pre class="out selectable">{it.output.trim() || '(no output)'}</pre>{/if}
        </div>
      {/if}
    {/each}
    {#if agent.busy && agent.status && !waiting}<div class="status"><Loader size={12} class="spin" /> {agent.status}</div>{/if}
  </div>

  <div class="composer">
    <textarea bind:this={input} bind:value={text} rows="1" placeholder="Ask about RYMWIN, or say what to do…" oninput={autosize} onkeydown={onKey}></textarea>
    {#if agent.busy}
      <button class="icon-btn send" title="Stop" onclick={() => agent.stop()}><Square size={13} fill="currentColor" /></button>
    {:else}
      <button class="icon-btn send" title="Send" disabled={!text.trim()} onclick={() => send()}><ArrowUp size={15} /></button>
    {/if}
  </div>
</section>

<style>
  .assist {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
    background: var(--surface);
  }
  .head {
    flex: none;
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 9px 12px;
    border-bottom: 1px solid var(--border);
    font-size: 13px;
  }
  .head :global(.spark) {
    color: var(--accent);
  }
  .title {
    font-weight: 650;
  }
  .sub {
    font-size: 12px;
    min-width: 0;
  }
  .muted {
    color: var(--text-3);
  }
  .spacer {
    flex: 1;
  }
  .askfirst {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 12px;
    color: var(--text-2);
    white-space: nowrap;
  }
  .thread {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 9px;
    font-size: 13px;
    line-height: 1.5;
  }
  .empty {
    color: var(--text-3);
    font-size: 12.5px;
    line-height: 1.55;
  }
  .ex {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 10px;
  }
  .chip {
    border: 1px solid var(--border);
    background: var(--surface-2);
    border-radius: 999px;
    padding: 3px 10px;
    font-size: 12px;
    color: var(--text-2);
  }
  .chip:hover {
    background: var(--surface-hover);
    color: var(--text);
  }
  .user {
    align-self: flex-end;
    max-width: 85%;
    padding: 6px 11px;
    border-radius: 14px 14px 4px 14px;
    background: var(--user-bubble);
    white-space: pre-wrap;
  }
  .say {
    max-width: 96%;
  }
  .say :global(p) {
    margin: 2px 0;
  }
  .err {
    color: var(--danger);
    font-size: 12.5px;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text-3);
    font-size: 12px;
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
    text-align: left;
  }
  .cl code {
    flex: 1;
    min-width: 0;
    font-family: var(--mono);
    font-size: 12px;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .cl :global(.ok) {
    color: var(--success);
  }
  .cl :global(.bad) {
    color: var(--danger);
  }
  .ex-code {
    font-size: 11px;
    color: var(--text-3);
    white-space: nowrap;
  }
  .approve {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 10px;
    border-top: 1px solid var(--border);
  }
  .risky {
    color: var(--warning);
    font-size: 12px;
  }
  .out {
    margin: 0;
    padding: 8px 10px;
    border-top: 1px solid var(--border);
    background: var(--code-bg);
    font-family: var(--mono);
    font-size: 11.5px;
    line-height: 1.5;
    max-height: 260px;
    overflow: auto;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .composer {
    flex: none;
    display: flex;
    align-items: flex-end;
    gap: 6px;
    margin: 10px 12px 12px;
    padding: 6px 6px 6px 12px;
    border: 1px solid var(--border-strong);
    border-radius: var(--r);
    background: var(--surface);
  }
  .composer:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  textarea {
    flex: 1;
    border: none;
    outline: none;
    resize: none;
    background: transparent;
    font-size: 13px;
    line-height: 1.5;
    padding: 4px 0;
    max-height: 140px;
    user-select: text;
    -webkit-user-select: text;
  }
  .send {
    flex: none;
    width: 28px;
    height: 28px;
    border-radius: 8px;
    background: var(--accent);
    color: var(--on-accent);
  }
  .send:disabled {
    background: var(--surface-3);
    color: var(--text-4);
  }
</style>
