<script lang="ts">
  import { onMount } from 'svelte';
  import { open as openDialog } from '@tauri-apps/plugin-dialog';
  import { store } from '../lib/state.svelte';
  import { rad, thumbUrl } from '../lib/api';
  import { docKind } from '../lib/util';
  import type { Document, Id } from '../lib/types';
  import DocIcon from './DocIcon.svelte';
  import { ArrowUp, Square, Paperclip, Camera, X, Loader } from '@lucide/svelte';

  let {
    streaming = false,
    placeholder = 'Ask anything about your vault…',
    targetProject = null,
    onsend,
    onstop,
    autofocus = true,
    large = false,
  }: {
    streaming?: boolean;
    placeholder?: string;
    targetProject?: Id | null;
    onsend: (text: string, attachments: Document[]) => void;
    onstop?: () => void;
    autofocus?: boolean;
    large?: boolean;
  } = $props();

  let text = $state('');
  let attachments = $state<Document[]>([]);
  let busy = $state(0);
  let ta: HTMLTextAreaElement | undefined = $state();
  let mention = $state<{ q: string; start: number } | null>(null);
  let mentionIndex = $state(0);

  const mentionList = $derived(
    mention ? store.projects.filter((p) => !p.archived && p.name.toLowerCase().includes(mention!.q.toLowerCase())).slice(0, 6) : [],
  );

  onMount(() => {
    if (autofocus) ta?.focus();
    const onAttach = (e: Event) => {
      attachments = [...attachments, ...((e as CustomEvent).detail as Document[])];
    };
    window.addEventListener('rad-attach', onAttach);
    return () => window.removeEventListener('rad-attach', onAttach);
  });

  $effect(() => {
    text;
    if (!ta) return;
    ta.style.height = 'auto';
    ta.style.height = Math.min(ta.scrollHeight, 260) + 'px';
  });

  async function project(): Promise<Id> {
    return targetProject ?? (await store.inboxProject()).id;
  }

  function submit() {
    const t = text.trim();
    if ((!t && !attachments.length) || streaming || busy) return;
    onsend(t, attachments);
    text = '';
    attachments = [];
    mention = null;
  }

  function onKey(e: KeyboardEvent) {
    if (mention && mentionList.length) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        mentionIndex = (mentionIndex + 1) % mentionList.length;
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        mentionIndex = (mentionIndex - 1 + mentionList.length) % mentionList.length;
        return;
      }
      if (e.key === 'Enter' || e.key === 'Tab') {
        e.preventDefault();
        pickMention(mentionList[mentionIndex].name);
        return;
      }
      if (e.key === 'Escape') {
        mention = null;
        return;
      }
    }
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      submit();
    }
  }

  function onInput() {
    if (!ta) return;
    const pos = ta.selectionStart;
    const before = text.slice(0, pos);
    const m = /(^|\s)@([\w\- ]{0,30})$/.exec(before);
    if (m && !/\s{2}/.test(m[2])) {
      mention = { q: m[2], start: pos - m[2].length - 1 };
      mentionIndex = 0;
    } else mention = null;
  }

  function pickMention(name: string) {
    if (!mention || !ta) return;
    const token = '@' + name.replace(/\s+/g, '-') + ' ';
    const end = ta.selectionStart;
    const caret = mention.start + token.length;
    text = text.slice(0, mention.start) + token + text.slice(end);
    mention = null;
    requestAnimationFrame(() => {
      ta?.focus();
      ta?.setSelectionRange(caret, caret);
    });
  }

  async function onPaste(e: ClipboardEvent) {
    const files = Array.from(e.clipboardData?.files ?? []);
    if (!files.length) return;
    e.preventDefault();
    for (const f of files) {
      busy++;
      try {
        const pid = await project();
        const name = f.name && f.name !== 'image.png' ? f.name : `Pasted ${new Date().toLocaleString().replace(/[/:]/g, '-')}.${(f.type.split('/')[1] || 'png').replace('jpeg', 'jpg')}`;
        const res = await rad.uploadBytes(pid, name, await f.arrayBuffer(), 'paste');
        attachments = [...attachments, res.document];
      } catch (err) {
        store.error(err, 'Paste failed');
      } finally {
        busy--;
      }
    }
  }

  async function attachFiles() {
    const sel = await openDialog({ multiple: true, title: 'Attach files' });
    if (!sel) return;
    const paths = Array.isArray(sel) ? sel : [sel];
    busy++;
    try {
      const res = await rad.uploadPaths(await project(), paths, 'chat');
      for (const r of res) {
        if ('document' in r) attachments = [...attachments, r.document];
        else store.toast('error', 'Upload failed', r.error);
      }
    } catch (e) {
      store.error(e, 'Upload failed');
    } finally {
      busy--;
    }
  }

  async function screenshot() {
    busy++;
    try {
      const r = await rad.captureScreenshot(await project());
      if (r) attachments = [...attachments, r.document];
    } catch (e) {
      store.error(e, 'Screenshot failed');
    } finally {
      busy--;
    }
  }

  export function focus() {
    ta?.focus();
  }
  export function setText(t: string) {
    text = t;
    ta?.focus();
  }
</script>

<div class="composer" class:large>
  {#if mention && mentionList.length}
    <div class="mentions">
      <div class="mh">Search only in…</div>
      {#each mentionList as p, i}
        <button class="mi" class:sel={i === mentionIndex} onmousedown={(e) => (e.preventDefault(), pickMention(p.name))}>
          <span class="pdot" style="background:{p.color}"></span>{p.name}<span class="spacer"></span><span class="muted">{p.document_count + p.note_count} items</span>
        </button>
      {/each}
    </div>
  {/if}
  {#if attachments.length || busy}
    <div class="atts">
      {#each attachments as a, i (a.id + '-' + i)}
        <div class="att">
          {#if a.mime.startsWith('image/')}
            <img src={thumbUrl(a.id)} alt={a.title} onerror={(e) => ((e.currentTarget as HTMLImageElement).style.visibility = 'hidden')} />
          {:else}
            <DocIcon kind={docKind(a)} size={16} tile />
          {/if}
          <span class="truncate">{a.title}</span>
          <button class="icon-btn sm" onclick={() => (attachments = attachments.filter((_, j) => j !== i))} aria-label="Remove"><X size={12} /></button>
        </div>
      {/each}
      {#if busy}<div class="att loading"><Loader size={15} class="spin" /> Adding…</div>{/if}
    </div>
  {/if}
  <textarea
    bind:this={ta}
    bind:value={text}
    {placeholder}
    rows="1"
    onkeydown={onKey}
    oninput={onInput}
    onpaste={onPaste}
    spellcheck="true"
  ></textarea>
  <div class="bar">
    <button class="icon-btn" title="Attach files" onclick={attachFiles}><Paperclip size={17} /></button>
    <button class="icon-btn" title="Screenshot" onclick={screenshot}><Camera size={17} /></button>
    <span class="hint-line">Paste images · <b>@</b> to pick a project · <span class="kbd">⏎</span> send</span>
    <span class="spacer"></span>
    {#if streaming}
      <button class="send stop" onclick={() => onstop?.()} title="Stop"><Square size={13} fill="currentColor" /></button>
    {:else}
      <button class="send" disabled={(!text.trim() && !attachments.length) || busy > 0} onclick={submit} title="Send"><ArrowUp size={18} strokeWidth={2.4} /></button>
    {/if}
  </div>
</div>

<style>
  .composer {
    position: relative;
    border-radius: 20px;
    border: 1px solid var(--border-strong);
    background: var(--surface);
    box-shadow: var(--shadow);
    padding: 10px 10px 8px 14px;
    transition: border-color var(--t), box-shadow var(--t);
  }
  .composer:focus-within {
    border-color: var(--accent-ring);
    box-shadow: var(--shadow), 0 0 0 4px var(--accent-softer);
  }
  textarea {
    display: block;
    width: 100%;
    resize: none;
    border: none;
    outline: none;
    background: transparent;
    font-size: 15px;
    line-height: 1.55;
    padding: 4px 2px;
    max-height: 260px;
    user-select: text;
    -webkit-user-select: text;
  }
  .large textarea {
    font-size: 16px;
    min-height: 54px;
  }
  textarea::placeholder {
    color: var(--text-4);
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 2px;
    margin-top: 4px;
    margin-left: -6px;
  }
  .hint-line {
    margin-left: 8px;
    font-size: 11.5px;
    color: var(--text-4);
  }
  .hint-line b {
    color: var(--text-3);
  }
  .send {
    width: 34px;
    height: 34px;
    border-radius: 12px;
    border: none;
    display: grid;
    place-items: center;
    background: var(--accent);
    color: var(--on-accent);
    box-shadow: 0 2px 8px color-mix(in oklab, var(--accent) 40%, transparent);
    transition: transform var(--t-fast) var(--ease-spring), opacity var(--t), background var(--t-fast);
  }
  .send:hover:not(:disabled) {
    transform: scale(1.06);
  }
  .send:disabled {
    opacity: 0.35;
    box-shadow: none;
  }
  .send.stop {
    background: var(--text);
    color: var(--surface);
  }
  .atts {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 8px;
  }
  .att {
    display: flex;
    align-items: center;
    gap: 8px;
    max-width: 240px;
    height: 44px;
    padding: 4px 4px 4px 4px;
    border-radius: 12px;
    border: 1px solid var(--border);
    background: var(--surface-2);
    font-size: 12.5px;
    animation: pop 180ms var(--ease) both;
  }
  .att img {
    width: 34px;
    height: 34px;
    border-radius: 8px;
    object-fit: cover;
    background: var(--surface-3);
  }
  .att.loading {
    padding: 0 12px;
    color: var(--text-3);
  }
  .mentions {
    position: absolute;
    left: 10px;
    bottom: calc(100% + 8px);
    width: 300px;
    padding: 5px;
    border-radius: var(--r);
    background: var(--glass);
    backdrop-filter: blur(28px);
    -webkit-backdrop-filter: blur(28px);
    border: 1px solid var(--border-strong);
    box-shadow: var(--shadow-lg);
    animation: pop 140ms var(--ease) both;
    z-index: 20;
  }
  .mh {
    padding: 4px 8px 6px;
    font-size: 11px;
    color: var(--text-3);
    font-weight: 600;
  }
  .mi {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    height: 32px;
    padding: 0 8px;
    border: none;
    border-radius: var(--r-xs);
    background: transparent;
    font-size: 13px;
    text-align: left;
  }
  .mi.sel {
    background: var(--accent-soft);
  }
  .pdot {
    width: 9px;
    height: 9px;
    border-radius: 3px;
  }
</style>
