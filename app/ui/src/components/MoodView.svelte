<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte';
  import { invoke, Channel } from '@tauri-apps/api/core';
  import { transcripts } from '../lib/gilgamesh/transcripts.svelte';
  import { store } from '../lib/state.svelte';
  import { mood, iso, daysUntil, OUTCOMES, type MoodEvent, type Outcome } from '../lib/mood.svelte';
  import { gilgamesh } from '../lib/gilgamesh/gilgamesh.svelte';
  import { speech } from '../lib/speech.svelte';
  import { renderMarkdown } from '../lib/markdown';
  import Modal from './Modal.svelte';
  import ModelPicker from './ModelPicker.svelte';
  import { Smile, Lock, Heart, Plus, ChevronLeft, ChevronRight, SendHorizontal, Trash2, Brain, SlidersHorizontal, Volume2, VolumeX, Loader, Delete, MessageCirclePlus, X, Mic, MicOff } from '@lucide/svelte';

  // ---------------------------------------------------------------- PIN

  let pin = $state('');
  let first = $state<string | null>(null);
  let pinError = $state('');
  let checking = $state(false);
  let eraseOpen = $state(false);
  let now = $state(Date.now());
  const waiting = $derived(Math.max(0, Math.ceil((mood.waitUntil - now) / 1000)));

  const pinTitle = $derived(mood.state === 'none' ? (first === null ? 'Choose a PIN for My Mood' : 'Enter it once more') : 'Enter your PIN');

  function digit(d: string) {
    if (checking || waiting || pin.length >= 8) return;
    pinError = '';
    pin += d;
  }

  function back() {
    pin = pin.slice(0, -1);
    pinError = '';
  }

  async function enter() {
    if (mood.state === 'locked') return pin.length >= 4 && tryPin();
    if (pin.length < 4) return (pinError = 'At least 4 digits');
    if (first === null) {
      first = pin;
      pin = '';
      return;
    }
    if (pin !== first) {
      pinError = 'The PINs don’t match — start again';
      first = null;
      pin = '';
      return;
    }
    checking = true;
    try {
      await mood.create(pin);
      pin = '';
      first = null;
      await opened();
    } catch (e) {
      store.error(e, 'Couldn’t set up My Mood');
    } finally {
      checking = false;
    }
  }

  async function tryPin() {
    checking = true;
    try {
      if (await mood.unlock(pin)) {
        pin = '';
        await opened();
      } else {
        pin = '';
        pinError = mood.waitUntil > Date.now() ? '' : 'That’s not it';
      }
    } catch (e) {
      store.error(e, 'Couldn’t open My Mood');
    } finally {
      checking = false;
    }
  }

  /** Just opened: a talk older than six hours is closed and Gilgamesh starts a fresh one. */
  async function opened() {
    const t = mood.index.talks.find((x) => x.id === mood.index.talk);
    if (!t || Date.now() - t.updated > 6 * 3600_000 || !mood.talk.length) await mood.newTalk();
    scrollDown();
  }

  async function erase() {
    eraseOpen = false;
    pin = '';
    first = null;
    await mood.erase();
    store.toast('success', 'Erased', 'Everything in My Mood is gone.');
  }

  function onKey(e: KeyboardEvent) {
    if (mood.state !== 'locked' && mood.state !== 'none') return;
    if (eraseOpen) return;
    if (/^\d$/.test(e.key)) digit(e.key);
    else if (e.key === 'Backspace') back();
    else if (e.key === 'Enter') enter();
  }

  // ---------------------------------------------------------------- calendar

  let cursor = $state(new Date(new Date().getFullYear(), new Date().getMonth(), 1));
  let selected = $state(iso(new Date()));
  const today = iso(new Date());
  const weekdays = Array.from({ length: 7 }, (_, i) => new Date(2024, 0, 1 + i).toLocaleDateString(undefined, { weekday: 'narrow' }));

  const cells = $derived.by(() => {
    const start = new Date(cursor);
    start.setDate(1 - ((cursor.getDay() + 6) % 7));
    return Array.from({ length: 42 }, (_, i) => {
      const d = new Date(start);
      d.setDate(start.getDate() + i);
      return { date: iso(d), day: d.getDate(), inMonth: d.getMonth() === cursor.getMonth() };
    });
  });

  const eventsOn = (date: string) => mood.index.events.filter((e) => e.date === date).sort((a, b) => (a.time ?? '').localeCompare(b.time ?? ''));
  const maxWorry = (date: string) => Math.max(-1, ...eventsOn(date).map((e) => e.worry));
  const dayEvents = $derived(eventsOn(selected));
  const note = $derived(mood.day(selected));
  let noteText = $state('');
  $effect(() => {
    noteText = note?.text ?? '';
  });

  const MOODS = ['😞', '🙁', '😐', '🙂', '😄'];
  const longDate = (d: string) => new Date(d + 'T12:00').toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' });
  const relative = (d: string) => {
    const n = daysUntil(d);
    return n === 0 ? 'today' : n === 1 ? 'tomorrow' : n === -1 ? 'yesterday' : n > 0 ? `in ${n} days` : `${-n} days ago`;
  };

  function month(step: number) {
    cursor = new Date(cursor.getFullYear(), cursor.getMonth() + step, 1);
  }

  function saveNote(m: number | null = note?.mood ?? null) {
    if (noteText === (note?.text ?? '') && m === (note?.mood ?? null)) return;
    mood.saveDay({ date: selected, mood: m, text: noteText.trim() });
  }

  // Event editor
  let editing = $state<MoodEvent | null>(null);
  let editorOpen = $state(false);
  function edit(e: MoodEvent) {
    editing = { ...e };
    editorOpen = true;
  }
  async function saveEvent() {
    if (!editing || !editing.title.trim()) return;
    editing.title = editing.title.trim();
    editing.time ||= null;
    await mood.saveEvent($state.snapshot(editing));
    editorOpen = false;
  }
  async function removeEvent() {
    if (!editing) return;
    await mood.removeEvent(editing.id);
    editorOpen = false;
  }
  function settle(e: MoodEvent, o: Outcome) {
    mood.saveEvent({ ...$state.snapshot(e), outcome: o });
  }

  // ---------------------------------------------------------------- talk

  let draft = $state('');
  let list = $state<HTMLDivElement>();
  let memOpen = $state(false);
  let setOpen = $state(false);
  let newMemory = $state('');
  let oldPin = $state('');
  let newPin = $state('');

  const face = $derived(gilgamesh.settings.faces.find((f) => f.id === gilgamesh.settings.face) ?? gilgamesh.settings.faces[0]);

  async function scrollDown() {
    await tick();
    list?.scrollTo({ top: list.scrollHeight, behavior: 'smooth' });
  }

  async function send() {
    const t = draft.trim();
    if (!t || mood.thinking) return;
    draft = '';
    speech.stop();
    scrollDown();
    await mood.say(t);
    scrollDown();
  }

  // ---------------------------------------------------------------- voice

  // Talking out loud: speech to text as in Gilgamesh's own view. There's one microphone, so his
  // listening elsewhere (and a transcript being made) stops first.
  let mic = $state<'off' | 'starting' | 'on'>('off');
  let heard = $state('');
  let level = $state(0);
  let micPaused = false;

  async function startMic() {
    if (mic !== 'off') return;
    gilgamesh.stop();
    if (transcripts.recording) await transcripts.stop();
    mic = 'starting';
    const ch = new Channel<any>();
    ch.onmessage = onStt;
    try {
      await invoke('stt_listen', { onEvent: ch, paused: false });
      mic = 'on';
      micPaused = false;
    } catch (e) {
      mic = 'off';
      store.error(e, 'Couldn’t start the microphone (set up speech recognition in Gilgamesh’s view)');
    }
  }

  function stopMic() {
    if (mic !== 'off') invoke('stt_stop').catch(() => {});
    mic = 'off';
    heard = '';
    level = 0;
  }

  /** Through the speakers he'd hear himself: the microphone rests while he speaks. */
  function pauseMic(p: boolean) {
    if (mic !== 'on' || micPaused === p || gilgamesh.settings.headphones) return;
    micPaused = p;
    invoke('stt_pause', { paused: p }).catch(() => {});
  }

  async function onStt(ev: any) {
    if (ev.type === 'level') level = ev.rms;
    else if (ev.type === 'speech_start') {
      heard = '';
      speech.stop();
    } else if (ev.type === 'partial') heard = ev.text;
    else if (ev.type === 'cancelled') heard = '';
    else if (ev.type === 'final') {
      heard = '';
      const t = String(ev.text ?? '').trim();
      if (!t || mood.thinking) return;
      mood.touch();
      scrollDown();
      await mood.say(t, true);
      scrollDown();
    } else if (ev.type === 'error') {
      store.toast('error', 'Microphone', ev.message);
      stopMic();
    }
  }

  // His replies read aloud in his voice: when the owner wants it, and always while talking out loud.
  let spoken = 0;
  $effect(() => {
    const last = mood.talk.at(-1);
    if (!last || last.role !== 'assistant' || last.at <= spoken) return;
    spoken = last.at;
    if (!(mood.index.speak || mic === 'on') || !store.config) return;
    pauseMic(true);
    speech
      .speak(`mood-${last.at}`, last.text, { ...store.config, tts_voice: gilgamesh.settings.voice || 'en_male' })
      .catch(() => {})
      .finally(() => pauseMic(false));
  });
  // Locked (by hand or when idle): the microphone goes off too.
  $effect(() => {
    if (mood.state !== 'open') stopMic();
  });

  async function changePin() {
    if (!/^\d{4,8}$/.test(newPin)) return store.toast('error', 'The new PIN needs 4 to 8 digits');
    if (await mood.changePin(oldPin, newPin)) {
      store.toast('success', 'PIN changed');
      oldPin = newPin = '';
    } else store.toast('error', 'The current PIN isn’t right');
  }

  function onComposerKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      send();
    }
  }

  let clock: ReturnType<typeof setInterval>;
  onMount(() => {
    mood.load().catch((e) => store.error(e, 'Couldn’t load My Mood'));
    clock = setInterval(() => (now = Date.now()), 1000);
  });
  // Leaving the view locks it again.
  onDestroy(() => {
    clearInterval(clock);
    stopMic();
    speech.stop();
    mood.lock();
  });
</script>

<svelte:window onkeydown={onKey} onpointerdown={() => mood.state === 'open' && mood.touch()} />

<div class="mood">
  <header class="drag">
    <h1><Heart size={15} /> My Mood</h1>
    <span class="spacer"></span>
    {#if mood.state === 'open'}
      <button class="btn sm ghost no-drag" title="Start a fresh talk (this one is kept in his memory)" disabled={mood.thinking} onclick={() => mood.newTalk().then(scrollDown)}><MessageCirclePlus size={14} /> New talk</button>
      <button class="icon-btn no-drag" class:active={mood.index.speak} title={mood.index.speak ? 'Replies are read aloud — turn off' : 'Read his replies aloud'} onclick={() => { speech.stop(); mood.setSpeak(!mood.index.speak); }}>
        {#if mood.index.speak}<Volume2 size={17} />{:else}<VolumeX size={17} />{/if}
      </button>
      <button class="icon-btn no-drag" title="What Gilgamesh remembers" onclick={() => (memOpen = true)}><Brain size={17} /></button>
      <button class="icon-btn no-drag" title="Settings" onclick={() => (setOpen = true)}><SlidersHorizontal size={17} /></button>
      <button class="icon-btn no-drag" title="Lock My Mood" onclick={() => mood.lock()}><Lock size={17} /></button>
    {/if}
  </header>

  {#if mood.state === 'loading' || mood.erasing}
    <div class="center"><Loader size={20} class="spin" /></div>
  {:else if mood.state !== 'open'}
    <div class="center">
      <div class="pinbox">
        {#if face?.thumb}<img class="avatar" src={face.thumb} alt="" />{:else}<div class="avatar glyph"><Heart size={26} /></div>{/if}
        <h2>{pinTitle}</h2>
        {#if mood.state === 'none' && first === null}<p class="fine">Just between you and Gilgamesh. 4–8 digits; it also encrypts what’s kept here, so it can’t be recovered if forgotten.</p>{/if}
        <div class="dots" class:shake={!!pinError}>
          {#each Array(Math.max(4, pin.length)) as _, i}<i class:on={i < pin.length}></i>{/each}
        </div>
        <div class="msg">
          {#if waiting}Too many tries — wait {waiting}s{:else if checking}<Loader size={14} class="spin" />{:else}{pinError}{/if}
        </div>
        <div class="pad">
          {#each ['1', '2', '3', '4', '5', '6', '7', '8', '9'] as d}
            <button class="key" disabled={!!waiting} onclick={() => digit(d)}>{d}</button>
          {/each}
          <button class="key smile" title="" aria-label="Smile" onclick={() => (eraseOpen = true)}><Smile size={24} /></button>
          <button class="key" disabled={!!waiting} onclick={() => digit('0')}>0</button>
          {#if pin.length}
            <button class="key aux" aria-label="Delete digit" onclick={back}><Delete size={20} /></button>
          {:else}
            <span></span>
          {/if}
        </div>
        <button class="btn primary ok" disabled={pin.length < 4 || checking || !!waiting} onclick={enter}>{mood.state === 'none' ? (first === null ? 'Next' : 'Set PIN') : 'Open'}</button>
      </div>
    </div>
  {:else}
    <div class="main">
      <aside class="cal">
        <div class="month">
          <button class="icon-btn" onclick={() => month(-1)} aria-label="Previous month"><ChevronLeft size={16} /></button>
          <span>{cursor.toLocaleDateString(undefined, { month: 'long', year: 'numeric' })}</span>
          <button class="icon-btn" onclick={() => month(1)} aria-label="Next month"><ChevronRight size={16} /></button>
        </div>
        <div class="grid">
          {#each weekdays as w}<span class="wd">{w}</span>{/each}
          {#each cells as c (c.date)}
            {@const w = maxWorry(c.date)}
            {@const n = mood.day(c.date)}
            <button class="cell" class:out={!c.inMonth} class:today={c.date === today} class:sel={c.date === selected} onclick={() => (selected = c.date)}>
              {c.day}
              <span class="marks">
                {#if w >= 0}<i class="ev" style="--w:{w}"></i>{/if}
                {#if n}<i class="nt"></i>{/if}
              </span>
            </button>
          {/each}
        </div>

        {#if mood.unresolved.length}
          <div class="ask">
            {#each mood.unresolved.slice(0, 3) as e (e.id)}
              <div class="ask-item">
                <div class="q">How did <b>{e.title}</b> go? <span class="when">{relative(e.date)}</span></div>
                <div class="outs">
                  {#each Object.entries(OUTCOMES) as [k, label]}
                    <button class="chip" onclick={() => settle(e, k as Outcome)}>{label}</button>
                  {/each}
                </div>
              </div>
            {/each}
          </div>
        {/if}

        <div class="day">
          <div class="day-head">
            <div>
              <div class="d">{longDate(selected)}</div>
              <div class="when">{relative(selected)}</div>
            </div>
            <button class="btn sm ghost" onclick={() => edit(mood.newEvent(selected))}><Plus size={14} /> Event</button>
          </div>
          {#each dayEvents as e (e.id)}
            <button class="event" onclick={() => edit(e)}>
              <span class="worry" style="--w:{e.worry}">{e.worry}</span>
              <span class="et">{e.time ? `${e.time} · ` : ''}{e.title}</span>
              {#if e.outcome}<span class="out o-{e.outcome}">{OUTCOMES[e.outcome]}</span>{/if}
            </button>
          {:else}
            <div class="empty">Nothing planned.</div>
          {/each}

          {#if daysUntil(selected) <= 0}
            <div class="label">How was the day?</div>
            <div class="moods">
              {#each MOODS as m, i}
                <button class="mface" class:on={note?.mood === i + 1} onclick={() => saveNote(note?.mood === i + 1 ? null : i + 1)}>{m}</button>
              {/each}
            </div>
            <textarea rows="4" placeholder="What happened…" bind:value={noteText} onblur={() => saveNote()}></textarea>
          {/if}
        </div>
      </aside>

      <section class="talk">
        <div class="msgs" bind:this={list}>
          {#each mood.talk as m (m.at + m.role)}
            {#if m.role === 'user'}
              <div class="bubble you">{m.text}</div>
            {:else}
              <div class="row">
                {#if face?.thumb}<img class="mini" src={face.thumb} alt="" />{:else}<span class="mini glyph"><Heart size={13} /></span>{/if}
                <div class="bubble him">{@html renderMarkdown(m.text)}</div>
              </div>
            {/if}
          {/each}
          {#if mood.thinking}
            <div class="row">
              {#if face?.thumb}<img class="mini" src={face.thumb} alt="" />{:else}<span class="mini glyph"><Heart size={13} /></span>{/if}
              <div class="bubble him typing"><i></i><i></i><i></i></div>
            </div>
          {/if}
          {#if mood.error}<div class="err">{mood.error} <button class="btn sm ghost" onclick={() => (mood.error = null)}><X size={13} /></button></div>{/if}
        </div>
        {#if mic === 'on'}
          <div class="hearing" style="--lvl:{Math.min(1, level * 12)}"><i></i>{heard || (mood.thinking ? 'Thinking…' : speech.speaking ? 'Speaking…' : 'Listening — just talk')}</div>
        {/if}
        <div class="composer">
          <button class="icon-btn mic" class:live={mic === 'on'} disabled={mic === 'starting'} title={mic === 'on' ? 'Stop talking out loud' : 'Talk out loud'} onclick={() => (mic === 'off' ? startMic() : stopMic())}>
            {#if mic === 'starting'}<Loader size={18} class="spin" />{:else if mic === 'on'}<MicOff size={18} />{:else}<Mic size={18} />{/if}
          </button>
          <textarea rows="2" placeholder="What’s on your mind?" bind:value={draft} onkeydown={onComposerKey}></textarea>
          <button class="btn primary" disabled={!draft.trim() || mood.thinking} onclick={send} aria-label="Send"><SendHorizontal size={16} /></button>
        </div>
      </section>
    </div>
  {/if}
</div>

<Modal bind:open={eraseOpen} title="Erase My Mood?" width={420}>
  <p class="modal-p">Every talk, memory, event and note in My Mood will be overwritten and the vault compacted. This can’t be undone. Backups made earlier still hold the old copy, encrypted with the old PIN.</p>
  {#snippet footer()}
    <button class="btn ghost" onclick={() => (eraseOpen = false)}>Keep it</button>
    <button class="btn danger" onclick={erase}><Trash2 size={14} /> Erase everything</button>
  {/snippet}
</Modal>

<Modal bind:open={editorOpen} title={editing && mood.index.events.some((e) => e.id === editing?.id) ? 'Event' : 'New event'} width={460}>
  {#if editing}
    <div class="form">
      <input class="input" placeholder="What is it? (e.g. Release deadline)" bind:value={editing.title} />
      <div class="two">
        <input class="input" type="date" bind:value={editing.date} />
        <input class="input" type="time" value={editing.time ?? ''} oninput={(e) => editing && (editing.time = e.currentTarget.value || null)} />
      </div>
      <div class="label">How much is it on your mind? <b>{editing.worry}/10</b></div>
      <input type="range" min="0" max="10" bind:value={editing.worry} />
      <textarea class="input" rows="3" placeholder="What worries you about it?" bind:value={editing.notes}></textarea>
      {#if daysUntil(editing.date) <= 0}
        <div class="label">How did it go?</div>
        <div class="outs">
          {#each Object.entries(OUTCOMES) as [k, label]}
            <button class="chip" class:on={editing.outcome === k} onclick={() => editing && (editing.outcome = editing.outcome === k ? null : (k as Outcome))}>{label}</button>
          {/each}
        </div>
        <textarea class="input" rows="2" placeholder="Looking back…" bind:value={editing.reflection}></textarea>
      {/if}
    </div>
  {/if}
  {#snippet footer()}
    {#if editing && mood.index.events.some((e) => e.id === editing?.id)}<button class="btn ghost danger-text" onclick={removeEvent}><Trash2 size={14} /> Delete</button>{/if}
    <span class="spacer"></span>
    <button class="btn ghost" onclick={() => (editorOpen = false)}>Cancel</button>
    <button class="btn primary" disabled={!editing?.title.trim()} onclick={saveEvent}>Save</button>
  {/snippet}
</Modal>

<Modal bind:open={memOpen} title="What Gilgamesh remembers" width={520}>
  <div class="mem">
    {#each mood.index.memories as m (m.at)}
      <div class="mem-row"><span>{m.text}</span><button class="icon-btn" aria-label="Forget" onclick={() => mood.removeMemory(m.at)}><X size={14} /></button></div>
    {:else}
      <p class="fine">Nothing yet. Tell him about yourself, and he’ll keep what matters.</p>
    {/each}
    <form class="two" onsubmit={(e) => { e.preventDefault(); mood.addMemory(newMemory); newMemory = ''; }}>
      <input class="input" placeholder="Add something for him to remember" bind:value={newMemory} />
      <button class="btn" disabled={!newMemory.trim()}><Plus size={14} /> Add</button>
    </form>
    {#if mood.index.story}<div class="label">The long story</div><p class="story">{mood.index.story}</p>{/if}
    <p class="fine">{mood.index.talks.length} talks remembered{mood.index.story ? ', older ones folded into the story' : ''}.</p>
  </div>
</Modal>

<Modal bind:open={setOpen} title="My Mood settings" width={480}>
  <div class="form">
    <div class="label">Model for these talks</div>
    <ModelPicker value={mood.index.model} onchange={(m) => mood.setModel(m)} filter="chat" compact={false} placeholder="RAD’s chat model" />
    <p class="fine">These talks go to this model. Pick a local one (built-in, Ollama, LM Studio) to keep them on your computer; a cloud model’s provider receives what you write here.</p>
    <div class="label">Change PIN</div>
    <div class="two">
      <input class="input" type="password" inputmode="numeric" placeholder="Current PIN" bind:value={oldPin} />
      <input class="input" type="password" inputmode="numeric" placeholder="New PIN" bind:value={newPin} />
    </div>
    <button class="btn" disabled={!oldPin || !newPin} onclick={changePin}>Change PIN</button>
    <p class="fine">My Mood locks itself when you leave it, after 10 minutes without use, and when the vault locks. The smiley on the PIN screen erases everything.</p>
  </div>
</Modal>

<style>
  .mood {
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
    display: flex;
    align-items: center;
    gap: 7px;
  }
  h1 :global(svg) {
    color: #ec4899;
  }
  .spacer {
    flex: 1;
  }
  .icon-btn.active {
    color: var(--accent);
    background: var(--accent-soft);
  }
  .center {
    flex: 1;
    display: grid;
    place-items: center;
    padding: 24px;
  }

  /* PIN */
  .pinbox {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    width: 280px;
    text-align: center;
  }
  .avatar {
    width: 72px;
    height: 72px;
    border-radius: 50%;
    object-fit: cover;
    box-shadow: var(--shadow);
  }
  .glyph {
    display: grid;
    place-items: center;
    background: color-mix(in oklab, #ec4899 14%, var(--surface));
    color: #ec4899;
  }
  h2 {
    margin: 4px 0 0;
    font-size: 17px;
    font-weight: 650;
  }
  .fine {
    margin: 0;
    font-size: 12px;
    color: var(--text-3);
    line-height: 1.45;
  }
  .dots {
    display: flex;
    gap: 12px;
    margin-top: 8px;
  }
  .dots i {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    border: 1.5px solid var(--border-strong);
    transition: background var(--t-fast);
  }
  .dots i.on {
    background: var(--accent);
    border-color: var(--accent);
  }
  .shake {
    animation: shake 300ms;
  }
  @keyframes shake {
    25% { transform: translateX(-6px); }
    75% { transform: translateX(6px); }
  }
  .msg {
    min-height: 18px;
    font-size: 12.5px;
    color: var(--danger);
  }
  .pad {
    display: grid;
    grid-template-columns: repeat(3, 64px);
    gap: 12px;
  }
  .key {
    width: 64px;
    height: 64px;
    border-radius: 50%;
    border: 1px solid var(--border);
    background: var(--surface-2);
    font-size: 22px;
    font-weight: 500;
    color: var(--text);
    display: grid;
    place-items: center;
    cursor: pointer;
    transition: background var(--t-fast), transform var(--t-fast);
  }
  .key:hover:not(:disabled) {
    background: var(--surface-3);
  }
  .key:active:not(:disabled) {
    transform: scale(0.94);
  }
  .key:disabled {
    opacity: 0.4;
  }
  .key.aux,
  .key.smile {
    background: transparent;
    border-color: transparent;
    color: var(--text-3);
  }
  .key.smile:hover {
    color: #f5a524;
    background: transparent;
  }
  .ok {
    margin-top: 6px;
    width: 100%;
  }

  /* Open */
  .main {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .cal {
    width: 300px;
    flex: none;
    border-right: 1px solid var(--border);
    overflow-y: auto;
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .month {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-weight: 600;
    font-size: 13.5px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: 2px;
  }
  .wd {
    text-align: center;
    font-size: 11px;
    color: var(--text-3);
    padding: 2px 0 4px;
  }
  .cell {
    position: relative;
    aspect-ratio: 1;
    border: none;
    border-radius: var(--r-sm);
    background: transparent;
    font-size: 12.5px;
    color: var(--text);
    cursor: pointer;
    padding: 0;
  }
  .cell:hover {
    background: var(--surface-hover);
  }
  .cell.out {
    color: var(--text-4);
  }
  .cell.today {
    font-weight: 700;
    color: var(--accent);
  }
  .cell.sel {
    background: var(--accent-soft);
  }
  .marks {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 3px;
    display: flex;
    justify-content: center;
    gap: 3px;
  }
  .marks i {
    width: 5px;
    height: 5px;
    border-radius: 50%;
  }
  .ev,
  .worry {
    --c: color-mix(in oklab, var(--danger) calc(var(--w) * 10%), var(--success));
    background: var(--c);
  }
  .nt {
    background: var(--text-3);
  }
  .ask {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .ask-item {
    padding: 10px;
    border-radius: var(--r);
    background: var(--accent-softer);
    border: 1px solid var(--accent-soft);
  }
  .q {
    font-size: 13px;
    margin-bottom: 6px;
  }
  .outs {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    border: 1px solid var(--border-strong);
    background: var(--surface);
    border-radius: 99px;
    padding: 3px 10px;
    font-size: 12px;
    color: var(--text-2);
    cursor: pointer;
  }
  .chip:hover,
  .chip.on {
    border-color: var(--accent);
    color: var(--accent);
  }
  .day {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .day-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    margin-bottom: 2px;
  }
  .d {
    font-weight: 600;
    font-size: 13.5px;
  }
  .when {
    font-size: 11.5px;
    color: var(--text-3);
  }
  .event {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 7px 8px;
    border-radius: var(--r-sm);
    border: 1px solid var(--border);
    background: var(--surface-2);
    text-align: left;
    color: var(--text);
    font-size: 13px;
    cursor: pointer;
  }
  .event:hover {
    border-color: var(--border-strong);
  }
  .worry {
    flex: none;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    font-size: 10.5px;
    font-weight: 700;
    color: #fff;
  }
  .et {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .out {
    font-size: 10.5px;
    padding: 1px 6px;
    border-radius: 99px;
  }
  .o-better,
  .o-as_expected {
    background: var(--success-soft);
    color: var(--success);
  }
  .o-worse {
    background: var(--warning-soft);
    color: var(--warning);
  }
  .empty {
    font-size: 12.5px;
    color: var(--text-3);
  }
  .label {
    display: block;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-2);
    margin-top: 6px;
  }
  .moods {
    display: flex;
    gap: 4px;
  }
  .mface {
    flex: 1;
    font-size: 20px;
    border: 1px solid transparent;
    background: transparent;
    border-radius: var(--r-sm);
    cursor: pointer;
    padding: 4px 0;
    filter: grayscale(0.8);
    opacity: 0.6;
  }
  .mface:hover,
  .mface.on {
    filter: none;
    opacity: 1;
  }
  .mface.on {
    border-color: var(--accent);
    background: var(--accent-softer);
  }
  textarea {
    resize: vertical;
    font: inherit;
    font-size: 13px;
    color: var(--text);
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    padding: 8px 10px;
  }

  .talk {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .msgs {
    flex: 1;
    overflow-y: auto;
    padding: 20px max(20px, calc((100% - 720px) / 2));
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .row {
    display: flex;
    gap: 9px;
    align-items: flex-start;
  }
  .mini {
    flex: none;
    width: 30px;
    height: 30px;
    border-radius: 50%;
    object-fit: cover;
  }
  .bubble {
    max-width: 80%;
    padding: 9px 13px;
    border-radius: 16px;
    font-size: 14px;
    line-height: 1.55;
    white-space: pre-wrap;
  }
  .bubble.you {
    align-self: flex-end;
    background: var(--user-bubble);
    border-bottom-right-radius: 5px;
  }
  .bubble.him {
    white-space: normal;
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-top-left-radius: 5px;
  }
  .bubble.him :global(p) {
    margin: 0 0 0.6em;
  }
  .bubble.him :global(p:last-child) {
    margin: 0;
  }
  .typing {
    display: flex;
    gap: 4px;
    padding: 13px;
  }
  .typing i {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--text-3);
    animation: blink 1.2s infinite;
  }
  .typing i:nth-child(2) {
    animation-delay: 0.2s;
  }
  .typing i:nth-child(3) {
    animation-delay: 0.4s;
  }
  @keyframes blink {
    50% { opacity: 0.25; }
  }
  .err {
    font-size: 12.5px;
    color: var(--danger);
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .composer {
    flex: none;
    display: flex;
    gap: 8px;
    align-items: flex-end;
    padding: 10px max(20px, calc((100% - 720px) / 2)) 16px;
    border-top: 1px solid var(--border);
  }
  .composer textarea {
    flex: 1;
    resize: none;
    font-size: 14px;
  }
  .mic {
    align-self: center;
  }
  .mic.live {
    color: var(--danger);
    background: var(--danger-soft);
  }
  .hearing {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px max(20px, calc((100% - 720px) / 2)) 0;
    font-size: 12.5px;
    color: var(--text-2);
    font-style: italic;
  }
  .hearing i {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--danger);
    transform: scale(calc(0.8 + var(--lvl) * 0.9));
    transition: transform 80ms;
  }

  .form,
  .mem {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .two {
    display: flex;
    gap: 8px;
  }
  .two > :global(*) {
    flex: 1;
  }
  .two > .btn {
    flex: none;
  }
  .mem-row {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    font-size: 13px;
    padding: 6px 0;
    border-bottom: 1px solid var(--border);
  }
  .mem-row span {
    flex: 1;
  }
  .story {
    margin: 0;
    font-size: 13px;
    color: var(--text-2);
    line-height: 1.5;
  }
  .modal-p {
    margin: 0;
    font-size: 13.5px;
    line-height: 1.5;
    color: var(--text-2);
  }
  .danger-text {
    color: var(--danger);
  }
</style>
