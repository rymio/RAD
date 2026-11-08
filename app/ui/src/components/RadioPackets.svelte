<script lang="ts">
  // The packet analyser, Wireshark-style: every Mode S frame heard on 1090 MHz in a list
  // (coloured by kind, filtered with expressions like `df==17 && kind==position`), the selected
  // one decoded field by field, and its bytes and bits with the field under the pointer lit up.
  import { tick } from 'svelte';
  import { radio, type Packet, type Field } from '../lib/radio.svelte';
  import { Pause, Play, Trash2, Download, ChevronRight, ChevronDown, Filter, X, ArrowDownToLine } from '@lucide/svelte';

  const ROW = 22;
  let filterText = $state('');
  let scroller = $state<HTMLDivElement | null>(null);
  let scrollTop = $state(0);
  let viewH = $state(400);
  let follow = $state(true);
  let hoverField = $state<Field | null>(null);
  let pinField = $state<Field | null>(null);
  let closed = $state<Record<string, boolean>>({});

  /** A display filter: clauses joined by && (or "and"); a clause is `field op value` or words. */
  function compile(src: string): { test: (p: Packet) => boolean; error: string | null } {
    const t = src.trim();
    if (!t) return { test: () => true, error: null };
    const clauses = t.split(/\s*(?:&&|\band\b)\s*/i).filter(Boolean);
    const tests: ((p: Packet) => boolean)[] = [];
    for (const c of clauses) {
      const m = c.match(/^(\w+)\s*(==|!=|>=|<=|>|<|~|contains)\s*"?([^"]*)"?$/i);
      if (!m) {
        const w = c.toLowerCase();
        tests.push((p) => `${p.icao} ${p.info} ${p.hex} ${p.kind} df${p.df}`.toLowerCase().includes(w));
        continue;
      }
      const [, field, op, raw] = m;
      const get: Record<string, (p: Packet) => string | number> = {
        df: (p) => p.df,
        icao: (p) => p.icao,
        kind: (p) => p.kind,
        type: (p) => p.kind,
        crc: (p) => p.crc,
        signal: (p) => p.signal,
        n: (p) => p.n,
        no: (p) => p.n,
        len: (p) => p.hex.length * 4,
        info: (p) => p.info,
        hex: (p) => p.hex,
        tc: (p) => (p.df === 17 || p.df === 18 ? parseInt(p.hex.slice(8, 10), 16) >> 3 : -1),
      };
      const g = get[field.toLowerCase()];
      if (!g) return { test: () => true, error: `Unknown field “${field}” (df, icao, kind, tc, crc, signal, len, info, hex, n)` };
      const num = Number(raw);
      const isNum = raw.trim() !== '' && Number.isFinite(num);
      tests.push((p) => {
        const v = g(p);
        if (op === '~' || op.toLowerCase() === 'contains') return String(v).toLowerCase().includes(raw.toLowerCase());
        if (typeof v === 'number' && isNum) return op === '==' ? v === num : op === '!=' ? v !== num : op === '>' ? v > num : op === '<' ? v < num : op === '>=' ? v >= num : v <= num;
        const a = String(v).toLowerCase();
        const b = raw.toLowerCase();
        return op === '==' ? a === b : op === '!=' ? a !== b : op === '>' ? a > b : op === '<' ? a < b : op === '>=' ? a >= b : a <= b;
      });
    }
    return { test: (p) => tests.every((f) => f(p)), error: null };
  }

  const filter = $derived(compile(filterText));
  const shown = $derived(filterText.trim() ? radio.packets.filter(filter.test) : radio.packets);
  const first = $derived(Math.max(0, Math.floor(scrollTop / ROW) - 5));
  const visible = $derived(shown.slice(first, first + Math.ceil(viewH / ROW) + 10));
  const sel = $derived(radio.packets.find((p) => p.n === radio.selectedPacket) ?? null);
  const groups = $derived.by(() => {
    if (!sel) return [];
    const g = new Map<string, Field[]>();
    for (const f of sel.fields) g.set(f.group, [...(g.get(f.group) ?? []), f]);
    return [...g.entries()];
  });
  const bytes = $derived(sel ? (sel.hex.match(/../g) ?? []) : []);
  const lit = $derived(hoverField ?? pinField);

  $effect(() => {
    void shown.length;
    if (follow && scroller) tick().then(() => scroller && (scroller.scrollTop = scroller.scrollHeight));
  });

  function onScroll() {
    if (!scroller) return;
    scrollTop = scroller.scrollTop;
    follow = scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - ROW * 2;
  }

  function pick(p: Packet) {
    radio.selectedPacket = p.n === radio.selectedPacket ? null : p.n;
    pinField = null;
  }

  function keys(e: KeyboardEvent) {
    if (!sel || (e.key !== 'ArrowDown' && e.key !== 'ArrowUp')) return;
    e.preventDefault();
    const i = shown.findIndex((p) => p.n === sel.n);
    const j = Math.max(0, Math.min(shown.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1)));
    radio.selectedPacket = shown[j].n;
    follow = false;
    if (scroller) {
      const y = j * ROW;
      if (y < scroller.scrollTop) scroller.scrollTop = y;
      else if (y > scroller.scrollTop + scroller.clientHeight - ROW) scroller.scrollTop = y - scroller.clientHeight + ROW;
    }
  }

  function inField(bit: number, f: Field | null) {
    return !!f && bit >= f.start && bit < f.start + f.len;
  }
  function byteLit(i: number) {
    const f = lit;
    if (!f) return false;
    return i * 8 + 8 >= f.start && i * 8 + 1 < f.start + f.len;
  }

  const time = (t: number) => new Date(t).toLocaleTimeString(undefined, { hour12: false }) + '.' + String(t % 1000).padStart(3, '0');
  const start = $derived(radio.packets[0]?.t ?? 0);
</script>

<svelte:window onkeydown={keys} />

<div class="pk">
  <div class="controls">
    {#if radio.mode !== 'adsb'}
      <button class="btn sm primary" onclick={() => radio.radar().then(() => (radio.tab = 'packets'))}><Play size={12} /> Capture on 1090 MHz</button>
    {:else}
      <button class="btn sm" onclick={() => (radio.paused = !radio.paused)} title={radio.paused ? 'Resume capturing' : 'Pause (frames go on being decoded for the radar)'}>
        {#if radio.paused}<Play size={12} /> Resume{:else}<Pause size={12} /> Pause{/if}
      </button>
    {/if}
    <div class="filter" class:bad={!!filter.error}>
      <Filter size={13} />
      <input class="mono" placeholder="Display filter: df==17 && kind==position, icao==4840D6, signal>-20, crc==corrected, tc==19, or words" bind:value={filterText} spellcheck="false" />
      {#if filterText}<button class="icon-btn sm" title="Clear" onclick={() => (filterText = '')}><X size={12} /></button>{/if}
    </div>
    <span class="muted small">{shown.length.toLocaleString()}{filterText ? ` of ${radio.packets.length.toLocaleString()}` : ''} packets</span>
    <button class="icon-btn sm" class:active={follow} title="Follow new packets" onclick={() => { follow = !follow; if (follow && scroller) scroller.scrollTop = scroller.scrollHeight; }}><ArrowDownToLine size={13} /></button>
    <button class="icon-btn sm" title="Save as AVR (*HEX; lines: dump1090, Virtual Radar Server…)" disabled={!radio.packets.length} onclick={() => radio.export('avr')}><Download size={13} /></button>
    <button class="btn sm ghost" disabled={!radio.packets.length} onclick={() => radio.export('json')}>JSON</button>
    <button class="icon-btn sm" title="Clear" disabled={!radio.packets.length} onclick={() => radio.clearPackets()}><Trash2 size={13} /></button>
  </div>
  {#if filter.error}<div class="ferr">{filter.error}</div>{/if}

  <div class="head">
    <span class="c-n">No.</span><span class="c-t">Time</span><span class="c-i">ICAO</span><span class="c-df">DF</span><span class="c-k">Type</span><span class="c-l">Len</span><span class="c-s">Signal</span><span class="c-info">Info</span>
  </div>
  <div class="list" bind:this={scroller} bind:clientHeight={viewH} onscroll={onScroll}>
    <div style:height="{shown.length * ROW}px" class="spacer-rows">
      {#each visible as p, i (p.n)}
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
        <div class="row k-{p.kind}" class:on={p.n === radio.selectedPacket} class:fixed={p.crc === 'corrected'} style:top="{(first + i) * ROW}px" onclick={() => pick(p)}>
          <span class="c-n">{p.n}</span>
          <span class="c-t" title={time(p.t)}>{((p.t - start) / 1000).toFixed(3)}</span>
          <span class="c-i">{p.icao}</span>
          <span class="c-df">{p.df}</span>
          <span class="c-k">{p.kind}</span>
          <span class="c-l">{p.hex.length / 2}</span>
          <span class="c-s">{p.signal.toFixed(1)}</span>
          <span class="c-info truncate">{p.info}</span>
        </div>
      {/each}
    </div>
    {#if !radio.packets.length}
      <div class="empty">{radio.mode === 'adsb' ? 'Waiting for frames on 1090 MHz… (with no aircraft in range, nothing comes; see the Aircraft tab)' : 'Start capturing to see the Mode S frames aircraft send.'}</div>
    {/if}
  </div>

  <div class="detail">
    {#if sel}
      <div class="tree">
        <div class="frame">Frame {sel.n}: {sel.hex.length * 4} bits ({sel.hex.length / 2} bytes) at {time(sel.t)}, signal {sel.signal.toFixed(1)} dBFS, parity {sel.crc === 'ok' ? 'good' : sel.crc === 'corrected' ? 'one bit corrected' : 'carries the address'}</div>
        {#each groups as [g, fields] (g)}
          <div class="grp">
            <button class="gh" onclick={() => (closed = { ...closed, [g]: !closed[g] })}>{#if closed[g]}<ChevronRight size={12} />{:else}<ChevronDown size={12} />{/if} {g === 'Mode S' ? 'Mode S (downlink)' : g}</button>
            {#if !closed[g]}
              {#each fields as f (f.name + f.start)}
                <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                <div class="fld" class:on={pinField === f} onmouseenter={() => (hoverField = f)} onmouseleave={() => (hoverField = null)} onclick={() => (pinField = pinField === f ? null : f)}>
                  <span class="fn">{f.name}:</span> <span class="fv">{f.value}</span> <span class="fb">bits {f.start}–{f.start + f.len - 1}</span>
                </div>
              {/each}
            {/if}
          </div>
        {/each}
      </div>
      <div class="hexpane">
        <div class="hex mono">
          {#each bytes as b, i (i)}
            {#if i % 8 === 0}<span class="off">{(i).toString(16).padStart(4, '0')}</span>{/if}
            <span class="byte" class:lit={byteLit(i)}>{b}</span>
            {#if i % 8 === 7}<br />{/if}
          {/each}
        </div>
        <div class="bits mono">
          {#each bytes as b, i (i)}
            <span class="bgrp">{#each parseInt(b, 16).toString(2).padStart(8, '0').split('') as bit, k (k)}<span class:lit={inField(i * 8 + k + 1, lit)}>{bit}</span>{/each}</span>
          {/each}
        </div>
        {#if lit}<div class="small muted">{lit.name}: bits {lit.start}–{lit.start + lit.len - 1} ({lit.len} bits)</div>{/if}
      </div>
    {:else}
      <div class="nosel muted small">Select a packet to see its fields, bytes and bits. ↑ ↓ move between packets. Ask Gilgamesh about one and he sees it too.</div>
    {/if}
  </div>
</div>

<style>
  .pk {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--surface);
  }
  .controls {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
  }
  .filter {
    flex: 1;
    min-width: 200px;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 8px;
    height: 28px;
    border-radius: var(--r-sm);
    border: 1px solid var(--border-strong);
    background: color-mix(in oklab, var(--success) 7%, var(--surface));
    color: var(--text-3);
  }
  .filter.bad {
    background: var(--danger-soft);
  }
  .filter input {
    flex: 1;
    border: 0;
    outline: 0;
    background: transparent;
    font-size: 12px;
    color: var(--text);
  }
  .ferr {
    padding: 3px 12px;
    font-size: 12px;
    color: var(--danger);
  }
  .head,
  .row {
    display: flex;
    align-items: center;
    gap: 0;
    font-size: 12px;
    height: 22px;
  }
  .head {
    background: var(--surface-2);
    border-bottom: 1px solid var(--border);
    color: var(--text-3);
    font-weight: 500;
    padding-right: 10px;
  }
  .head > span,
  .row > span {
    padding: 0 8px;
    white-space: nowrap;
  }
  .c-n {
    width: 64px;
    text-align: right;
  }
  .c-t {
    width: 84px;
    text-align: right;
  }
  .c-i {
    width: 70px;
  }
  .c-df {
    width: 36px;
  }
  .c-k {
    width: 74px;
  }
  .c-l {
    width: 40px;
  }
  .c-s {
    width: 60px;
    text-align: right;
  }
  .c-info {
    flex: 1;
    min-width: 0;
  }
  .row > span:not(.c-info) {
    font-family: var(--mono);
    font-size: 11.5px;
  }
  .list {
    flex: 1.3;
    min-height: 120px;
    overflow: auto;
    position: relative;
  }
  .spacer-rows {
    position: relative;
  }
  .row {
    position: absolute;
    left: 0;
    right: 0;
    cursor: default;
    color: #1d1d24;
  }
  /* Wireshark-like colouring by kind of message. */
  .k-ident {
    background: #e6dcf7;
  }
  .k-position {
    background: #daf0d8;
  }
  .k-velocity {
    background: #dae8f8;
  }
  .k-altitude {
    background: #f8f1d2;
  }
  .k-squawk {
    background: #fbe3cc;
  }
  .k-all-call {
    background: #ececef;
  }
  .k-comm-b {
    background: #d4f1ee;
  }
  .k-acas {
    background: #f8dbe7;
  }
  .k-status {
    background: #d9f3f8;
  }
  .row.fixed {
    font-style: italic;
  }
  .row.on {
    background: var(--accent) !important;
    color: var(--on-accent);
  }
  .empty {
    padding: 18px 12px;
    color: var(--text-3);
    font-size: 12.5px;
  }
  .detail {
    flex: 1;
    min-height: 140px;
    display: flex;
    border-top: 2px solid var(--border-strong);
  }
  .tree {
    flex: 1.2;
    min-width: 0;
    overflow: auto;
    padding: 6px 8px;
    font-size: 12px;
    user-select: text;
  }
  .frame {
    padding: 2px 4px 6px;
    color: var(--text-2);
  }
  .gh {
    display: flex;
    align-items: center;
    gap: 4px;
    border: 0;
    background: none;
    font: 600 12px var(--font);
    color: var(--text);
    padding: 3px 2px;
    cursor: pointer;
  }
  .fld {
    padding: 2px 6px 2px 22px;
    border-radius: 4px;
    cursor: pointer;
  }
  .fld:hover,
  .fld.on {
    background: var(--accent-soft);
  }
  .fn {
    color: var(--text-2);
  }
  .fv {
    font-weight: 500;
  }
  .fb {
    color: var(--text-4);
    font-size: 11px;
    margin-left: 4px;
  }
  .hexpane {
    flex: 1;
    min-width: 0;
    overflow: auto;
    padding: 8px 12px;
    border-left: 1px solid var(--border);
    font-size: 12px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .hex {
    line-height: 1.7;
  }
  .off {
    color: var(--text-4);
    margin-right: 10px;
  }
  .byte {
    padding: 1px 3px;
    border-radius: 3px;
  }
  .byte.lit,
  .bits .lit {
    background: var(--accent);
    color: var(--on-accent);
  }
  .bits {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    font-size: 11px;
    color: var(--text-2);
  }
  .bgrp span {
    padding: 0 0.5px;
  }
  .nosel {
    padding: 14px;
  }
</style>
