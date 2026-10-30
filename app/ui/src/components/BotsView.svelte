<script lang="ts">
  // Bots: the owner's assistants that work on computers of their own (lib/bots.svelte.ts). On
  // the left the bots; on the right the chosen bot's tasks, what it's waiting for, and its
  // screen, which the owner can take over (and record as a skill). Skills, routines and logins
  // have tabs of their own.
  import { onMount } from 'svelte';
  import { ask } from '@tauri-apps/plugin-dialog';
  import { store } from '../lib/state.svelte';
  import { activity } from '../lib/gilgamesh/observe.svelte';
  import { bots, nextRun, scheduleLabel, type Bot as BotT, type Skill, type Routine, type Secret, type Task, type Schedule } from '../lib/bots.svelte';
  import { timeAgo } from '../lib/util';
  import Modal from './Modal.svelte';
  import { Bot, Plus, Loader, Play, CircleStop, Pencil, Trash2, KeyRound, Sparkles, Repeat, Hand, Disc, Send, Check, X, ArrowLeft, ChevronDown, ChevronUp, Eye, EyeOff, Volume2, VolumeX, Power, MonitorPlay, CornerDownLeft } from '@lucide/svelte';

  type Tab = 'bots' | 'skills' | 'routines' | 'logins';
  let tab = $state<Tab>('bots');
  let selected = $state<string | null>(store.route.kind === 'bots' ? (store.route.bot ?? null) : null);
  let shownTask = $state<string | null>(null);
  let prompt = $state('');
  let skillPick = $state('');
  let takeover = $state(false);
  let typed = $state('');
  let address = $state('');
  let answers = $state<Record<string, string>>({});
  let drafting = $state(false);

  const b = bots;
  const bot = $derived(b.bots.find((x) => x.id === selected) ?? b.bots[0] ?? null);
  const current = $derived(bot ? b.current(bot.id) : null);
  const history = $derived(bot ? b.tasks.filter((t) => t.bot === bot.id) : []);
  const task = $derived(history.find((t) => t.id === shownTask) ?? current ?? history[0] ?? null);
  const waiting = $derived(bot ? b.pending.filter((p) => p.bot === bot.id) : []);
  const recording = $derived(bot ? b.recording[bot.id] : undefined);

  // Gilgamesh sees what's on screen here.
  $effect(() => {
    const working = b.bots.filter((x) => b.current(x.id)).map((x) => `${x.name}: ${b.current(x.id)!.title}`);
    activity.seen('bots', b.bots.length ? `${b.bots.length} bots${working.length ? `; working: ${working.join('; ')}` : ', all free'}${bot ? `; looking at ${bot.name}` : ''}` : 'no bots yet');
    return () => activity.seen('bots', null);
  });

  onMount(() => {
    b.load().catch((e) => store.error(e, 'Couldn’t load the bots'));
  });

  // The screen is refreshed while it's shown (quietly does nothing while the computer is off).
  $effect(() => {
    const id = bot?.id;
    if (!id || tab !== 'bots' || !bot?.container) return;
    let alive = true;
    const loop = async () => {
      while (alive) {
        await b.refreshScreen(id);
        await new Promise((r) => setTimeout(r, current || takeover ? 1500 : 4000));
      }
    };
    void loop();
    return () => (alive = false);
  });

  // ---------------------------------------------------------------- bots

  let botOpen = $state(false);
  let botEdit = $state<{ id: string | null; name: string; role: string; approvals: boolean; vision: boolean; memory: string[] }>({ id: null, name: '', role: '', approvals: true, vision: false, memory: [] });

  function newBot() {
    botEdit = { id: null, name: '', role: '', approvals: true, vision: false, memory: [] };
    botOpen = true;
  }

  function editBot(x: BotT) {
    botEdit = { id: x.id, name: x.name, role: x.role, approvals: x.approvals, vision: x.vision, memory: [...x.memory] };
    botOpen = true;
  }

  async function saveBot(e: Event) {
    e.preventDefault();
    try {
      if (botEdit.id) await b.update(botEdit.id, { name: botEdit.name.trim() || 'Bot', role: botEdit.role.trim(), approvals: botEdit.approvals, vision: botEdit.vision, memory: botEdit.memory });
      else {
        const made = await b.create(botEdit.name, botEdit.role);
        await b.update(made.id, { approvals: botEdit.approvals, vision: botEdit.vision });
        selected = made.id;
      }
      botOpen = false;
    } catch (err) {
      store.error(err, 'Couldn’t save the bot');
    }
  }

  async function removeBot(x: BotT) {
    if (!(await ask(`Remove ${x.name}? Its computer (with its logins in the browser and its files) is deleted too. Your saved logins stay.`, { title: 'Remove bot', kind: 'warning' }))) return;
    botOpen = false;
    await b.remove(x.id).catch((e) => store.error(e, 'Couldn’t remove the bot'));
    selected = null;
  }

  async function give(e: Event) {
    e.preventDefault();
    if (!bot) return;
    const skill = b.skills.find((s) => s.id === skillPick);
    if (!prompt.trim() && !skill) return;
    const text = prompt;
    prompt = '';
    skillPick = '';
    shownTask = null;
    b.run(bot.id, text, { skill }).catch((err) => store.error(err, `${bot.name} couldn’t start`));
  }

  function answer(p: (typeof b.pending)[number], v: string | boolean) {
    b.answer(p, v);
    delete answers[p.task];
  }

  // ---------------------------------------------------------------- the screen

  async function screenClick(e: MouseEvent) {
    if (!bot || !takeover) return;
    const img = e.currentTarget as HTMLImageElement;
    const r = img.getBoundingClientRect();
    const x = ((e.clientX - r.left) / r.width) * img.naturalWidth;
    const y = ((e.clientY - r.top) / r.height) * img.naturalHeight;
    await b.ownerClick(bot.id, Math.round(x), Math.round(y)).catch((err) => store.error(err, 'Couldn’t click'));
  }

  async function sendText(e: Event) {
    e.preventDefault();
    if (!bot || !typed) return;
    const t = typed;
    typed = '';
    await b.ownerType(bot.id, t).catch((err) => store.error(err, 'Couldn’t type'));
  }

  async function go(e: Event) {
    e.preventDefault();
    if (!bot || !address.trim()) return;
    const u = address.trim();
    address = '';
    await b.ownerGoto(bot.id, u).catch((err) => store.error(err, 'Couldn’t open the page'));
  }

  async function toggleRecording() {
    if (!bot) return;
    if (!recording) {
      takeover = true;
      b.startRecording(bot.id);
      return;
    }
    const steps = b.stopRecording(bot.id);
    if (!steps.length) return store.toast('info', 'Nothing recorded', 'Use the screen while recording to show a bot how to do something.');
    drafting = true;
    try {
      const d = await b.draftSkill({ demo: steps });
      openSkill({ id: null, name: d.name, description: d.description, steps: d.steps, source: 'demo' });
    } catch (err) {
      store.error(err, 'Couldn’t write the skill');
    } finally {
      drafting = false;
    }
  }

  async function skillFromTask(t: Task) {
    drafting = true;
    try {
      const d = await b.draftSkill({ task: t });
      openSkill({ id: null, name: d.name, description: d.description, steps: d.steps, source: 'task' });
    } catch (err) {
      store.error(err, 'Couldn’t write the skill');
    } finally {
      drafting = false;
    }
  }

  async function startComputer() {
    if (!bot) return;
    try {
      await b.computer(bot);
      await b.refreshScreen(bot.id);
    } catch (err) {
      store.error(err, 'Couldn’t start the computer');
    }
  }

  // ---------------------------------------------------------------- skills

  let skillOpen = $state(false);
  let skillEdit = $state<{ id: string | null; name: string; description: string; steps: string; source: Skill['source'] }>({ id: null, name: '', description: '', steps: '', source: 'owner' });

  function openSkill(s: typeof skillEdit) {
    skillEdit = s;
    skillOpen = true;
  }

  async function saveSkill(e: Event) {
    e.preventDefault();
    await b.saveSkill({ id: skillEdit.id ?? undefined, name: skillEdit.name.trim() || 'Skill', description: skillEdit.description.trim(), steps: skillEdit.steps.trim(), source: skillEdit.source }).catch((err) => store.error(err, 'Couldn’t save the skill'));
    skillOpen = false;
  }

  async function removeSkill(s: Skill) {
    if (await ask(`Delete the skill “${s.name}”? Routines that use it are deleted too.`, { title: 'Delete skill', kind: 'warning' })) await b.removeSkill(s.id);
  }

  function runSkill(s: Skill, botId: string) {
    tab = 'bots';
    selected = botId;
    shownTask = null;
    b.run(botId, '', { skill: s }).catch((err) => store.error(err, 'Couldn’t run the skill'));
  }

  // ---------------------------------------------------------------- routines

  let routineOpen = $state(false);
  let routineEdit = $state<{ id: string | null; skill: string; bot: string; schedule: Schedule; enabled: boolean; announce: boolean }>({ id: null, skill: '', bot: '', schedule: { every: 'day', time: '08:00', day: 1 }, enabled: true, announce: true });

  function openRoutine(r?: Routine, skill?: Skill) {
    routineEdit = r
      ? { id: r.id, skill: r.skill, bot: r.bot, schedule: { ...r.schedule }, enabled: r.enabled, announce: r.announce }
      : { id: null, skill: skill?.id ?? b.skills[0]?.id ?? '', bot: bot?.id ?? b.bots[0]?.id ?? '', schedule: { every: 'day', time: '08:00', day: 1 }, enabled: true, announce: true };
    routineOpen = true;
  }

  async function saveRoutine(e: Event) {
    e.preventDefault();
    if (!routineEdit.skill || !routineEdit.bot) return;
    await b.saveRoutine({ ...routineEdit, id: routineEdit.id ?? undefined }).catch((err) => store.error(err, 'Couldn’t save the routine'));
    routineOpen = false;
  }

  async function toggleRoutine(r: Routine) {
    r.enabled = !r.enabled;
    await b.save();
  }

  // ---------------------------------------------------------------- logins

  let secretOpen = $state(false);
  let showPassword = $state(false);
  let secretEdit = $state<{ id: string | null; name: string; domain: string; username: string; password: string; bots: string[] }>({ id: null, name: '', domain: '', username: '', password: '', bots: [] });

  function openSecret(s?: Secret) {
    showPassword = false;
    secretEdit = s ? { id: s.id, name: s.name, domain: s.domain, username: s.username, password: s.password, bots: [...s.bots] } : { id: null, name: '', domain: '', username: '', password: '', bots: bot ? [bot.id] : [] };
    secretOpen = true;
  }

  async function saveSecret(e: Event) {
    e.preventDefault();
    const name = secretEdit.name.trim() || secretEdit.domain.trim().split('.').slice(-2, -1)[0] || 'login';
    if (!secretEdit.domain.trim()) return;
    await b.saveSecret({ ...secretEdit, name, id: secretEdit.id ?? undefined }).catch((err) => store.error(err, 'Couldn’t save the login'));
    secretOpen = false;
  }

  async function removeSecret(s: Secret) {
    if (await ask(`Delete the “${s.name}” login? Bots can no longer log in to ${s.domain} with it (the browser may stay logged in on their computers).`, { title: 'Delete login', kind: 'warning' })) await b.removeSecret(s.id);
  }

  const statusLabel: Record<Task['status'], string> = { running: 'Working', waiting: 'Waiting for you', done: 'Done', failed: 'Didn’t finish', stopped: 'Stopped' };
  const fromLabel: Record<Task['from'], string> = { owner: '', gilgamesh: 'via Gilgamesh', routine: 'routine', bot: 'from a bot' };
  const time = (ms: number) => new Date(ms).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' });
  const DAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
</script>

<div class="bots">
  <header class="drag">
    <Bot size={17} />
    <h1>Bots</h1>
    <nav class="tabs no-drag">
      <button class:on={tab === 'bots'} onclick={() => (tab = 'bots')}>Bots</button>
      <button class:on={tab === 'skills'} onclick={() => (tab = 'skills')}>Skills{#if b.skills.length}<span>{b.skills.length}</span>{/if}</button>
      <button class:on={tab === 'routines'} onclick={() => (tab = 'routines')}>Routines{#if b.routines.length}<span>{b.routines.length}</span>{/if}</button>
      <button class:on={tab === 'logins'} onclick={() => (tab = 'logins')}>Logins{#if b.secrets.length}<span>{b.secrets.length}</span>{/if}</button>
    </nav>
    <span class="spacer"></span>
    <button class="icon-btn no-drag" class:active={b.voice} title={b.voice ? 'Gilgamesh tells you when a bot he asked (or a routine) finishes or needs you (click to keep quiet)' : 'Gilgamesh keeps quiet about bots (click to have him tell you)'} onclick={() => b.setVoice(!b.voice)}>
      {#if b.voice}<Volume2 size={16} />{:else}<VolumeX size={16} />{/if}
    </button>
    <button class="btn sm primary no-drag" onclick={newBot}><Plus size={13} /> New bot</button>
  </header>

  {#if !b.loaded}
    <div class="center"><Loader size={20} class="spin" /></div>
  {:else if tab === 'bots'}
    {#if !b.bots.length}
      <div class="center">
        <div class="empty">
          <div class="glyph"><Bot size={28} /></div>
          <h2>Bots that do things for you</h2>
          <p>A bot works on a computer of its own, with a browser that keeps its logins: give it a task and it browses, fills in forms, logs in with logins you’ve saved and reports back. It asks before buying, sending or deleting anything. Teach it a job once and run it on a schedule. Gilgamesh can give bots tasks too.</p>
          <button class="btn primary" onclick={newBot}><Plus size={14} /> Make your first bot</button>
        </div>
      </div>
    {:else}
      <div class="split">
        <aside class="list">
          {#each b.bots as x (x.id)}
            {@const t = b.current(x.id)}
            {@const p = b.pending.some((q) => q.bot === x.id)}
            <button class="botrow" class:sel={bot?.id === x.id} onclick={() => ((selected = x.id), (shownTask = null))}>
              <span class="avatar">{x.name.slice(0, 1).toUpperCase()}</span>
              <span class="who">
                <strong class="truncate">{x.name}</strong>
                <small class="truncate">{p ? 'Waiting for you' : t ? (b.status[x.id] ?? 'Working…') : x.role || 'Free'}</small>
              </span>
              <span class="dot" class:ok={!!t && !p} class:warn={p}></span>
            </button>
          {/each}
        </aside>

        {#if bot}
          <section class="main">
            <div class="bothead">
              <div class="title">
                <strong>{bot.name}</strong>
                {#if bot.role}<span class="role truncate" title={bot.role}>{bot.role}</span>{/if}
              </div>
              <span class="spacer"></span>
              {#if current}<button class="btn sm danger" onclick={() => b.stop(bot.id)}><CircleStop size={13} /> Stop</button>{/if}
              <button class="icon-btn sm" title="Turn its computer off (it keeps its files and logins)" disabled={!bot.container} onclick={() => b.stopComputer(bot.id).catch((e) => store.error(e, 'Couldn’t stop the computer'))}><Power size={14} /></button>
              <button class="icon-btn sm" title="Edit: name, role, approvals, memory" onclick={() => editBot(bot)}><Pencil size={14} /></button>
            </div>

            <div class="work">
              <div class="feed">
                {#if b.setup && current}
                  <div class="setup"><Loader size={14} class="spin" /> Setting up the computer: {b.setup}</div>
                {/if}
                {#each waiting as p (p.task + p.text)}
                  <div class="waitcard">
                    {#if p.kind === 'approve'}
                      <div><Hand size={15} /> <strong>{bot.name}</strong> wants to {p.text}.</div>
                      <div class="row" style="gap:8px; justify-content:flex-end">
                        <button class="btn sm" onclick={() => answer(p, false)}><X size={13} /> Don’t</button>
                        <button class="btn sm primary" onclick={() => answer(p, true)}><Check size={13} /> Allow</button>
                      </div>
                    {:else}
                      <div><Hand size={15} /> <strong>{bot.name}</strong> asks: {p.text}</div>
                      <form class="row" style="gap:8px" onsubmit={(e) => (e.preventDefault(), answer(p, answers[p.task] ?? ''))}>
                        <input class="input" style="flex:1" placeholder="Your answer (or take over the screen, then say “done”)" bind:value={answers[p.task]} />
                        <button class="btn sm primary" disabled={!answers[p.task]?.trim()}><Send size={13} /></button>
                      </form>
                    {/if}
                  </div>
                {/each}

                {#if task}
                  <div class="taskhead">
                    {#if shownTask && task.id !== current?.id}<button class="icon-btn sm" title="Back to the latest" onclick={() => (shownTask = null)}><ArrowLeft size={14} /></button>{/if}
                    <strong class="truncate" title={task.prompt}>{task.title}</strong>
                    <span class="pill" class:success={task.status === 'done'} class:danger={task.status === 'failed'} class:warning={task.status === 'waiting'} class:accent={task.status === 'running'}>{statusLabel[task.status]}</span>
                    {#if fromLabel[task.from]}<span class="muted small">{fromLabel[task.from]}</span>{/if}
                  </div>
                  <ol class="steps selectable">
                    {#each task.steps as s, i (i)}
                      <li data-k={s.kind} class:bad={s.ok === false}>
                        <span class="t">{time(s.at)}</span>
                        <span class="x">{s.text}</span>
                      </li>
                    {/each}
                    {#if task.status === 'running'}<li data-k="note"><span class="t"></span><span class="x"><Loader size={12} class="spin" /> {b.status[bot.id] ?? 'Working…'}</span></li>{/if}
                  </ol>
                  {#if task.status === 'done'}
                    <div class="row" style="gap:8px">
                      <button class="btn sm" disabled={drafting} onclick={() => skillFromTask(task)}>{#if drafting}<Loader size={13} class="spin" />{:else}<Sparkles size={13} />{/if} Save as a skill</button>
                    </div>
                  {/if}
                {:else}
                  <p class="muted pad">No tasks yet. Tell {bot.name} what to do below{b.skills.length ? ', or pick a skill' : ''}.</p>
                {/if}

                {#if history.length > 1}
                  <details class="earlier">
                    <summary>Earlier tasks ({history.length})</summary>
                    {#each history as t (t.id)}
                      <button class="hist" class:sel={t.id === task?.id} onclick={() => (shownTask = t.id)}>
                        <span class="dot" class:ok={t.status === 'done'} class:bad={t.status === 'failed'} class:warn={t.status === 'waiting' || t.status === 'running'}></span>
                        <span class="truncate">{t.title}</span>
                        <small>{timeAgo(t.started)}</small>
                      </button>
                    {/each}
                  </details>
                {/if}
              </div>

              <div class="screen">
                <div class="screenbar">
                  <MonitorPlay size={14} />
                  <span>Its screen</span>
                  <span class="spacer"></span>
                  <button class="btn sm" class:primary={takeover} title="Click and type on its screen yourself (a CAPTCHA, a code, showing it how)" onclick={() => (takeover = !takeover)}><Hand size={13} /> {takeover ? 'You’re in control' : 'Take over'}</button>
                  <button class="btn sm" class:danger={!!recording} disabled={drafting} title="Record what you do on its screen and turn it into a skill" onclick={toggleRecording}>
                    {#if drafting}<Loader size={13} class="spin" />{:else}<Disc size={13} />{/if} {recording ? `Stop (${recording.length})` : 'Record a skill'}
                  </button>
                </div>
                {#if b.screens[bot.id]}
                  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
                  <img class="shot" class:live={takeover} src={b.screens[bot.id]} alt="{bot.name}’s screen" onclick={screenClick} />
                {:else}
                  <div class="noscreen">
                    {#if bot.container}<span class="muted">Its computer is off.</span>{:else}<span class="muted">It gets a computer with its first task.</span>{/if}
                    <button class="btn sm" onclick={startComputer}><Power size={13} /> Start it</button>
                  </div>
                {/if}
                {#if takeover}
                  <div class="controls">
                    <form class="row" style="gap:6px" onsubmit={go}>
                      <input class="input" placeholder="Open an address…" bind:value={address} spellcheck="false" autocomplete="off" />
                      <button class="btn sm"><CornerDownLeft size={13} /></button>
                    </form>
                    <form class="row" style="gap:6px" onsubmit={sendText}>
                      <input class="input" placeholder="Type into the field you clicked…" bind:value={typed} autocomplete="off" />
                      <button class="btn sm" disabled={!typed}><Send size={13} /></button>
                    </form>
                    <div class="row keys">
                      <button class="btn sm" onclick={() => b.ownerBack(bot.id)}><ArrowLeft size={13} /></button>
                      <button class="btn sm" onclick={() => b.ownerKey(bot.id, 'Enter')}>Enter</button>
                      <button class="btn sm" onclick={() => b.ownerKey(bot.id, 'Tab')}>Tab</button>
                      <button class="btn sm" onclick={() => b.ownerKey(bot.id, 'Escape')}>Esc</button>
                      <button class="btn sm" onclick={() => b.ownerKey(bot.id, 'Backspace')}>⌫</button>
                      <button class="btn sm" onclick={() => b.ownerScroll(bot.id, 'up')}><ChevronUp size={13} /></button>
                      <button class="btn sm" onclick={() => b.ownerScroll(bot.id, 'down')}><ChevronDown size={13} /></button>
                    </div>
                    {#if recording}<p class="hint">Recording: what you click and type is turned into a skill when you stop. Passwords aren’t recorded.</p>{/if}
                  </div>
                {/if}
              </div>
            </div>

            <form class="composer" onsubmit={give}>
              {#if b.skills.length}
                <select class="select" bind:value={skillPick} title="Use a skill">
                  <option value="">No skill</option>
                  {#each b.skills as s (s.id)}<option value={s.id}>{s.name}</option>{/each}
                </select>
              {/if}
              <input class="input" placeholder={current ? `${bot.name} is busy…` : skillPick ? 'Anything to add (optional)' : `Give ${bot.name} a task, e.g. “check the price of …”`} bind:value={prompt} disabled={!!current} />
              <button class="btn primary" disabled={!!current || (!prompt.trim() && !skillPick)}><Play size={14} /> Go</button>
            </form>
          </section>
        {/if}
      </div>
    {/if}
  {:else if tab === 'skills'}
    <div class="pane">
      <div class="panehead">
        <p class="muted">Skills are instructions a bot follows: save one from a finished task, record one by doing it yourself on a bot’s screen, or write one.</p>
        <button class="btn sm" onclick={() => openSkill({ id: null, name: '', description: '', steps: '1. ', source: 'owner' })}><Plus size={13} /> New skill</button>
      </div>
      {#each b.skills as s (s.id)}
        <article class="item">
          <div class="itemhead">
            <Sparkles size={15} />
            <strong>{s.name}</strong>
            <span class="muted small">{s.source === 'demo' ? 'recorded' : s.source === 'task' ? 'from a task' : 'written'}</span>
            <span class="spacer"></span>
            {#each b.bots as x (x.id)}<button class="btn sm" disabled={b.busy(x.id)} title="Run it on {x.name}" onclick={() => runSkill(s, x.id)}><Play size={12} /> {x.name}</button>{/each}
            <button class="icon-btn sm" title="Run it on a schedule" onclick={() => openRoutine(undefined, s)}><Repeat size={14} /></button>
            <button class="icon-btn sm" title="Edit" onclick={() => openSkill({ id: s.id, name: s.name, description: s.description, steps: s.steps, source: s.source })}><Pencil size={14} /></button>
            <button class="icon-btn sm" title="Delete" onclick={() => removeSkill(s)}><Trash2 size={14} /></button>
          </div>
          {#if s.description}<p class="desc">{s.description}</p>{/if}
          <pre class="stepsText selectable">{s.steps}</pre>
        </article>
      {:else}
        <p class="muted pad">No skills yet.</p>
      {/each}
    </div>
  {:else if tab === 'routines'}
    <div class="pane">
      <div class="panehead">
        <p class="muted">A routine runs a skill on a bot on a schedule, while RAD is open. Gilgamesh can tell you the result.</p>
        <button class="btn sm" disabled={!b.skills.length || !b.bots.length} title={b.skills.length ? '' : 'Make a skill first'} onclick={() => openRoutine()}><Plus size={13} /> New routine</button>
      </div>
      {#each b.routines as r (r.id)}
        {@const s = b.skills.find((x) => x.id === r.skill)}
        {@const x = b.bots.find((y) => y.id === r.bot)}
        <article class="item row" style="gap:10px">
          <button class="switch" class:on={r.enabled} aria-label="On" onclick={() => toggleRoutine(r)}></button>
          <div class="grow">
            <strong>{s?.name ?? '(deleted skill)'}</strong> <span class="muted">on {x?.name ?? '(deleted bot)'}</span>
            <div class="muted small">{scheduleLabel(r.schedule)}{r.enabled ? ` · next ${new Date(nextRun(r.schedule, r.lastRun)).toLocaleString('en-GB', { weekday: 'short', hour: '2-digit', minute: '2-digit' })}` : ''}{r.announce ? ' · Gilgamesh tells you' : ''}</div>
          </div>
          <button class="icon-btn sm" title="Edit" onclick={() => openRoutine(r)}><Pencil size={14} /></button>
          <button class="icon-btn sm" title="Delete" onclick={() => b.removeRoutine(r.id)}><Trash2 size={14} /></button>
        </article>
      {:else}
        <p class="muted pad">No routines yet.</p>
      {/each}
    </div>
  {:else}
    <div class="pane">
      <div class="panehead">
        <p class="muted">Logins are kept in your encrypted vault. A bot can only use the ones you give it, only on that login’s site, and never sees them: RAD types them in for it. It asks you before logging in (unless you’ve turned that off for the bot).</p>
        <button class="btn sm" onclick={() => openSecret()}><Plus size={13} /> New login</button>
      </div>
      {#each b.secrets as s (s.id)}
        <article class="item row" style="gap:10px">
          <KeyRound size={15} />
          <div class="grow">
            <strong>{s.name}</strong> <span class="muted">{s.domain}{s.username ? ` · ${s.username}` : ''}</span>
            <div class="muted small">{s.bots.length ? `For ${s.bots.map((id) => b.bots.find((x) => x.id === id)?.name).filter(Boolean).join(', ')}` : 'Given to no bot'}{s.used ? ` · used ${timeAgo(s.used)}` : ''}</div>
          </div>
          <button class="icon-btn sm" title="Edit" onclick={() => openSecret(s)}><Pencil size={14} /></button>
          <button class="icon-btn sm" title="Delete" onclick={() => removeSecret(s)}><Trash2 size={14} /></button>
        </article>
      {:else}
        <p class="muted pad">No logins yet.</p>
      {/each}
    </div>
  {/if}
</div>

<Modal bind:open={botOpen} title={botEdit.id ? `Edit ${botEdit.name}` : 'New bot'} width={520}>
  <form onsubmit={saveBot}>
    <div class="field">
      <label class="label" for="bn">Name</label>
      <input id="bn" class="input" bind:value={botEdit.name} placeholder="Ada" />
    </div>
    <div class="field">
      <label class="label" for="br">Role <span class="muted">(what it’s for; it reads this before every task)</span></label>
      <textarea id="br" class="input" rows="3" bind:value={botEdit.role} placeholder="Shopping and bookings: compares prices, keeps an eye on orders and deliveries."></textarea>
    </div>
    <label class="check">
      <input type="checkbox" bind:checked={botEdit.approvals} />
      <span>Ask me first <small>Before it buys, pays, sends, posts, deletes or logs in.</small></span>
    </label>
    <label class="check">
      <input type="checkbox" bind:checked={botEdit.vision} />
      <span>Sees the screen <small>Sends a screenshot with every page. Needs a chat model that can read images; slower and costlier.</small></span>
    </label>
    {#if botEdit.id}
      <div class="field">
        <span class="label">Memory <span class="muted">(what it keeps for later tasks)</span></span>
        {#each botEdit.memory as m, i (i)}
          <div class="mem"><span>{m}</span><button type="button" class="icon-btn sm" title="Forget" onclick={() => botEdit.memory.splice(i, 1)}><X size={12} /></button></div>
        {:else}
          <p class="muted small">Nothing yet.</p>
        {/each}
      </div>
    {/if}
    <div class="row" style="gap:8px">
      {#if botEdit.id}<button type="button" class="btn ghost danger" onclick={() => removeBot(b.bots.find((x) => x.id === botEdit.id)!)}><Trash2 size={13} /> Remove</button>{/if}
      <span class="spacer"></span>
      <button type="button" class="btn ghost" onclick={() => (botOpen = false)}>Cancel</button>
      <button class="btn primary">{botEdit.id ? 'Save' : 'Make it'}</button>
    </div>
  </form>
</Modal>

<Modal bind:open={skillOpen} title={skillEdit.id ? 'Edit skill' : 'New skill'} width={600}>
  <form onsubmit={saveSkill}>
    <div class="field">
      <label class="label" for="sn">Name</label>
      <input id="sn" class="input" bind:value={skillEdit.name} placeholder="Download the electricity bill" />
    </div>
    <div class="field">
      <label class="label" for="sd">What it does</label>
      <input id="sd" class="input" bind:value={skillEdit.description} />
    </div>
    <div class="field">
      <label class="label" for="ss">Steps</label>
      <textarea id="ss" class="input mono" rows="12" bind:value={skillEdit.steps}></textarea>
      <div class="hint">Write {'{like this}'} for anything that changes from one run to the next, and “log in with the saved login for SITE” for logging in.</div>
    </div>
    <div class="row" style="gap:8px; justify-content:flex-end">
      <button type="button" class="btn ghost" onclick={() => (skillOpen = false)}>Cancel</button>
      <button class="btn primary" disabled={!skillEdit.steps.trim()}>Save</button>
    </div>
  </form>
</Modal>

<Modal bind:open={routineOpen} title={routineEdit.id ? 'Edit routine' : 'New routine'} width={480}>
  <form onsubmit={saveRoutine}>
    <div class="row" style="gap:10px">
      <div class="field" style="flex:1">
        <label class="label" for="rs">Skill</label>
        <select id="rs" class="select" bind:value={routineEdit.skill}>{#each b.skills as s (s.id)}<option value={s.id}>{s.name}</option>{/each}</select>
      </div>
      <div class="field" style="flex:1">
        <label class="label" for="rb">Bot</label>
        <select id="rb" class="select" bind:value={routineEdit.bot}>{#each b.bots as x (x.id)}<option value={x.id}>{x.name}</option>{/each}</select>
      </div>
    </div>
    <div class="row" style="gap:10px">
      <div class="field" style="flex:1">
        <label class="label" for="re">When</label>
        <select id="re" class="select" bind:value={routineEdit.schedule.every}>
          <option value="hour">Every hour</option>
          <option value="day">Every day</option>
          <option value="weekday">Weekdays</option>
          <option value="week">Once a week</option>
        </select>
      </div>
      {#if routineEdit.schedule.every === 'week'}
        <div class="field" style="flex:1">
          <label class="label" for="rd">Day</label>
          <select id="rd" class="select" bind:value={routineEdit.schedule.day}>{#each DAYS as d, i (d)}<option value={i}>{d}</option>{/each}</select>
        </div>
      {/if}
      <div class="field" style="width:110px">
        <label class="label" for="rt">{routineEdit.schedule.every === 'hour' ? 'Minutes past' : 'At'}</label>
        <input id="rt" class="input" type="time" bind:value={routineEdit.schedule.time} />
      </div>
    </div>
    <label class="check">
      <input type="checkbox" bind:checked={routineEdit.announce} />
      <span>Gilgamesh tells me the result</span>
    </label>
    <p class="hint">Routines run while RAD is open. One that was due while it was closed runs once when it opens.</p>
    <div class="row" style="gap:8px; justify-content:flex-end">
      <button type="button" class="btn ghost" onclick={() => (routineOpen = false)}>Cancel</button>
      <button class="btn primary" disabled={!routineEdit.skill || !routineEdit.bot}>Save</button>
    </div>
  </form>
</Modal>

<Modal bind:open={secretOpen} title={secretEdit.id ? `Edit ${secretEdit.name}` : 'New login'} width={480}>
  <form onsubmit={saveSecret}>
    <div class="row" style="gap:10px">
      <div class="field" style="flex:1">
        <label class="label" for="kd">Site</label>
        <input id="kd" class="input" bind:value={secretEdit.domain} placeholder="github.com" spellcheck="false" autocomplete="off" />
      </div>
      <div class="field" style="width:150px">
        <label class="label" for="kn">Name</label>
        <input id="kn" class="input" bind:value={secretEdit.name} placeholder="github" spellcheck="false" autocomplete="off" />
      </div>
    </div>
    <div class="field">
      <label class="label" for="ku">Username or email</label>
      <input id="ku" class="input" bind:value={secretEdit.username} spellcheck="false" autocomplete="off" />
    </div>
    <div class="field">
      <label class="label" for="kp">Password</label>
      <div class="row" style="gap:6px">
        <input id="kp" class="input" style="flex:1" type={showPassword ? 'text' : 'password'} bind:value={secretEdit.password} autocomplete="new-password" />
        <button type="button" class="icon-btn" title={showPassword ? 'Hide' : 'Show'} onclick={() => (showPassword = !showPassword)}>{#if showPassword}<EyeOff size={15} />{:else}<Eye size={15} />{/if}</button>
      </div>
    </div>
    <div class="field">
      <span class="label">Bots that may use it</span>
      {#each b.bots as x (x.id)}
        <label class="check"><input type="checkbox" checked={secretEdit.bots.includes(x.id)} onchange={(e) => (secretEdit.bots = (e.currentTarget as HTMLInputElement).checked ? [...secretEdit.bots, x.id] : secretEdit.bots.filter((y) => y !== x.id))} /> <span>{x.name}</span></label>
      {:else}
        <p class="muted small">Make a bot first.</p>
      {/each}
      <div class="hint">Typed only into pages on {secretEdit.domain.trim() || 'this site'} (and its subdomains). Two-factor codes: the bot asks you.</div>
    </div>
    <div class="row" style="gap:8px; justify-content:flex-end">
      <button type="button" class="btn ghost" onclick={() => (secretOpen = false)}>Cancel</button>
      <button class="btn primary" disabled={!secretEdit.domain.trim()}>Save</button>
    </div>
  </form>
</Modal>

<style>
  .bots {
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
    margin: 0 8px 0 0;
    font-size: 15px;
    font-weight: 650;
  }
  .tabs {
    display: flex;
    gap: 2px;
  }
  .tabs button {
    padding: 4px 10px;
    border-radius: 7px;
    font-size: 13px;
    color: var(--text-2);
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .tabs button:hover {
    background: var(--surface-2);
  }
  .tabs button.on {
    color: var(--text);
    background: var(--surface-3);
    font-weight: 600;
  }
  .tabs span {
    font-size: 11px;
    color: var(--text-3);
  }
  .center {
    flex: 1;
    display: grid;
    place-items: center;
    padding: 24px;
  }
  .empty {
    max-width: 460px;
    text-align: center;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
  }
  .empty h2 {
    margin: 4px 0 0;
    font-size: 19px;
  }
  .empty p {
    margin: 0 0 6px;
    color: var(--text-2);
    line-height: 1.5;
  }
  .glyph {
    width: 60px;
    height: 60px;
    border-radius: 18px;
    display: grid;
    place-items: center;
    color: var(--accent);
    background: var(--accent-soft);
  }
  .split {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .list {
    flex: none;
    width: 210px;
    overflow: auto;
    padding: 8px;
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .botrow {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 7px 8px;
    border-radius: 9px;
    text-align: left;
    min-width: 0;
  }
  .botrow:hover {
    background: var(--surface-2);
  }
  .botrow.sel {
    background: var(--surface-3);
  }
  .avatar {
    flex: none;
    width: 28px;
    height: 28px;
    border-radius: 9px;
    display: grid;
    place-items: center;
    font-weight: 700;
    font-size: 13px;
    color: var(--accent);
    background: var(--accent-soft);
  }
  .who {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .who small {
    color: var(--text-3);
    font-size: 11.5px;
  }
  .main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .bothead {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 14px;
    border-bottom: 1px solid var(--border);
  }
  .title {
    min-width: 0;
    display: flex;
    align-items: baseline;
    gap: 10px;
  }
  .role {
    color: var(--text-3);
    font-size: 12.5px;
    max-width: 420px;
  }
  .work {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .feed {
    flex: 1;
    min-width: 0;
    overflow: auto;
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .setup {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--text-2);
  }
  .waitcard {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px 12px;
    border-radius: 10px;
    border: 1px solid var(--warning, #d29922);
    background: var(--surface-2);
    font-size: 13.5px;
  }
  .waitcard > div:first-child {
    display: flex;
    align-items: center;
    gap: 7px;
    flex-wrap: wrap;
  }
  .taskhead {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .steps {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 13px;
  }
  .steps li {
    display: flex;
    gap: 10px;
    line-height: 1.45;
  }
  .steps .t {
    flex: none;
    width: 38px;
    color: var(--text-3);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
    padding-top: 1px;
  }
  .steps .x {
    min-width: 0;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }
  .steps li[data-k='act'] .x {
    color: var(--text-2);
    font-family: var(--mono, ui-monospace, monospace);
    font-size: 12px;
  }
  .steps li[data-k='result'] .x {
    color: var(--text-3);
    font-size: 12px;
    padding-left: 12px;
  }
  .steps li[data-k='user'] .x {
    font-weight: 600;
  }
  .steps li[data-k='done'] .x {
    padding: 8px 10px;
    border-radius: 9px;
    background: var(--accent-soft);
  }
  .steps li[data-k='ask'] .x,
  .steps li[data-k='approve'] .x {
    color: var(--warning, #d29922);
  }
  .steps li[data-k='error'] .x,
  .steps li.bad .x {
    color: var(--danger, #e5534b);
  }
  .earlier summary {
    cursor: pointer;
    font-size: 12.5px;
    color: var(--text-3);
    margin-bottom: 4px;
  }
  .hist {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 6px;
    border-radius: 7px;
    font-size: 13px;
    text-align: left;
  }
  .hist:hover,
  .hist.sel {
    background: var(--surface-2);
  }
  .hist small {
    margin-left: auto;
    flex: none;
    color: var(--text-3);
  }
  .screen {
    flex: none;
    width: min(46%, 560px);
    border-left: 1px solid var(--border);
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    overflow: auto;
  }
  .screenbar {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .shot {
    width: 100%;
    border-radius: 8px;
    border: 1px solid var(--border);
    display: block;
  }
  .shot.live {
    cursor: crosshair;
    outline: 2px solid var(--accent);
  }
  .noscreen {
    aspect-ratio: 16 / 10;
    border-radius: 8px;
    border: 1px dashed var(--border);
    display: grid;
    place-content: center;
    justify-items: center;
    gap: 10px;
    font-size: 13px;
  }
  .controls {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .controls .input {
    flex: 1;
  }
  .keys {
    gap: 4px;
    flex-wrap: wrap;
  }
  .composer {
    flex: none;
    display: flex;
    gap: 8px;
    padding: 10px 14px;
    border-top: 1px solid var(--border);
  }
  .composer .input {
    flex: 1;
  }
  .composer .select {
    width: 170px;
  }
  .pane {
    flex: 1;
    overflow: auto;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    max-width: 900px;
  }
  .panehead {
    display: flex;
    align-items: flex-start;
    gap: 14px;
  }
  .panehead p {
    flex: 1;
    margin: 0;
    line-height: 1.5;
    font-size: 13px;
  }
  .item {
    padding: 12px 14px;
    border-radius: 12px;
    border: 1px solid var(--border);
    background: var(--surface-1, transparent);
  }
  .itemhead {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .desc {
    margin: 6px 0 0;
    color: var(--text-2);
    font-size: 13px;
  }
  .stepsText {
    margin: 8px 0 0;
    white-space: pre-wrap;
    font-size: 12.5px;
    color: var(--text-2);
    font-family: inherit;
  }
  .grow {
    flex: 1;
    min-width: 0;
  }
  .small {
    font-size: 12px;
  }
  .pad {
    padding: 4px 0;
  }
  .check {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    margin: 0 0 12px;
    font-size: 13.5px;
  }
  .check small {
    display: block;
    color: var(--text-3);
    font-size: 12px;
  }
  .mem {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    font-size: 13px;
    padding: 3px 0;
  }
  .mem span {
    flex: 1;
  }
  textarea.input {
    resize: vertical;
    height: auto;
    padding-top: 8px;
    padding-bottom: 8px;
  }
</style>
