<script lang="ts">
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { getVersion } from '@tauri-apps/api/app';
  import { store } from '../lib/state.svelte';
  import Modal from './Modal.svelte';
  import Logo from './Logo.svelte';

  let version = $state('');
  let showLicense = $state(false);

  const year = new Date().getFullYear();
  const LICENSE = `MIT License

Copyright (c) ${year} Robert Rymarczyk

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.`;

  onMount(() => {
    getVersion().then((v) => (version = v)).catch(() => {});
    // Help → About RAD in the menu bar.
    const un = listen('show-about', () => (store.aboutOpen = true));
    return () => {
      un.then((u) => u());
    };
  });
</script>

<Modal bind:open={store.aboutOpen} width={420} onclose={() => (showLicense = false)}>
  <div class="about">
    <Logo size={64} />
    <h2>RAD</h2>
    <div class="ver">Version {version || '—'}</div>
    <p class="tag">Your private, encrypted AI assistant — chat with your documents, browse, run terminals and servers, and play your media.</p>
    <dl>
      <dt>Author</dt>
      <dd>Robert Rymarczyk</dd>
      <dt>Licence</dt>
      <dd>MIT <button class="link" onclick={() => (showLicense = !showLicense)}>{showLicense ? 'Hide' : 'Read'}</button></dd>
    </dl>
    {#if showLicense}<pre class="lic selectable">{LICENSE}</pre>{/if}
    <div class="copy">© {year} Robert Rymarczyk</div>
  </div>
</Modal>

<style>
  .about {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    padding: 10px 6px 2px;
  }
  h2 {
    margin: 12px 0 2px;
    font-size: 22px;
    font-weight: 700;
    letter-spacing: 0.02em;
  }
  .ver {
    color: var(--text-3);
    font-size: 12.5px;
    font-variant-numeric: tabular-nums;
  }
  .tag {
    margin: 12px 0 16px;
    color: var(--text-2);
    font-size: 13px;
    line-height: 1.5;
  }
  dl {
    display: grid;
    grid-template-columns: auto auto;
    gap: 6px 14px;
    margin: 0;
    font-size: 13px;
  }
  dt {
    color: var(--text-3);
    text-align: right;
  }
  dd {
    margin: 0;
    text-align: left;
    font-weight: 550;
  }
  .link {
    margin-left: 6px;
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font-size: 12.5px;
    font-weight: 500;
  }
  .lic {
    width: 100%;
    max-height: 220px;
    overflow: auto;
    margin: 14px 0 0;
    padding: 10px 12px;
    border-radius: var(--r-sm);
    background: var(--surface-2);
    border: 1px solid var(--border);
    font-family: var(--mono);
    font-size: 11px;
    line-height: 1.5;
    text-align: left;
    white-space: pre-wrap;
  }
  .copy {
    margin-top: 16px;
    color: var(--text-4);
    font-size: 11.5px;
  }
</style>
