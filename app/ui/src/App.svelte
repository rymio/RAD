<script lang="ts">
  import { onMount } from 'svelte';
  import { store } from './lib/state.svelte';
  import Onboarding from './components/Onboarding.svelte';
  import Unlock from './components/Unlock.svelte';
  import Shell from './components/Shell.svelte';
  import QuickAsk from './components/QuickAsk.svelte';
  import Toasts from './components/Toasts.svelte';
  import Logo from './components/Logo.svelte';

  let { view }: { view: string } = $props();

  onMount(() => {
    store.boot(view === 'quick');
  });
</script>

{#if view === 'quick'}
  <QuickAsk />
{:else if store.phase === 'boot'}
  <div class="boot drag"><div class="pulse"><Logo size={56} /></div></div>
{:else if store.phase === 'app'}
  <Shell />
{:else if store.phase === 'unlock'}
  <Unlock />
{:else}
  <Onboarding />
{/if}
<Toasts />

<style>
  .boot {
    height: 100%;
    display: grid;
    place-items: center;
    background: var(--bg);
  }
  .pulse {
    animation: breathe 1.8s var(--ease) infinite;
  }
  @keyframes breathe {
    50% {
      transform: scale(1.06);
      opacity: 0.8;
    }
  }
</style>
