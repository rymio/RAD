<script lang="ts">
  // Development → Fyrox: the Fyrox game engine and its editor, Gilgamesh's world (a Fyrox
  // project where he lives and you visit him), and the Fyrox docs for RAG. Gilgamesh is beside it.
  import { onMount } from 'svelte';
  import { ask as confirmDialog } from '@tauri-apps/plugin-dialog';
  import { store } from '../lib/state.svelte';
  import { world } from '../lib/world.svelte';
  import { fyrox } from '../lib/fyrox.svelte';
  import { timeAgo } from '../lib/util';
  import WorldChat from './WorldChat.svelte';
  import { Gamepad2, Play, Square, Loader, PencilRuler, RotateCcw, Download, DatabaseZap, Check, RefreshCw, BookOpen, Hammer, Keyboard, Moon, Sun, TriangleAlert, ListTodo } from '@lucide/svelte';

  let tasksText = $state('');
  let editingTasks = $state(false);

  onMount(() => {
    world.load().catch((e) => store.error(e, 'Couldn’t check Gilgamesh’s world'));
    fyrox.load().catch(() => {});
  });

  const st = $derived(world.status);
  const things = $derived(st?.state?.things ?? []);
  const his = $derived(things.filter((t) => t.by === 'gilgamesh').length);
  const yours = $derived(things.filter((t) => t.by === 'you').length);
  const hour = $derived(st?.state?.hour ?? new Date().getHours());
  const night = $derived(hour >= 22.5 || hour < 6);

  async function reset() {
    const ok = await confirmDialog('Start Gilgamesh’s world over? A fresh world is made; the old one is kept beside it in RAD’s Fyrox folder.', { title: 'A new world', kind: 'warning' });
    if (ok) await world.reset();
  }

  function editTasks() {
    tasksText = world.tasks.join('\n');
    editingTasks = true;
  }
  function saveTasks() {
    world.setTasks(tasksText.split('\n'));
    editingTasks = false;
  }
</script>

<div class="fyrox">
  <div class="main">
    <header class="drag">
      <Gamepad2 size={17} />
      <h1>Fyrox</h1>
      <span class="muted small">Rust game engine 1.0 · editor · Gilgamesh’s world</span>
    </header>

    <div class="scroll">
      <section class="card worldcard">
        <div class="scene" class:night>
          <div class="sky">{#if night}<Moon size={18} />{:else}<Sun size={18} />{/if}</div>
          <div class="river"></div>
          <div class="house"></div>
          <div class="g" title="Gilgamesh"></div>
          <div class="you" title="You"></div>
        </div>
        <div class="body">
          <h2>Gilgamesh’s world</h2>
          <p class="muted">A small land by a river where Gilgamesh lives. Visit him as a traveller: walk up to him and talk, about anything or your tasks. He builds things for himself, and you can build too. It’s a real Fyrox project, so you can also shape it in the Fyrox editor.</p>

          {#if st && !st.built}
            <div class="warn"><TriangleAlert size={14} /> The world isn’t built yet. Run <code>./scripts/build-fyrox.sh</code> (a few minutes the first time), then come back.</div>
          {/if}

          <div class="row">
            {#if world.running}
              <span class="live"><i></i> Open in its own window</span>
              <button class="btn sm" onclick={() => world.stop()}><Square size={13} /> Leave</button>
            {:else}
              <button class="btn primary" disabled={world.starting || !st?.built || st?.editing} onclick={() => world.start()}>
                {#if world.starting}<Loader size={14} class="spin" />{:else}<Play size={14} />{/if} Visit Gilgamesh
              </button>
            {/if}
            <button class="btn sm ghost" disabled={!st?.editor_built} title="Open the world in the Fyrox editor (the world closes while you edit)" onclick={() => world.edit()}><PencilRuler size={13} /> {st?.editing ? 'Open in the editor' : 'Edit in Fyrox'}</button>
            <button class="btn sm ghost" disabled={!st?.built} onclick={reset} title="Make a fresh world"><RotateCcw size={13} /> Start over</button>
          </div>
          {#if st?.editing}<p class="small muted">The Fyrox editor has the world open. Save (⌘S) and close it to visit Gilgamesh again.</p>{/if}

          <label class="check"><input type="checkbox" checked={world.life} onchange={(e) => world.setLife(e.currentTarget.checked)} /> <Hammer size={13} /> He builds things on his own now and then (while the world is open)</label>

          {#if st?.state}
            <div class="facts">
              <span>{things.length} things in the world</span>
              <span>{his} built by him</span>
              <span>{yours} by you</span>
              <span>He’s {st.state.gilgamesh.doing}</span>
            </div>
          {/if}
          {#if world.recent.length}
            <ul class="recent">{#each world.recent.slice(0, 5) as r, i (i)}<li>{r}</li>{/each}</ul>
          {/if}

          <div class="keys"><Keyboard size={13} /> <b>Arrows / WASD</b> walk · <b>Enter</b> talk when you’re by him · <b>Esc</b> stop talking · <b>B</b> build</div>
        </div>
      </section>

      <section class="card">
        <div class="head"><ListTodo size={15} /> <h3>The clay tablet</h3><span class="spacer"></span>{#if !editingTasks}<button class="btn sm ghost" onclick={editTasks}>Edit</button>{/if}</div>
        {#if editingTasks}
          <textarea class="input" rows="5" bind:value={tasksText} placeholder="One task per line"></textarea>
          <div class="row end"><button class="btn sm ghost" onclick={() => (editingTasks = false)}>Cancel</button><button class="btn sm primary" onclick={saveTasks}>Save</button></div>
        {:else if world.tasks.length}
          <ul class="tasks">{#each world.tasks as t, i (i)}<li>{t}</li>{/each}</ul>
        {:else}
          <p class="muted small">Your tasks, on the tablet by his house. Stand by it in the world to read them. Gilgamesh writes them there when you talk about your tasks, or edit them here.</p>
        {/if}
      </section>

      <section class="card">
        <div class="head"><BookOpen size={15} /> <h3>Fyrox docs</h3></div>
        {#if fyrox.downloading}
          <p class="row"><Loader size={13} class="spin" /> {fyrox.downloading}…</p>
        {:else if fyrox.docs}
          <p class="small">The Fyrox book ({fyrox.docs.pages} pages, {fyrox.docs.files.length} chapters) and the engine’s notes, for Fyrox {fyrox.docs.engine}. Downloaded {timeAgo(fyrox.docs.fetched)} and kept on this computer.</p>
          <div class="row">
            {#if fyrox.inRag}
              <span class="rag"><Check size={12} /> In your vault (project Fyrox): ask about Fyrox in any chat</span>
            {:else}
              <button class="btn sm primary" disabled={fyrox.adding} onclick={() => fyrox.addToRag().catch((e) => store.error(e, 'Couldn’t add the docs to the vault'))}>{#if fyrox.adding}<Loader size={13} class="spin" />{:else}<DatabaseZap size={13} />{/if} Add to RAG</button>
            {/if}
            <button class="btn sm ghost" onclick={() => fyrox.download()}><RefreshCw size={13} /> Download again</button>
          </div>
        {:else if fyrox.loaded}
          <p class="small muted">Download the Fyrox book into your vault, so you (and Gilgamesh) can ask about Fyrox in any chat, offline too.</p>
          <button class="btn sm primary" onclick={() => fyrox.download()}><Download size={13} /> Download the Fyrox docs</button>
        {/if}
      </section>
    </div>
  </div>
  <aside class="side"><WorldChat /></aside>
</div>

<style>
  .fyrox {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .side {
    width: clamp(300px, 32vw, 440px);
    display: flex;
    flex-direction: column;
    border-left: 1px solid var(--border);
    min-height: 0;
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
  .small {
    font-size: 12.5px;
  }
  .scroll {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 14px;
    max-width: 860px;
  }
  .card {
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--surface);
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .worldcard {
    padding: 0;
    overflow: hidden;
  }
  .worldcard .body {
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .worldcard h2 {
    margin: 0;
    font-size: 18px;
  }
  .worldcard p {
    margin: 0;
    line-height: 1.5;
  }
  /* A little picture of the world, drawn with CSS. */
  .scene {
    position: relative;
    height: 120px;
    background: repeating-linear-gradient(90deg, #6c9e4a 0 14px, #649645 14px 16px);
    image-rendering: pixelated;
  }
  .scene.night {
    filter: brightness(0.45) saturate(0.7) hue-rotate(15deg);
  }
  .sky {
    position: absolute;
    right: 12px;
    top: 10px;
    color: #fff8d0;
  }
  .river {
    position: absolute;
    left: 62%;
    top: 0;
    bottom: 0;
    width: 46px;
    background: #4084ba;
    box-shadow: -6px 0 0 #dec48c, 6px 0 0 #dec48c;
  }
  .house {
    position: absolute;
    left: 14%;
    top: 26px;
    width: 64px;
    height: 48px;
    background: #b08054;
    border: 2px solid #281c18;
    box-shadow: inset 0 6px 0 #c49060, inset 0 -18px 0 0 #b08054;
  }
  .house::after {
    content: '';
    position: absolute;
    left: 26px;
    bottom: 0;
    width: 12px;
    height: 18px;
    background: #46301e;
  }
  .g,
  .you {
    position: absolute;
    width: 14px;
    height: 22px;
    border: 2px solid #281c18;
    border-radius: 3px 3px 2px 2px;
  }
  .g {
    left: calc(14% + 76px);
    top: 66px;
    background: linear-gradient(#22201a 0 30%, #c6926a 30% 45%, #2e4896 45%);
  }
  .you {
    left: 40%;
    top: 74px;
    background: linear-gradient(#6e4628 0 25%, #e2b68c 25% 42%, #d6be8c 42%);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .row.end {
    justify-content: flex-end;
  }
  .live {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: var(--success);
  }
  .live i {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--success);
  }
  .warn {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 8px 10px;
    border-radius: var(--r-sm);
    background: var(--warning-soft);
    font-size: 12.5px;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: var(--text-2);
  }
  .facts {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .facts span {
    font-size: 12px;
    padding: 2px 8px;
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--text-2);
  }
  .recent,
  .tasks {
    margin: 0;
    padding-left: 18px;
    font-size: 13px;
    color: var(--text-2);
    line-height: 1.6;
  }
  .keys {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    font-size: 12px;
    color: var(--text-3);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .head h3 {
    margin: 0;
    font-size: 14px;
  }
  .spacer {
    flex: 1;
  }
  .rag {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 12.5px;
    color: var(--success);
  }
  textarea.input {
    resize: vertical;
  }
</style>
