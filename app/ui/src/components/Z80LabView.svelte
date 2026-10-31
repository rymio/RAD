<script lang="ts">
  // Games → Z80 Lab: the circuit on the left (lib/z80lab.svelte.ts, CircuitCanvas), the program,
  // terminal, CPU and problems on the right, and Gilgamesh underneath.
  import { onMount, tick } from 'svelte';
  import { ask as confirmDialog } from '@tauri-apps/plugin-dialog';
  import { store } from '../lib/state.svelte';
  import { lab, CLOCKS } from '../lib/z80lab.svelte';
  import { PROGRAMS } from '../lib/z80lab/templates';
  import { hex } from '../lib/z80lab/sim';
  import { timeAgo } from '../lib/util';
  import { gilgamesh } from '../lib/gilgamesh/gilgamesh.svelte';
  import CircuitCanvas from './CircuitCanvas.svelte';
  import GilgameshChat from './GilgameshChat.svelte';
  import Menu from './Menu.svelte';
  import { Cpu, Plus, ArrowLeft, Play, Square, StepForward, RotateCcw, Power, Hammer, Loader, CircleAlert, TriangleAlert, Info, Trash2, Pencil, MoreHorizontal, Terminal, FileCode, Activity, CircleHelp, Wrench, Zap } from '@lucide/svelte';

  type Tab = 'program' | 'terminal' | 'cpu' | 'problems';
  let tab = $state<Tab>('program');
  let editor = $state<HTMLTextAreaElement | null>(null);
  let termEl = $state<HTMLDivElement | null>(null);
  let scrollTop = $state(0);
  let typed = $state('');
  const LH = 18;

  const p = $derived(lab.project);
  const lines = $derived((p?.source ?? '').split('\n'));
  const errLines = $derived(new Set((lab.asm?.errors ?? []).filter((e) => e.kind !== 'warning').map((e) => e.line)));
  const errors = $derived(lab.problems.filter((x) => x.level === 'error').length);
  const warnings = $derived(lab.problems.filter((x) => x.level === 'warning').length);
  const dirty = $derived(!!p && lab.burnedSource !== null && lab.burnedSource !== p.source);

  onMount(() => {
    lab.load().catch((e) => store.error(e, 'Couldn’t load your circuits'));
    return () => lab.stop();
  });

  // The terminal follows what arrives.
  $effect(() => {
    void lab.terminal;
    tick().then(() => termEl?.scrollTo({ top: termEl.scrollHeight }));
  });

  // The line being run stays in view.
  $effect(() => {
    const l = lab.line;
    if (!l || !editor || tab !== 'program') return;
    const top = (l - 1) * LH;
    if (top < editor.scrollTop || top > editor.scrollTop + editor.clientHeight - LH * 2) editor.scrollTop = Math.max(0, top - editor.clientHeight / 3);
  });

  function editKey(e: KeyboardEvent) {
    if (e.key === 'Tab') {
      e.preventDefault();
      const el = e.currentTarget as HTMLTextAreaElement;
      const [a, b] = [el.selectionStart, el.selectionEnd];
      // Spaces to the next tab stop (every 8 columns, as assembler is laid out).
      const n = 8 - ((a - el.value.lastIndexOf('\n', a - 1) - 1) % 8);
      lab.setSource(el.value.slice(0, a) + ' '.repeat(n) + el.value.slice(b));
      tick().then(() => el.setSelectionRange(a + n, a + n));
    } else if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
      e.preventDefault();
      void lab.run();
    }
  }

  async function loadProgram(key: string) {
    if (!p || !key) return;
    if (p.source.trim() && !(await confirmDialog(`Replace the program with “${PROGRAMS[key].label}”?`, { title: 'Load an example', kind: 'warning' }))) return;
    lab.setSource(PROGRAMS[key].source);
    tab = 'program';
  }

  function termKey(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey) return;
    if (e.key.length === 1) lab.type(e.key);
    else if (e.key === 'Enter') lab.type('\r');
    else if (e.key === 'Backspace') lab.type('\b');
    else return;
    e.preventDefault();
  }

  async function remove(id: string, title: string) {
    if (await confirmDialog(`Delete “${title}”? The circuit and its program are deleted.`, { title: 'Delete circuit', kind: 'warning' })) await lab.remove(id);
  }

  function rename() {
    const t = window.prompt('Name this computer', p?.title);
    if (t) lab.rename(t);
  }

  const flags = (f: number) => [['S', 0x80], ['Z', 0x40], ['H', 0x10], ['P/V', 0x04], ['N', 0x02], ['C', 0x01]].map(([n, b]) => ({ n: n as string, on: !!(f & (b as number)) }));
  const fmtHz = (h: number) => (h >= 1e6 ? `${(h / 1e6).toFixed(2)} MHz` : h >= 1e3 ? `${(h / 1e3).toFixed(1)} kHz` : `${Math.round(h)} Hz`);

  async function discuss() {
    if (p) await gilgamesh.discussCircuit(p.id);
  }
</script>

<div class="lab">
  <header class="drag">
    {#if p}
      <button class="icon-btn no-drag" title="Your circuits" onclick={() => lab.close()}><ArrowLeft size={16} /></button>
      <h1 class="truncate">{p.title}</h1>
      <span class="spacer"></span>
      <button class="btn sm no-drag pwrbtn" class:on={lab.power} title={lab.power ? 'Switch the power off' : 'Switch the power on'} onclick={() => lab.setPower(!lab.power)}><Power size={13} /> {lab.power ? 'On' : 'Off'}</button>
      <select class="input sm no-drag clock" title="The clock. Slow it down to watch every bus cycle travel the wires" value={String(lab.clock)} onchange={(e) => lab.setClock(Number(e.currentTarget.value))}>
        {#each CLOCKS as c (c.hz)}<option value={String(c.hz)}>{c.hz === 0 ? `Full speed${lab.oscHz ? ` (${fmtHz(lab.oscHz)})` : ''}` : c.label}</option>{/each}
      </select>
      {#if lab.running && !lab.watching && lab.speed}<span class="muted small speed" title="How fast the simulation really runs">{fmtHz(lab.speed)}</span>{/if}
      <button class="btn sm no-drag" title="Run one instruction and show its bus cycles" disabled={lab.running} onclick={() => lab.step()}><StepForward size={13} /> Step</button>
      <button class="btn sm no-drag" title="Reset (like pressing the reset button)" disabled={!lab.power} onclick={() => lab.reset()}><RotateCcw size={13} /> Reset</button>
      {#if lab.running}
        <button class="btn sm primary no-drag" onclick={() => lab.stop()}><Square size={13} /> Stop</button>
      {:else}
        <button class="btn sm primary no-drag" title="Assemble, burn the ROM, power on and run (⌘↵)" disabled={lab.assembling} onclick={() => lab.run()}>{#if lab.assembling}<Loader size={13} class="spin" />{:else}<Play size={13} />{/if} Run</button>
      {/if}
      <Menu
        items={[
          { label: 'Rename', icon: Pencil, onclick: rename },
          { label: 'New Easy Z80', icon: Cpu, onclick: () => lab.create('easy') },
          { label: 'New empty board', icon: Plus, onclick: () => lab.create('blank') },
          null,
          { label: 'Delete this circuit', icon: Trash2, danger: true, onclick: () => remove(p.id, p.title) },
        ]}
      >
        {#snippet trigger(toggle: () => void)}<button class="icon-btn no-drag" title="More" onclick={toggle}><MoreHorizontal size={16} /></button>{/snippet}
      </Menu>
    {:else}
      <Cpu size={17} />
      <h1>Z80 Lab</h1>
      <span class="muted small">build a Z80 computer and run it</span>
      <span class="spacer"></span>
    {/if}
  </header>

  {#if p}
    <div class="main">
      <CircuitCanvas />
      <div class="side">
        <div class="tabs">
          <button class:on={tab === 'program'} onclick={() => (tab = 'program')}><FileCode size={13} /> Program</button>
          <button class:on={tab === 'terminal'} onclick={() => (tab = 'terminal')}><Terminal size={13} /> Terminal</button>
          <button class:on={tab === 'cpu'} onclick={() => (tab = 'cpu')}><Activity size={13} /> CPU</button>
          <button class:on={tab === 'problems'} onclick={() => (tab = 'problems')}>
            {#if errors}<CircleAlert size={13} class="bad" />{:else if warnings}<TriangleAlert size={13} class="warn" />{:else}<Wrench size={13} />{/if} Problems{#if errors + warnings}<span class="count">{errors + warnings}</span>{/if}
          </button>
        </div>

        {#if tab === 'program'}
          <div class="ptools">
            <select class="input sm" value="" onchange={(e) => { void loadProgram(e.currentTarget.value); e.currentTarget.value = ''; }}>
              <option value="">Load an example…</option>
              {#each Object.entries(PROGRAMS) as [k, v] (k)}<option value={k}>{v.label}</option>{/each}
            </select>
            <span class="spacer"></span>
            {#if dirty}<span class="muted small">changed since burned</span>{/if}
            <button class="btn sm" disabled={lab.assembling} title="Assemble and burn it into the ROM" onclick={() => lab.assemble()}>{#if lab.assembling}<Loader size={13} class="spin" />{:else}<Hammer size={13} />{/if} Assemble & burn</button>
          </div>
          <div class="code">
            <div class="gutter" style="transform:translateY({-scrollTop}px)">
              {#each lines as _, i (i)}
                <div class="ln" class:cur={lab.line === i + 1} class:err={errLines.has(i + 1)}>{lab.line === i + 1 ? '▶' : i + 1}</div>
              {/each}
            </div>
            {#if lab.line}<div class="curline" style="top:{(lab.line - 1) * LH - scrollTop + 6}px"></div>{/if}
            <textarea
              class="src"
              bind:this={editor}
              spellcheck="false"
              value={p.source}
              oninput={(e) => lab.setSource(e.currentTarget.value)}
              onscroll={(e) => (scrollTop = e.currentTarget.scrollTop)}
              onkeydown={editKey}
            ></textarea>
          </div>
          {#if lab.asm && !lab.asm.ok}
            <div class="out bad">
              {#each lab.asm.errors as e, i (i)}<button class="eline" onclick={() => { if (e.line && editor) { editor.focus(); const at = lines.slice(0, e.line - 1).join('\n').length + (e.line > 1 ? 1 : 0); editor.setSelectionRange(at, at + (lines[e.line - 1]?.length ?? 0)); } }}>Line {e.line ?? '?'}: {e.message}</button>{:else}<pre>{lab.asm.output}</pre>{/each}
            </div>
          {:else if lab.burnNote}
            <div class="out">{lab.burnNote}</div>
          {/if}
        {:else if tab === 'terminal'}
          <div class="term" bind:this={termEl} role="textbox" aria-label="Serial terminal" tabindex="0" onkeydown={termKey} title="Click here and type: the keys go to the SIO">{lab.terminal}<span class="cursor">▌</span></div>
          <form class="send" onsubmit={(e) => { e.preventDefault(); lab.type(typed + '\r'); typed = ''; }}>
            <input class="input sm" bind:value={typed} placeholder="Type a line and press Enter (or click the terminal and type)" />
            <button class="btn sm" type="button" onclick={() => (lab.terminal = '')}>Clear</button>
          </form>
          {#if !p.parts.some((x) => x.type === 'terminal')}<p class="muted small pad">There’s no serial terminal on the board: add one and wire its RXD to the SIO’s TxDA, and TXD to RxDA.</p>{/if}
        {:else if tab === 'cpu'}
          <div class="cpu">
            {#if lab.regs}
              {@const r = lab.regs}
              <div class="regs">
                {#each [['PC', r.pc], ['SP', r.sp], ['AF', r.af], ['BC', r.bc], ['DE', r.de], ['HL', r.hl], ['IX', r.ix], ['IY', r.iy]] as [n, v] (n)}
                  <span class="rn">{n}</span><span class="rv">{hex(v as number, 4)}</span>
                {/each}
              </div>
              <div class="flags">{#each flags(r.af & 0xff) as f (f.n)}<span class:on={f.on}>{f.n}</span>{/each}</div>
              <p class="muted small">I {hex(r.i)} · IM {r.im} · interrupts {r.iff1 ? 'on' : 'off'}{r.halted ? ' · halted' : ''} · {r.t.toLocaleString()} T-states{lab.line ? ` · line ${lab.line}` : ''}</p>
            {:else}
              <p class="muted small">Switch on and run to see the registers.</p>
            {/if}
            <h4>Bus cycles</h4>
            <pre class="cycles">{lab.cycles.length ? lab.cycles.join('\n') : 'Slow the clock to 1 kHz or less (or press Step) to see each bus cycle: the address, the byte and the chip that answered.'}</pre>
            {#if lab.sim?.notes.length}
              <h4>What happened</h4>
              <ul class="notes">{#each lab.sim.notes.slice(-10) as n, i (i)}<li>{n}</li>{/each}</ul>
            {/if}
          </div>
        {:else}
          <div class="probs">
            {#each lab.problems as x, i (i)}
              <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
              <p class={x.level} onclick={() => x.parts && (lab.selected = x.parts)}>
                {#if x.level === 'error'}<CircleAlert size={13} />{:else if x.level === 'warning'}<TriangleAlert size={13} />{:else}<Info size={13} />{/if}
                <span>{x.text}</span>
              </p>
            {:else}
              <p class="ok"><Zap size={13} /> No problems found: every chip has power, the CPU has a clock and a reset, and no inputs float.</p>
            {/each}
          </div>
        {/if}
      </div>
    </div>
    {#key p.id}
      <GilgameshChat
        about={gilgamesh.circuit?.id === p.id}
        {discuss}
        storeKey="rad-z80lab-chat"
        placeholder="Ask Gilgamesh about the circuit or the program…"
        hint="Ask him anything about this computer: what a pin does, why a chip needs a pull-up, how the address decoder picks the ROM, what the program is doing, or why it doesn’t work. He sees the wiring, the problems, the registers and the terminal. He can build for you too: “add a 74HC138”, “wire U1 CLK to QG1 OUT”, “slow the clock to 5 Hz and run”."
        chips={[
          { label: 'Why doesn’t it work?', icon: CircleHelp, ask: 'Why doesn’t my computer work? Look at the wiring and the problems and tell me what to fix first.' },
          { label: 'What’s happening now?', icon: Activity, ask: 'What is the CPU doing right now, on the wires and in the program?' },
          { label: 'Explain this chip', icon: Cpu, ask: lab.selected.length ? 'Explain the selected chip: what it does here and what its important pins are for.' : 'Explain how this computer works, chip by chip.' },
        ]}
      />
    {/key}
  {:else if !lab.loaded}
    <div class="center"><Loader size={20} class="spin" /></div>
  {:else}
    <div class="list">
      <section class="intro">
        <div class="glyph"><Cpu size={28} /></div>
        <h2>Build a Z80 computer</h2>
        <p>Drag chips onto the board (a Z80, ROM, RAM, the SIO, a GAL, oscillators, resistors, a power supply), wire their pins, write a program in Z80 assembler and run it on the circuit you built. Slow the clock right down to watch every bus cycle travel the wires. Gilgamesh is underneath to explain, find the fault, or build with you.</p>
        <div class="starts">
          <button class="btn primary" onclick={() => lab.create('easy')}><Cpu size={14} /> Easy Z80, wired and ready</button>
          <button class="btn" onclick={() => lab.create('blank')}><Plus size={14} /> Empty board</button>
        </div>
        <p class="muted small">The Easy Z80 design is Sergey Kiselev’s single-board computer (github.com/skiselev/easy_z80), simplified for the lab.</p>
      </section>
      {#if lab.list.length}
        <h3 class="muted small">Your circuits</h3>
        <div class="items">
          {#each lab.list as x (x.id)}
            <div class="it">
              <button class="open" onclick={() => lab.open(x.id)}>
                <span class="name truncate">{x.title}</span>
                <span class="muted small">{x.parts.length} parts · {x.wires.length} wires · {timeAgo(x.updated)}</span>
              </button>
              <button class="icon-btn sm" title="Delete" onclick={() => remove(x.id, x.title)}><Trash2 size={13} /></button>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .lab {
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
  .small {
    font-size: 12px;
  }
  .spacer {
    flex: 1;
  }
  .pwrbtn.on {
    color: #16a34a;
    border-color: #16a34a;
  }
  .clock {
    width: auto;
  }
  .speed {
    min-width: 64px;
  }
  .main {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .side {
    width: 430px;
    flex: none;
    display: flex;
    flex-direction: column;
    border-left: 1px solid var(--border);
    min-height: 0;
    background: var(--surface);
  }
  .tabs {
    display: flex;
    gap: 2px;
    padding: 6px 8px 0;
    border-bottom: 1px solid var(--border);
  }
  .tabs button {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 6px 10px;
    font-size: 12.5px;
    border: 0;
    border-bottom: 2px solid transparent;
    background: none;
    color: var(--text-2);
    cursor: pointer;
  }
  .tabs button.on {
    color: var(--text);
    border-bottom-color: var(--accent);
  }
  .tabs :global(.bad) {
    color: #dc2626;
  }
  .tabs :global(.warn) {
    color: #d97706;
  }
  .count {
    font-size: 10.5px;
    padding: 0 5px;
    border-radius: 999px;
    background: var(--surface-2);
  }
  .ptools {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px;
  }
  .ptools select {
    width: auto;
    max-width: 210px;
  }
  .code {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: hidden;
    border-top: 1px solid var(--border);
    background: var(--bg, var(--surface));
  }
  .gutter {
    position: absolute;
    left: 0;
    top: 6px;
    width: 38px;
    text-align: right;
    font: 11px/18px var(--font-mono, monospace);
    color: var(--text-3);
    pointer-events: none;
    user-select: none;
  }
  .ln {
    padding-right: 6px;
    height: 18px;
  }
  .ln.cur {
    color: #ca8a04;
    font-weight: 700;
  }
  .ln.err {
    color: #dc2626;
    font-weight: 700;
  }
  .curline {
    position: absolute;
    left: 40px;
    right: 0;
    height: 18px;
    background: rgba(250, 204, 21, 0.22);
    pointer-events: none;
  }
  .src {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    padding: 6px 8px 6px 46px;
    border: 0;
    outline: none;
    resize: none;
    background: transparent;
    color: var(--text);
    font: 12.5px/18px var(--font-mono, monospace);
    tab-size: 8;
    white-space: pre;
    overflow: auto;
  }
  .out {
    max-height: 120px;
    overflow: auto;
    padding: 8px 10px;
    border-top: 1px solid var(--border);
    font-size: 12px;
    color: var(--text-2);
  }
  .out.bad {
    color: #dc2626;
  }
  .out pre {
    margin: 0;
    white-space: pre-wrap;
  }
  .eline {
    display: block;
    border: 0;
    background: none;
    color: inherit;
    text-align: left;
    padding: 1px 0;
    cursor: pointer;
    font: inherit;
  }
  .term {
    flex: 1;
    min-height: 0;
    margin: 8px;
    padding: 10px;
    overflow: auto;
    border-radius: var(--r-sm);
    background: #0b0f0c;
    color: #4ade80;
    font: 13px/1.4 var(--font-mono, monospace);
    white-space: pre-wrap;
    word-break: break-all;
    outline: none;
  }
  .term:focus {
    box-shadow: 0 0 0 2px var(--accent-ring);
  }
  .cursor {
    animation: blink 1s steps(1) infinite;
  }
  @keyframes blink {
    50% {
      opacity: 0;
    }
  }
  .send {
    display: flex;
    gap: 6px;
    padding: 0 8px 8px;
  }
  .send input {
    flex: 1;
  }
  .pad {
    padding: 0 10px;
  }
  .cpu {
    flex: 1;
    overflow: auto;
    padding: 10px;
  }
  .regs {
    display: grid;
    grid-template-columns: 28px 1fr 28px 1fr;
    gap: 4px 8px;
    font: 13px var(--font-mono, monospace);
  }
  .rn {
    color: var(--text-3);
  }
  .flags {
    display: flex;
    gap: 6px;
    margin: 10px 0 6px;
    font: 12px var(--font-mono, monospace);
  }
  .flags span {
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--surface-2);
    color: var(--text-3);
  }
  .flags span.on {
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 700;
  }
  h4 {
    margin: 14px 0 6px;
    font-size: 12px;
    color: var(--text-3);
  }
  .cycles {
    margin: 0;
    font: 11.5px/1.5 var(--font-mono, monospace);
    white-space: pre-wrap;
    color: var(--text-2);
  }
  .notes {
    margin: 0;
    padding-left: 16px;
    font-size: 12px;
    color: var(--text-2);
  }
  .probs {
    flex: 1;
    overflow: auto;
    padding: 8px 10px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .probs p {
    display: flex;
    gap: 7px;
    align-items: flex-start;
    margin: 0;
    padding: 7px 9px;
    border-radius: var(--r-sm);
    background: var(--surface-2);
    font-size: 12.5px;
    line-height: 1.45;
    cursor: default;
  }
  .probs p :global(svg) {
    flex: none;
    margin-top: 2px;
  }
  .probs .error :global(svg) {
    color: #dc2626;
  }
  .probs .warning :global(svg) {
    color: #d97706;
  }
  .probs .info :global(svg) {
    color: var(--text-3);
  }
  .probs .ok :global(svg) {
    color: #16a34a;
  }
  .center {
    flex: 1;
    display: grid;
    place-items: center;
  }
  .list {
    flex: 1;
    overflow: auto;
    padding: 28px 24px;
    max-width: 680px;
    width: 100%;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .intro {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 10px;
  }
  .intro h2 {
    margin: 4px 0 0;
    font-size: 19px;
  }
  .intro p {
    margin: 0;
    line-height: 1.55;
    color: var(--text-2);
  }
  .glyph {
    width: 56px;
    height: 56px;
    border-radius: 16px;
    display: grid;
    place-items: center;
    color: var(--accent);
    background: var(--accent-soft);
  }
  .starts {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  h3 {
    margin: 14px 0 0;
  }
  .items {
    border: 1px solid var(--border);
    border-radius: var(--r);
    overflow: hidden;
  }
  .it {
    display: flex;
    align-items: center;
    padding-right: 8px;
  }
  .it + .it {
    border-top: 1px solid var(--border);
  }
  .open {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    padding: 10px 14px;
    border: 0;
    background: none;
    color: var(--text);
    text-align: left;
    cursor: pointer;
  }
  .open:hover {
    background: var(--surface-2);
  }
  .name {
    font-size: 13.5px;
    font-weight: 550;
    max-width: 100%;
  }
</style>
