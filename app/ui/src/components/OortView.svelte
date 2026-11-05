<script lang="ts">
  // Development → Oort: the space-fleet programming game (oort.rs) in RAD's Chromium engine,
  // with Gilgamesh beside it. See lib/oort.svelte.ts.
  import { onMount } from 'svelte';
  import { store } from '../lib/state.svelte';
  import { oort, OORT_HOME } from '../lib/oort.svelte';
  import BrowserView from './BrowserView.svelte';
  import OortChat from './OortChat.svelte';

  onMount(() => {
    oort.load().catch((e) => store.error(e, 'Couldn’t load Oort'));
    return () => oort.attach(false);
  });

  // Reads the page while it's oort.rs (a wiki link opened in it isn't the game).
  function onpage(t: { url: string } | null, running: boolean) {
    const on = running && !!t?.url.startsWith(OORT_HOME);
    if (on !== oort.open) oort.attach(on);
    if (on) void oort.refresh();
  }
</script>

<BrowserView kind="oort" home={OORT_HOME} {onpage} goto={oort.pendingUrl}>
  {#snippet aside()}<OortChat />{/snippet}
</BrowserView>
