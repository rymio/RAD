<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { listen } from '@tauri-apps/api/event';
  import { store } from '../lib/state.svelte';
  import { rad } from '../lib/api';
  import Sidebar from './Sidebar.svelte';
  import { gilgamesh } from '../lib/gilgamesh/gilgamesh.svelte';
  import { activity } from '../lib/gilgamesh/observe.svelte';
  import { transcripts } from '../lib/gilgamesh/transcripts.svelte';
  import { monitor } from '../lib/monitor.svelte';
  import MonitorView from './MonitorView.svelte';
  import { bots } from '../lib/bots.svelte';
  import { sight } from '../lib/gilgamesh/sight.svelte';
  import { stage } from '../lib/gilgamesh/stage.svelte';
  import BotsView from './BotsView.svelte';
  import PhotoCraftView from './PhotoCraftView.svelte';
  import { photocraft } from '../lib/photocraft.svelte';
  import { MicOff } from '@lucide/svelte';
  import ChatView from './ChatView.svelte';
  import ProjectView from './ProjectView.svelte';
  import Settings from './Settings.svelte';
  import BrowserView from './BrowserView.svelte';
  import TerminalView from './TerminalView.svelte';
  import FilesView from './FilesView.svelte';
  import CalendarView from './CalendarView.svelte';
  import EmailView from './EmailView.svelte';
  import MusicView from './MusicView.svelte';
  import VideoView from './VideoView.svelte';
  import RymnetView from './RymnetView.svelte';
  import RymwinView from './RymwinView.svelte';
  import ContainersView from './ContainersView.svelte';
  import SidexView from './SidexView.svelte';
  import DatabaseView from './DatabaseView.svelte';
  import GamesView from './GamesView.svelte';
  import ZxView from './ZxView.svelte';
  import LabView from './LabView.svelte';
  import C64View from './C64View.svelte';
  import C64LabView from './C64LabView.svelte';
  import DosView from './DosView.svelte';
  import GilgameshView from './GilgameshView.svelte';
  import MoodView from './MoodView.svelte';
  import { mood } from '../lib/mood.svelte';
  import CommandPalette from './CommandPalette.svelte';
  import VpnToggle from './VpnToggle.svelte';
  import { UploadCloud, PanelLeftOpen } from '@lucide/svelte';

  let dragging = $state(false);

  const dropTarget = $derived(store.route.kind === 'project' ? store.projectMap.get(store.route.id)?.name : null);

  async function uploadTo(paths: string[]) {
    if (!paths.length) return;
    try {
      if (store.route.kind === 'project') {
        const res = await rad.uploadPaths(store.route.id, paths);
        report(res);
      } else {
        // In a chat: add to the inbox and attach to the composer.
        const inbox = await store.inboxProject();
        const res = await rad.uploadPaths(inbox.id, paths, 'chat');
        report(res);
        const docs = res.filter((r): r is any => 'document' in r).map((r) => r.document);
        window.dispatchEvent(new CustomEvent('rad-attach', { detail: docs }));
      }
    } catch (e) {
      store.error(e, 'Upload failed');
    }
  }

  function report(res: any[]) {
    const ok = res.filter((r) => 'document' in r);
    const dup = ok.filter((r) => r.duplicate).length;
    const errs = res.filter((r) => 'error' in r);
    if (ok.length) store.toast('success', `${ok.length} file${ok.length > 1 ? 's' : ''} added`, dup ? `${dup} already in the vault — linked, not re-uploaded.` : 'Indexing in the background.');
    for (const e of errs) store.toast('error', 'Upload failed', e.error);
  }

  // Gilgamesh is with you in every view while the vault is open: he notices where you go (a small
  // nod if he's listening), and is put away when the vault locks.
  $effect(() => {
    const r = store.route;
    untrack(() => {
      activity.visit(r);
      if (gilgamesh.phase === 'listening') gilgamesh.head.acknowledge();
    });
  });

  onMount(() => {
    gilgamesh.load().catch((e) => store.error(e, 'Couldn’t load Gilgamesh'));
    // The monitored websites are checked now (RAD has just been opened) and every three hours.
    monitor.start().catch((e) => store.error(e, 'Couldn’t load Web Monitoring'));
    // Bots' routines run on their schedule while RAD is open.
    bots.start().catch((e) => store.error(e, 'Couldn’t load the bots'));
    // His camera, when the owner has turned his sight on (off when the vault locks).
    sight.start().catch((e) => store.error(e, 'Couldn’t load Gilgamesh’s sight'));
    // Whether PhotoCraft is there (for Gilgamesh's answers).
    void photocraft.refresh();
    // His own window (if it's open from before) follows him again; it closes when the vault locks.
    stage.start().catch((e) => console.warn('Gilgamesh window', e));
    const un: Array<() => void> = [() => gilgamesh.unload(), () => monitor.unload(), () => bots.unload(), () => sight.unload(), () => stage.unload(), () => mood.unload()];
    getCurrentWebview()
      .onDragDropEvent((ev) => {
        const p = ev.payload;
        if (p.type === 'enter' || p.type === 'over') dragging = !['browser', 'chromium', 'tor', 'terminal', 'files', 'calendar', 'email', 'music', 'video', 'rymnet', 'rymwin', 'container', 'sidex', 'database', 'games', 'zx', 'zxlab', 'c64', 'c64lab', 'dos', 'gilgamesh', 'monitor', 'bots', 'photocraft', 'mood'].includes(store.route.kind);
        else if (p.type === 'leave') dragging = false;
        else if (p.type === 'drop') {
          dragging = false;
          if (['browser', 'chromium', 'tor', 'terminal', 'files', 'calendar', 'email', 'music', 'video', 'rymnet', 'rymwin', 'container', 'sidex', 'database', 'games', 'zx', 'zxlab', 'c64', 'c64lab', 'dos', 'gilgamesh', 'monitor', 'bots', 'photocraft', 'mood'].includes(store.route.kind)) return;
          uploadTo(p.paths);
        }
      })
      .then((u) => un.push(u));
    listen<string[]>('rad-open-files', (e) => uploadTo(e.payload)).then((u) => un.push(u));
    return () => un.forEach((u) => u());
  });

  function onKey(e: KeyboardEvent) {
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      store.paletteOpen = !store.paletteOpen;
    } else if (mod && e.key.toLowerCase() === 'n' && !e.shiftKey) {
      e.preventDefault();
      store.newChat();
    } else if (mod && e.key === ',') {
      e.preventDefault();
      store.navigate({ kind: 'settings' });
    } else if (mod && e.key === '\\') {
      e.preventDefault();
      store.toggleSidebar();
    } else if (mod && e.shiftKey && e.key.toLowerCase() === 'l') {
      e.preventDefault();
      store.lock();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="shell">
  {#if store.sidebarCollapsed}
    <button class="icon-btn sb-open no-drag" class:mac={document.documentElement.dataset.os === 'macos'} title="Show sidebar (⌘\)" onclick={() => store.toggleSidebar()}><PanelLeftOpen size={17} /></button>
    <div class="vpn-float" class:mac={document.documentElement.dataset.os === 'macos'}>
      <VpnToggle />
      {#if gilgamesh.phase !== 'off'}
        <!-- The sidebar (with his microphone) is hidden: the microphone still shows. -->
        <button class="icon-btn mic-float" title="Gilgamesh is listening — turn the microphone off" onclick={() => gilgamesh.stop()}><MicOff size={15} /></button>
      {:else if transcripts.recording}
        <button class="icon-btn mic-float" title="Gilgamesh is transcribing — stop and save the transcript" onclick={() => transcripts.stop()}><MicOff size={15} /></button>
      {/if}
    </div>
  {:else}
    <Sidebar />
  {/if}
  <main>
    {#if store.route.kind === 'chat'}
      {#key store.route.id}<ChatView chatId={store.route.id} />{/key}
    {:else if store.route.kind === 'project'}
      {#key store.route.id}<ProjectView projectId={store.route.id} />{/key}
    {:else if store.route.kind === 'settings'}
      <Settings />
    {:else if store.route.kind === 'browser'}
      <BrowserView />
    {:else if store.route.kind === 'monitor'}
      <MonitorView />
    {:else if store.route.kind === 'bots'}
      <BotsView />
    {:else if store.route.kind === 'photocraft'}
      <PhotoCraftView />
    {:else if store.route.kind === 'chromium'}
      <BrowserView kind="chromium" />
    {:else if store.route.kind === 'tor'}
      <BrowserView kind="tor" />
    {:else if store.route.kind === 'terminal'}
      <TerminalView />
    {:else if store.route.kind === 'files'}
      <FilesView />
    {:else if store.route.kind === 'calendar'}
      <CalendarView />
    {:else if store.route.kind === 'email'}
      <EmailView />
    {:else if store.route.kind === 'music'}
      <MusicView />
    {:else if store.route.kind === 'video'}
      <VideoView />
    {:else if store.route.kind === 'rymnet'}
      <RymnetView />
    {:else if store.route.kind === 'rymwin'}
      <RymwinView />
    {:else if store.route.kind === 'zxlab'}
      <LabView />
    {:else if store.route.kind === 'zx'}
      <ZxView />
    {:else if store.route.kind === 'c64'}
      <C64View />
    {:else if store.route.kind === 'c64lab'}
      <C64LabView />
    {:else if store.route.kind === 'dos'}
      <DosView />
    {:else if store.route.kind === 'gilgamesh'}
      <GilgameshView />
    {:else if store.route.kind === 'mood'}
      <MoodView />
    {:else if store.route.kind === 'games'}
      <GamesView />
    {:else if store.route.kind === 'database'}
      <DatabaseView />
    {:else if store.route.kind === 'sidex'}
      <SidexView />
    {:else if store.route.kind === 'container'}
      {#key store.route.type}<ContainersView kind={store.route.type} />{/key}
    {/if}
  </main>
  {#if dragging}
    <div class="dropzone">
      <div class="drop-card">
        <UploadCloud size={40} strokeWidth={1.5} />
        <div class="t">Drop to add to {dropTarget ?? 'this chat'}</div>
        <div class="s">{dropTarget ? 'Files are encrypted into your vault and indexed for chat.' : 'Saved to your Inbox and attached to your message.'}</div>
      </div>
    </div>
  {/if}
  <CommandPalette />
</div>

<style>
  .shell {
    display: flex;
    height: 100%;
  }
  .sb-open {
    position: fixed;
    z-index: 60;
    top: 6px;
    left: 8px;
  }
  .sb-open.mac {
    left: 78px;
  }
  .vpn-float {
    position: fixed;
    z-index: 60;
    top: 11px;
    left: 44px;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .mic-float {
    color: var(--danger);
  }
  .vpn-float.mac {
    left: 114px;
  }
  .shell:has(.sb-open) main {
    border-left: none;
  }
  main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    background: var(--surface);
    border-left: 1px solid var(--border);
    position: relative;
  }
  :global(html:not(.translucent)) .shell {
    background: var(--bg);
  }
  .dropzone {
    position: fixed;
    inset: 0;
    z-index: 400;
    display: grid;
    place-items: center;
    background: color-mix(in oklab, var(--accent) 10%, rgba(10, 10, 20, 0.25));
    backdrop-filter: blur(10px) saturate(1.4);
    -webkit-backdrop-filter: blur(10px) saturate(1.4);
    animation: pop 160ms var(--ease) both;
  }
  .drop-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    padding: 46px 70px;
    border-radius: 28px;
    border: 2px dashed var(--accent);
    background: var(--glass);
    color: var(--accent);
    box-shadow: var(--shadow-lg);
  }
  .t {
    font-size: 18px;
    font-weight: 650;
    color: var(--text);
  }
  .s {
    color: var(--text-2);
    font-size: 13px;
  }
</style>
