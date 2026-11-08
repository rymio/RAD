<script lang="ts">
  // Coach (Media → Coach): Rob's Aussie Accent Coach inside RAD. Read a sentence (or a whole page)
  // aloud; it's compared with an Australian reference reading: melody, pace, pauses, ending, word
  // sounds, the Australian vowels and the R. The app itself is components/coach/CoachApp.svelte,
  // in its own look; Gilgamesh is beside it, sees the feedback and can work it.
  import '@fontsource-variable/literata/opsz.css';
  import '@fontsource-variable/literata/opsz-italic.css';
  import '@fontsource-variable/hanken-grotesk';
  import '../lib/coach/coach.css';
  import CoachApp from './coach/CoachApp.svelte';
  import CoachChat from './CoachChat.svelte';
  import { MicVocal, PanelRight, PanelRightClose } from '@lucide/svelte';

  const KEY = 'rad.coach.chat';
  let showChat = $state(true);
  try {
    showChat = localStorage.getItem(KEY) !== '0';
  } catch {}
  function toggleChat() {
    showChat = !showChat;
    try {
      localStorage.setItem(KEY, showChat ? '1' : '0');
    } catch {}
  }
</script>

<div class="coach-view">
  <header class="drag">
    <span class="logo"><MicVocal size={17} /></span>
    <h1>Coach</h1>
    <span class="sub">Practise a General Australian accent</span>
    <span class="spacer"></span>
    <button class="icon-btn no-drag" title={showChat ? 'Hide Gilgamesh' : 'Show Gilgamesh'} onclick={toggleChat}>{#if showChat}<PanelRightClose size={16} />{:else}<PanelRight size={16} />{/if}</button>
  </header>
  <div class="main">
    <section class="work coach-app">
      <CoachApp />
    </section>
    {#if showChat}
      <aside class="side"><CoachChat /></aside>
    {/if}
  </div>
</div>

<style>
  .coach-view {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--bg);
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
  .logo {
    color: var(--accent);
    display: grid;
    place-items: center;
  }
  h1 {
    font-size: 15px;
    font-weight: 600;
    margin: 0;
  }
  .sub {
    font-size: 12px;
    color: var(--text-3);
  }
  .spacer {
    flex: 1;
  }
  .main {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .work {
    flex: 1;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }
  .side {
    width: 340px;
    flex: none;
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-left: 1px solid var(--border);
  }
</style>
