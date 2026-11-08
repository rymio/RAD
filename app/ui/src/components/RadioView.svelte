<script lang="ts">
  // Radio (Media → Radio): the owner's RTL-SDR dongle. The dongle does one thing at a time, so
  // each tab puts it to its own use: the waterfall (look and listen), the scanner (sweep a band),
  // the aircraft radar and the packets (both on 1090 MHz). Gilgamesh is beside it.
  import { onMount } from 'svelte';
  import { radio, mhz, bandOf, type Tab } from '../lib/radio.svelte';
  import RadioWaterfall from './RadioWaterfall.svelte';
  import RadioScanner from './RadioScanner.svelte';
  import RadioSubGhz from './RadioSubGhz.svelte';
  import RadioRadar from './RadioRadar.svelte';
  import RadioPackets from './RadioPackets.svelte';
  import RadioChat from './RadioChat.svelte';
  import { RadioTower, Power, Loader, PanelRight, PanelRightClose, AudioWaveform, ScanSearch, Plane, Binary, TriangleAlert, Terminal, Antenna } from '@lucide/svelte';

  let showChat = $state(true);
  onMount(() => void radio.load());

  const TABS: { id: Tab; label: string; icon: typeof Plane; title: string }[] = [
    { id: 'waterfall', label: 'Waterfall', icon: AudioWaveform, title: 'Look at ~2 MHz of the spectrum and listen (FM, airband, two-way radio)' },
    { id: 'scanner', label: 'Scanner', icon: ScanSearch, title: 'Sweep a band and list what’s transmitting' },
    { id: 'subghz', label: 'Sub-GHz', icon: Antenna, title: 'Watch a frequency for short transmissions and identify them: remotes, sensors, LoRa / Meshtastic, drone links' },
    { id: 'aircraft', label: 'Aircraft', icon: Plane, title: 'A local radar from ADS-B (1090 MHz)' },
    { id: 'packets', label: 'Packets', icon: Binary, title: 'Every Mode S frame, decoded field by field' },
  ];

  /** Each tab uses the dongle its own way. */
  function pick(t: Tab) {
    radio.tab = t;
    if (!radio.on) return;
    if (t === 'aircraft' || t === 'packets') {
      if (radio.mode !== 'adsb') void radio.radar().then(() => (radio.tab = t));
    } else if (t === 'subghz') {
      if (radio.mode !== 'bursts') void radio.watch();
    } else if (t === 'scanner') {
      if (radio.mode !== 'scan') void radio.scan();
    } else if (radio.mode !== 'spectrum') void radio.tune(radio.settings.freq, radio.settings.demod);
  }

  function power() {
    if (radio.on) return radio.stop();
    const t = radio.tab;
    return radio.start(t === 'aircraft' || t === 'packets' ? 'adsb' : t === 'scanner' ? 'scan' : t === 'subghz' ? 'bursts' : 'spectrum');
  }

  const now = $derived(
    !radio.on
      ? ''
      : radio.mode === 'bursts'
        ? `Watching ${mhz(radio.settings.watch)} · ${radio.bursts.length} transmissions`
      : radio.mode === 'adsb'
        ? `1090 MHz · ${radio.aircraft.length} aircraft · ${radio.frameRate} frames/s`
        : radio.mode === 'scan'
          ? `Scanning ${mhz(radio.settings.scanFrom)}–${mhz(radio.settings.scanTo)}${radio.scanStep ? ` · ${Math.round(((radio.scanStep.step + 1) / radio.scanStep.steps) * 100)}%` : ''}`
          : `${radio.listening ? 'Listening' : 'Showing'} ${mhz(radio.settings.freq)}${bandOf(radio.settings.freq) ? ` · ${bandOf(radio.settings.freq)!.name}` : ''}`,
  );
  const slow = $derived(radio.on && radio.msps > 0 && radio.msps < (radio.rate / 1e6) * 0.93);
</script>

<div class="radio">
  <header class="drag">
    <span class="logo"><RadioTower size={17} /></span>
    <h1>Radio</h1>
    {#if radio.on}<span class="now no-drag truncate" title={radio.tuner ? `RTL-SDR, ${radio.tuner} tuner` : ''}><span class="dot"></span>{now}</span>{/if}
    <span class="spacer"></span>
    <nav class="tabs no-drag">
      {#each TABS as t (t.id)}
        {@const Icon = t.icon}
        <button class:on={radio.tab === t.id} title={t.title} onclick={() => pick(t.id)}><Icon size={13} /> {t.label}</button>
      {/each}
    </nav>
    <span class="spacer"></span>
    {#if radio.on}
      <label class="no-drag ctl" title="Tuner gain. Automatic is fine for broadcast; turn it up for weak signals and aircraft, down if strong stations overload.">
        Gain
        <select class="input sm" value={radio.settings.gain == null ? 'auto' : String(radio.settings.gain)} onchange={(e) => radio.setGain(e.currentTarget.value === 'auto' ? null : Number(e.currentTarget.value))}>
          <option value="auto">Auto</option>
          {#each radio.gains as g (g)}<option value={String(g)}>{g} dB</option>{/each}
        </select>
      </label>
      <label class="no-drag ctl" title="Frequency correction for the dongle's crystal, in parts per million (stations sitting off their marks? adjust this)">
        PPM
        <input class="input sm ppm" type="number" step="1" value={radio.settings.ppm} onchange={(e) => radio.setPpm(Math.round(Number(e.currentTarget.value) || 0))} />
      </label>
      {#if slow}<span class="warn no-drag" title="The computer isn't keeping up with the dongle; samples are being dropped"><TriangleAlert size={14} /> {radio.msps.toFixed(2)} MS/s</span>{/if}
    {/if}
    <button class="btn sm no-drag" class:primary={!radio.on} disabled={radio.starting || radio.tool === null} onclick={power} title={radio.on ? 'Turn the dongle off' : 'Turn the dongle on'}>
      {#if radio.starting}<Loader size={13} class="spin" />{:else}<Power size={13} />{/if}
      {radio.on ? 'Off' : 'On'}
    </button>
    <button class="icon-btn no-drag" title={showChat ? 'Hide Gilgamesh' : 'Show Gilgamesh'} onclick={() => (showChat = !showChat)}>{#if showChat}<PanelRightClose size={16} />{:else}<PanelRight size={16} />{/if}</button>
  </header>

  <div class="main">
    <section class="work">
      {#if radio.error && !radio.on}
        <div class="banner err"><TriangleAlert size={14} /> {radio.error}</div>
      {/if}
      {#if !radio.on}
        <div class="off">
          <RadioTower size={36} />
          {#if radio.tool === null}
            <h2>rtl-sdr isn’t installed</h2>
            <p>RAD drives the dongle through <code>rtl_tcp</code>, part of the rtl-sdr tools. Install them, then come back:</p>
            <pre><Terminal size={13} /> brew install librtlsdr</pre>
            <p class="muted small">Or radioconda / any rtl-sdr install: RAD looks on the PATH, in ~/radioconda/bin, /opt/homebrew/bin and /usr/local/bin.</p>
            <button class="btn" onclick={() => radio.check()}>Look again</button>
          {:else}
            <h2>Your RTL-SDR</h2>
            <p>Plug the dongle in and turn it on. Then watch the waterfall and listen to FM or the airband, sweep a band for transmitters, or see the aircraft around you on 1090 MHz.</p>
            <button class="btn primary lg" disabled={radio.starting} onclick={power}>{#if radio.starting}<Loader size={15} class="spin" />{:else}<Power size={15} />{/if} Turn on</button>
            {#if radio.tool}<p class="muted small">Using {radio.tool}</p>{/if}
          {/if}
        </div>
      {:else if radio.tab === 'waterfall'}
        <RadioWaterfall />
      {:else if radio.tab === 'scanner'}
        <RadioScanner />
      {:else if radio.tab === 'subghz'}
        <RadioSubGhz />
      {:else if radio.tab === 'aircraft'}
        <RadioRadar />
      {:else}
        <RadioPackets />
      {/if}
    </section>
    {#if showChat}
      <aside class="side"><RadioChat /></aside>
    {/if}
  </div>
</div>

<style>
  .radio {
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
  .now {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-2);
    max-width: 320px;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--success);
    flex: none;
  }
  .spacer {
    flex: 1;
  }
  .tabs {
    display: flex;
    gap: 2px;
    padding: 2px;
    border-radius: var(--r-sm);
    background: var(--surface-2);
    border: 1px solid var(--border);
  }
  .tabs button {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font: 12.5px var(--font);
    padding: 4px 10px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--text-2);
    cursor: pointer;
  }
  .tabs button.on {
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow-sm);
  }
  .ctl {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 12px;
    color: var(--text-3);
  }
  .ppm {
    width: 56px;
  }
  .warn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    color: var(--warning);
  }
  .main {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .work {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .side {
    width: 340px;
    flex: none;
    display: flex;
    border-left: 1px solid var(--border);
    min-height: 0;
  }
  .banner {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 14px;
    font-size: 13px;
  }
  .banner.err {
    background: var(--danger-soft);
    color: var(--danger);
  }
  .off {
    margin: auto;
    max-width: 460px;
    text-align: center;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    padding: 24px;
    color: var(--text-2);
  }
  .off :global(svg:first-child) {
    color: var(--accent);
  }
  .off h2 {
    margin: 4px 0 0;
    font-size: 18px;
    color: var(--text);
  }
  .off p {
    margin: 0;
    line-height: 1.5;
    font-size: 13.5px;
  }
  .off pre {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    padding: 8px 14px;
    border-radius: var(--r-sm);
    background: var(--code-bg);
    font: 13px var(--mono);
    user-select: text;
  }
</style>
