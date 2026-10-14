<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { PROJECT_COLORS } from '../lib/util';
  import type { CalendarEvent } from '../lib/types';
  import Modal from './Modal.svelte';
  import { ChevronLeft, ChevronRight, Plus, Clock, Trash2, CalendarDays } from '@lucide/svelte';

  const KEY = '/v1/app-data/calendar_events';

  let events = $state<CalendarEvent[]>([]);
  let loaded = $state(false);
  let cursor = $state(firstOfMonth(new Date()));
  let selected = $state(iso(new Date()));
  let today = $state(iso(new Date()));

  // Editor
  let editing = $state<CalendarEvent | null>(null);
  let editorOpen = $state(false);
  let allDay = $state(false);

  // Monday unless the locale says otherwise (weekInfo.firstDay: 1 = Mon … 7 = Sun).
  const weekStart = (() => {
    try {
      const l = new Intl.Locale(navigator.language) as any;
      const d = (l.getWeekInfo?.() ?? l.weekInfo)?.firstDay;
      return typeof d === 'number' ? d % 7 : 1;
    } catch {
      return 1;
    }
  })();
  const weekdays = Array.from({ length: 7 }, (_, i) => new Date(2024, 0, 7 + weekStart + i).toLocaleDateString(undefined, { weekday: 'short' }));

  function iso(d: Date) {
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
  }
  function parse(s: string) {
    const [y, m, d] = s.split('-').map(Number);
    return new Date(y, m - 1, d);
  }
  function firstOfMonth(d: Date) {
    return new Date(d.getFullYear(), d.getMonth(), 1);
  }
  function fmtTime(t: string | null) {
    if (!t) return '';
    const [h, m] = t.split(':').map(Number);
    return new Date(2000, 0, 1, h, m).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
  }
  function sortKey(e: CalendarEvent) {
    return `${e.date} ${e.start ?? '00:00'} ${e.title}`;
  }

  const byDay = $derived.by(() => {
    const m = new Map<string, CalendarEvent[]>();
    for (const e of [...events].sort((a, b) => sortKey(a).localeCompare(sortKey(b)))) {
      const list = m.get(e.date) ?? [];
      list.push(e);
      m.set(e.date, list);
    }
    // All-day events first, then by start time.
    for (const list of m.values()) list.sort((a, b) => (a.start ? 1 : 0) - (b.start ? 1 : 0));
    return m;
  });

  const grid = $derived.by(() => {
    const first = cursor;
    const offset = (first.getDay() - weekStart + 7) % 7;
    const start = new Date(first.getFullYear(), first.getMonth(), 1 - offset);
    const days = Math.ceil((offset + new Date(first.getFullYear(), first.getMonth() + 1, 0).getDate()) / 7) * 7;
    return Array.from({ length: days }, (_, i) => {
      const d = new Date(start.getFullYear(), start.getMonth(), start.getDate() + i);
      return { date: iso(d), day: d.getDate(), inMonth: d.getMonth() === first.getMonth() };
    });
  });

  const monthLabel = $derived(cursor.toLocaleDateString(undefined, { month: 'long', year: 'numeric' }));
  const selectedLabel = $derived(parse(selected).toLocaleDateString(undefined, { weekday: 'long', month: 'long', day: 'numeric' }));
  const dayEvents = $derived(byDay.get(selected) ?? []);
  const upcoming = $derived(
    [...events].filter((e) => e.date > selected).sort((a, b) => sortKey(a).localeCompare(sortKey(b))).slice(0, 6),
  );

  onMount(() => {
    api
      .get<any>(KEY)
      .then((v) => (events = Array.isArray(v) ? v : []))
      .catch((e) => store.error(e, 'Couldn’t load the calendar'))
      .finally(() => (loaded = true));
    // Keep "today" right if the app stays open past midnight.
    const t = setInterval(() => (today = iso(new Date())), 60_000);
    return () => clearInterval(t);
  });

  async function save(list: CalendarEvent[]) {
    const prev = events;
    events = list;
    try {
      await api.put(KEY, list);
    } catch (e) {
      events = prev;
      store.error(e, 'Couldn’t save the calendar');
    }
  }

  function shift(months: number) {
    cursor = new Date(cursor.getFullYear(), cursor.getMonth() + months, 1);
  }
  function goToday() {
    const d = new Date();
    today = selected = iso(d);
    cursor = firstOfMonth(d);
  }
  function pick(date: string) {
    selected = date;
    const d = parse(date);
    if (d.getMonth() !== cursor.getMonth() || d.getFullYear() !== cursor.getFullYear()) cursor = firstOfMonth(d);
  }

  function newEvent(date = selected) {
    const now = new Date();
    const h = Math.min(now.getHours() + 1, 23);
    editing = { id: '', title: '', date, start: `${String(h).padStart(2, '0')}:00`, end: `${String(Math.min(h + 1, 23)).padStart(2, '0')}:${h === 23 ? '59' : '00'}`, notes: '', color: PROJECT_COLORS[0] };
    allDay = false;
    editorOpen = true;
  }
  function edit(e: CalendarEvent) {
    editing = { ...e, start: e.start ?? '09:00', end: e.end ?? '10:00' };
    allDay = !e.start;
    editorOpen = true;
  }

  const timeError = $derived(!!editing && !allDay && !!editing.start && !!editing.end && editing.end <= editing.start);

  async function commit(ev: Event) {
    ev.preventDefault();
    if (!editing || !editing.title.trim() || !editing.date || timeError) return;
    const e: CalendarEvent = {
      ...editing,
      id: editing.id || crypto.randomUUID(),
      title: editing.title.trim(),
      start: allDay ? null : editing.start || null,
      end: allDay || !editing.start ? null : editing.end || null,
      notes: editing.notes.trim(),
    };
    const exists = events.some((x) => x.id === e.id);
    editorOpen = false;
    await save(exists ? events.map((x) => (x.id === e.id ? e : x)) : [...events, e]);
    pick(e.date);
  }
  async function remove(e: CalendarEvent) {
    editorOpen = false;
    await save(events.filter((x) => x.id !== e.id));
    store.toast('info', 'Event deleted', e.title);
  }

  function onKey(e: KeyboardEvent) {
    if (editorOpen || (e.target as HTMLElement)?.closest('input, textarea, select, [contenteditable]')) return;
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === 'n') (e.preventDefault(), newEvent());
    else if (e.key === 't') goToday();
    else if (e.key === 'ArrowLeft') pick(iso(new Date(parse(selected).getTime() - 864e5)));
    else if (e.key === 'ArrowRight') pick(iso(new Date(parse(selected).getTime() + 864e5)));
    else if (e.key === 'ArrowUp') (e.preventDefault(), pick(iso(new Date(parse(selected).getTime() - 7 * 864e5))));
    else if (e.key === 'ArrowDown') (e.preventDefault(), pick(iso(new Date(parse(selected).getTime() + 7 * 864e5))));
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="cal">
  <header class="drag">
    <div class="nav no-drag">
      <button class="icon-btn" title="Previous month" onclick={() => shift(-1)}><ChevronLeft size={17} /></button>
      <button class="icon-btn" title="Next month" onclick={() => shift(1)}><ChevronRight size={17} /></button>
    </div>
    <h1>{monthLabel}</h1>
    <span class="spacer"></span>
    <button class="btn sm ghost no-drag" title="Today (T)" onclick={goToday}>Today</button>
    <button class="btn sm primary no-drag" title="New event (N)" onclick={() => newEvent()}><Plus size={14} /> New event</button>
  </header>

  <div class="main">
    <div class="month">
      <div class="wk">
        {#each weekdays as w}<div>{w}</div>{/each}
      </div>
      <div class="days" style="grid-template-rows:repeat({grid.length / 7}, 1fr)">
        {#each grid as d (d.date)}
          {@const list = byDay.get(d.date) ?? []}
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
          <div class="day" class:out={!d.inMonth} class:sel={d.date === selected} onclick={() => pick(d.date)} ondblclick={() => newEvent(d.date)}>
            <span class="num" class:today={d.date === today}>{d.day}</span>
            {#each list.slice(0, 3) as e (e.id)}
              <button class="chip" style="--c:{e.color}" title={`${e.start ? fmtTime(e.start) + ' ' : ''}${e.title}`} onclick={(ev) => (ev.stopPropagation(), pick(d.date), edit(e))}>
                {#if e.start}<span class="t">{fmtTime(e.start)}</span>{/if}
                <span class="truncate">{e.title}</span>
              </button>
            {/each}
            {#if list.length > 3}<span class="more">+{list.length - 3} more</span>{/if}
          </div>
        {/each}
      </div>
    </div>

    <aside class="agenda">
      <div class="ah">
        <strong>{selectedLabel}</strong>
        <button class="icon-btn sm" title="Add event on this day" onclick={() => newEvent()}><Plus size={14} /></button>
      </div>
      {#each dayEvents as e (e.id)}
        <button class="ev" style="--c:{e.color}" onclick={() => edit(e)}>
          <span class="bar"></span>
          <span class="body">
            <span class="et truncate">{e.title}</span>
            <span class="when"><Clock size={12} /> {e.start ? `${fmtTime(e.start)}${e.end ? ' – ' + fmtTime(e.end) : ''}` : 'All day'}</span>
            {#if e.notes}<span class="notes">{e.notes}</span>{/if}
          </span>
        </button>
      {:else}
        <div class="empty">
          <CalendarDays size={28} strokeWidth={1.4} />
          <span>{loaded ? 'Nothing planned.' : 'Loading…'}</span>
          {#if loaded}<button class="btn sm" onclick={() => newEvent()}><Plus size={14} /> Add an event</button>{/if}
        </div>
      {/each}

      {#if upcoming.length}
        <div class="section-title up">Coming up</div>
        {#each upcoming as e (e.id)}
          <button class="ev small" style="--c:{e.color}" onclick={() => (pick(e.date), edit(e))}>
            <span class="bar"></span>
            <span class="body">
              <span class="et truncate">{e.title}</span>
              <span class="when">{parse(e.date).toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric' })}{e.start ? ' · ' + fmtTime(e.start) : ''}</span>
            </span>
          </button>
        {/each}
      {/if}
    </aside>
  </div>
</div>

<Modal bind:open={editorOpen} title={editing?.id ? 'Edit event' : 'New event'} width={440}>
  {#if editing}
    <form onsubmit={commit}>
      <div class="field">
        <label class="label" for="ev-title">Title</label>
        <!-- svelte-ignore a11y_autofocus -->
        <input id="ev-title" class="input" bind:value={editing.title} placeholder="e.g. Dentist, Call with Sam" autofocus />
      </div>
      <div class="field">
        <label class="label" for="ev-date">Date</label>
        <input id="ev-date" class="input" type="date" bind:value={editing.date} />
      </div>
      <label class="check"><input type="checkbox" bind:checked={allDay} /> All day</label>
      {#if !allDay}
        <div class="times">
          <div class="field">
            <label class="label" for="ev-start">Starts</label>
            <input id="ev-start" class="input" type="time" bind:value={editing.start} />
          </div>
          <div class="field">
            <label class="label" for="ev-end">Ends</label>
            <input id="ev-end" class="input" type="time" bind:value={editing.end} />
          </div>
        </div>
        {#if timeError}<div class="terr">The end time must be after the start.</div>{/if}
      {/if}
      <div class="field">
        <label class="label" for="ev-notes">Notes</label>
        <textarea id="ev-notes" class="textarea" rows="3" bind:value={editing.notes} placeholder="Optional"></textarea>
      </div>
      <div class="field">
        <span class="label">Color</span>
        <div class="swatches">
          {#each PROJECT_COLORS as c}
            <button type="button" class="sw" class:sel={editing.color === c} style="background:{c}" onclick={() => (editing!.color = c)} aria-label={c}></button>
          {/each}
        </div>
      </div>
      <div class="row" style="justify-content:flex-end">
        {#if editing.id}
          <button type="button" class="btn ghost danger" onclick={() => remove(editing!)}><Trash2 size={14} /> Delete</button>
          <span class="spacer"></span>
        {/if}
        <button type="button" class="btn ghost" onclick={() => (editorOpen = false)}>Cancel</button>
        <button class="btn primary" disabled={!editing.title.trim() || !editing.date || timeError}>{editing.id ? 'Save' : 'Add event'}</button>
      </div>
    </form>
  {/if}
</Modal>

<style>
  .cal {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  header {
    flex: none;
    display: flex;
    align-items: center;
    gap: 10px;
    height: var(--titlebar-h);
    padding: 0 12px 0 calc(14px + var(--lead));
    border-bottom: 1px solid var(--border);
  }
  .nav {
    display: flex;
    gap: 2px;
  }
  h1 {
    margin: 0;
    font-size: 15px;
    font-weight: 650;
  }
  .main {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .month {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    padding: 8px 10px 10px;
  }
  .wk {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    padding: 0 0 6px;
  }
  .wk div {
    padding: 0 8px;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--text-3);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .days {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: 1px;
    background: var(--border);
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    overflow: hidden;
  }
  .day {
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 4px;
    background: var(--bg);
    overflow: hidden;
    cursor: default;
    transition: background var(--t-fast);
  }
  .day:hover {
    background: var(--surface-hover);
  }
  .day.out {
    background: color-mix(in oklab, var(--bg) 92%, var(--text) 8%);
  }
  .day.out .num {
    color: var(--text-4);
  }
  .day.sel {
    background: color-mix(in oklab, var(--accent) 10%, var(--bg));
    box-shadow: inset 0 0 0 1.5px var(--accent);
  }
  .num {
    align-self: flex-start;
    min-width: 22px;
    height: 22px;
    padding: 0 5px;
    display: grid;
    place-items: center;
    border-radius: 999px;
    font-size: 12px;
    font-weight: 550;
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }
  .num.today {
    background: var(--accent);
    color: #fff;
  }
  .chip {
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    height: 19px;
    padding: 0 5px;
    border: none;
    border-radius: 4px;
    background: color-mix(in oklab, var(--c) 18%, transparent);
    color: var(--text);
    font-size: 11.5px;
    text-align: left;
    flex: none;
  }
  .chip:hover {
    background: color-mix(in oklab, var(--c) 30%, transparent);
  }
  .chip .t {
    flex: none;
    color: color-mix(in oklab, var(--c) 70%, var(--text));
    font-variant-numeric: tabular-nums;
  }
  .more {
    padding: 0 5px;
    font-size: 11px;
    color: var(--text-3);
  }
  .agenda {
    flex: none;
    width: 280px;
    padding: 14px 12px;
    border-left: 1px solid var(--border);
    overflow-y: auto;
  }
  .ah {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 10px;
    font-size: 13.5px;
  }
  .ev {
    display: flex;
    gap: 10px;
    width: 100%;
    margin-bottom: 6px;
    padding: 8px 10px 8px 8px;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    background: var(--surface);
    color: var(--text);
    text-align: left;
    transition: border-color var(--t-fast), background var(--t-fast);
  }
  .ev:hover {
    border-color: var(--border-strong);
    background: var(--surface-hover);
  }
  .bar {
    flex: none;
    width: 4px;
    border-radius: 2px;
    background: var(--c);
  }
  .body {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .et {
    font-size: 13.5px;
    font-weight: 550;
  }
  .when {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 12px;
    color: var(--text-3);
  }
  .notes {
    font-size: 12.5px;
    color: var(--text-2);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .ev.small {
    padding: 6px 8px 6px 6px;
    border-color: transparent;
    background: transparent;
  }
  .ev.small:hover {
    background: var(--surface-hover);
  }
  .ev.small .et {
    font-size: 13px;
  }
  .up {
    margin: 18px 0 6px;
  }
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 24px 0;
    color: var(--text-3);
    font-size: 13px;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: -4px 0 12px;
    font-size: 13px;
    color: var(--text-2);
  }
  .times {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }
  .terr {
    margin: -6px 0 10px;
    font-size: 12px;
    color: var(--danger);
  }
  .swatches {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .sw {
    width: 22px;
    height: 22px;
    border-radius: 7px;
    border: 2px solid transparent;
    box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.1);
  }
  .sw.sel {
    border-color: var(--surface);
    box-shadow: 0 0 0 2px var(--text);
  }
</style>
