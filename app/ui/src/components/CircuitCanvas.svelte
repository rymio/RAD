<script lang="ts">
  // Z80 Lab's canvas: parts from the palette are dragged onto it, pins are wired by dragging from
  // one pin to another, and while the computer runs every wire shows its level (high, low,
  // floating) with the current flowing away from whatever drives it. See lib/z80lab.svelte.ts.
  import { lab, PART, PARTS, pinPos, bodySize } from '../lib/z80lab.svelte';
  import { CATEGORIES, PITCH, STUB, type PartDef } from '../lib/z80lab/parts';
  import { LO, HI, Z, X, hex, type Placed, type Wire } from '../lib/z80lab/sim';
  import { store } from '../lib/state.svelte';
  import { ZoomIn, ZoomOut, Maximize, Cable, Trash2, RotateCw, X as Close, FileText, ChevronRight, ChevronDown } from '@lucide/svelte';

  const p = $derived(lab.project);
  let svg = $state<SVGSVGElement | null>(null);
  let W = $state(800);
  let H = $state(600);
  let zoom = $state(0.7);
  let panX = $state(-40);
  let panY = $state(-40);
  let hover = $state<{ part: string; pin: number } | null>(null);
  let wiring = $state<{ from: { part: string; pin: number }; x: number; y: number } | null>(null);
  let dragNew = $state<{ type: string; cx: number; cy: number } | null>(null);
  let paletteOpen = $state<Record<string, boolean>>({ Power: true, Processor: true, Memory: true, Peripherals: true, Logic: false, 'Clock & reset': true, Passive: true, Output: true });
  let galDraft = $state<{ id: string; names: string; equations: string } | null>(null);

  const vb = $derived(`${panX} ${panY} ${W / zoom} ${H / zoom}`);
  const sel = $derived(lab.selected.map((id) => p?.parts.find((x) => x.id === id)).filter(Boolean) as Placed[]);
  const one = $derived(sel.length === 1 ? sel[0] : null);

  function world(e: { clientX: number; clientY: number }) {
    const r = svg!.getBoundingClientRect();
    return { x: (e.clientX - r.left) / zoom + panX, y: (e.clientY - r.top) / zoom + panY };
  }

  function end(part: Placed, n: number) {
    const q = pinPos(PART[part.type], n, part.rot ?? 0);
    return { x: part.x + q.x, y: part.y + q.y, dx: q.dx, dy: q.dy };
  }

  const parts = $derived(new Map(p?.parts.map((x) => [x.id, x]) ?? []));

  /** An orthogonal route between two pins, nudged per wire so buses run side by side. */
  function route(w: Wire): string {
    const a = parts.get(w.a.part);
    const b = parts.get(w.b.part);
    if (!a || !b || !PART[a.type] || !PART[b.type]) return '';
    const A = end(a, w.a.pin);
    const B = end(b, w.b.pin);
    let h = 0;
    for (const ch of w.id) h = (h * 31 + ch.charCodeAt(0)) | 0;
    const off = ((Math.abs(h) % 13) - 6) * 3;
    const s = 10;
    const A1 = { x: A.x + A.dx * s, y: A.y + A.dy * s };
    const B1 = { x: B.x + B.dx * s, y: B.y + B.dy * s };
    let mx: number;
    if (A.dx && A.dx === B.dx) mx = A.dx < 0 ? Math.min(A1.x, B1.x) - 12 - Math.abs(off) : Math.max(A1.x, B1.x) + 12 + Math.abs(off);
    else mx = (A1.x + B1.x) / 2 + off;
    return `M${A.x},${A.y} L${A1.x},${A1.y} L${mx},${A1.y} L${mx},${B1.y} L${B1.x},${B1.y} L${B.x},${B.y}`;
  }

  // ------------------------------------------------------------------ what the wires carry

  const sim = $derived((void lab.version, lab.sim));
  const netOf = (e: { part: string; pin: number }) => sim?.netOfPin(e.part, e.pin) ?? -1;
  const gpin = (e: { part: string; pin: number }) => sim?.pinIndex.get(`${e.part}:${e.pin}`) ?? -1;

  /** Each wire's level and which way the current goes (away from the net's driver). */
  const flows = $derived.by(() => {
    const v = lab.view;
    const out = new Map<string, { level: number; dir: 0 | 1 | -1; power: 'vcc' | 'gnd' | null }>();
    if (!p || !sim || !v) return out;
    // The wires of each net, as a graph of pins.
    const byNet = new Map<number, Wire[]>();
    for (const w of p.wires) {
      const n = netOf(w.a);
      if (n < 0) continue;
      (byNet.get(n) ?? byNet.set(n, []).get(n)!).push(w);
    }
    for (const [n, ws] of byNet) {
      const level = v.val[n];
      const d: number = v.drv[n];
      const power = d >= 0 && sim.devs[sim.pinDev[d]]?.part.type === 'psu' ? (level === HI ? 'vcc' : 'gnd') : null;
      const dist = new Map<number, number>();
      if (d >= 0 && v.str[n] > 0) {
        dist.set(d, 0);
        const adj = new Map<number, number[]>();
        for (const w of ws) {
          const ga = gpin(w.a);
          const gb = gpin(w.b);
          (adj.get(ga) ?? adj.set(ga, []).get(ga)!).push(gb);
          (adj.get(gb) ?? adj.set(gb, []).get(gb)!).push(ga);
        }
        const q: number[] = [d];
        while (q.length) {
          const x = q.shift()!;
          for (const y of adj.get(x) ?? []) if (!dist.has(y)) (dist.set(y, dist.get(x)! + 1), q.push(y));
        }
      }
      for (const w of ws) {
        const da = dist.get(gpin(w.a));
        const db = dist.get(gpin(w.b));
        // Ground: the current comes back to the supply, so it flows towards it.
        let dir: 0 | 1 | -1 = da === undefined || db === undefined ? 0 : da <= db ? 1 : -1;
        if (power === 'gnd') dir = (dir * -1) as 0 | 1 | -1;
        out.set(w.id, { level, dir, power });
      }
    }
    return out;
  });

  const busy = $derived(lab.running && !lab.watching);
  const answering = $derived(lab.view?.access?.who ?? null);

  function wireClass(w: Wire) {
    const f = flows.get(w.id);
    if (!lab.power || !f) return 'idle';
    if (f.power) return f.power;
    return f.level === HI ? 'hi' : f.level === LO ? 'lo' : f.level === X ? 'x' : 'z';
  }

  // ------------------------------------------------------------------ pointer

  let drag: { id: string; dx: number; dy: number; moved: boolean; others: { id: string; dx: number; dy: number }[] } | null = null;
  let pan: { x: number; y: number; px: number; py: number } | null = null;

  function downPart(e: PointerEvent, part: Placed) {
    if (e.button !== 0) return;
    e.stopPropagation();
    lab.selectedWire = null;
    if (e.shiftKey || e.metaKey) {
      lab.selected = lab.selected.includes(part.id) ? lab.selected.filter((x) => x !== part.id) : [...lab.selected, part.id];
      return;
    }
    if (!lab.selected.includes(part.id)) lab.selected = [part.id];
    if (lab.power && part.type === 'button') {
      lab.press(part.id, true);
      const up = () => {
        lab.press(part.id, false);
        window.removeEventListener('pointerup', up);
      };
      window.addEventListener('pointerup', up);
      return;
    }
    const w = world(e);
    drag = {
      id: part.id,
      dx: w.x - part.x,
      dy: w.y - part.y,
      moved: false,
      others: lab.selected.filter((id) => id !== part.id).map((id) => {
        const o = parts.get(id)!;
        return { id, dx: o.x - part.x, dy: o.y - part.y };
      }),
    };
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
  }

  function movePart(e: PointerEvent) {
    if (!drag) return;
    const w = world(e);
    const part = parts.get(drag.id);
    if (!part) return;
    const nx = w.x - drag.dx;
    const ny = w.y - drag.dy;
    if (!drag.moved && Math.hypot(nx - part.x, ny - part.y) < 4) return;
    drag.moved = true;
    lab.movePart(drag.id, nx, ny);
    for (const o of drag.others) lab.movePart(o.id, part.x + o.dx, part.y + o.dy);
  }

  function upPart(_e: PointerEvent, part: Placed) {
    if (drag && !drag.moved && lab.power && part.type === 'switch') lab.toggle(part.id);
    drag = null;
  }

  function downPin(e: PointerEvent, part: Placed, n: number) {
    if (e.button !== 0) return;
    e.stopPropagation();
    const w = world(e);
    wiring = { from: { part: part.id, pin: n }, x: w.x, y: w.y };
    window.addEventListener('pointermove', moveWire);
    window.addEventListener('pointerup', upWire, { once: true });
  }

  function moveWire(e: PointerEvent) {
    if (!wiring) return;
    const w = world(e);
    wiring = { ...wiring, x: w.x, y: w.y };
  }

  function upWire() {
    window.removeEventListener('pointermove', moveWire);
    if (wiring && hover && (hover.part !== wiring.from.part || hover.pin !== wiring.from.pin)) lab.addWire(wiring.from, hover);
    wiring = null;
  }

  function downBg(e: PointerEvent) {
    if (e.button !== 0 && e.button !== 1) return;
    lab.selected = [];
    lab.selectedWire = null;
    galDraft = null;
    pan = { x: e.clientX, y: e.clientY, px: panX, py: panY };
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
  }

  function moveBg(e: PointerEvent) {
    if (!pan) return;
    panX = pan.px - (e.clientX - pan.x) / zoom;
    panY = pan.py - (e.clientY - pan.y) / zoom;
  }

  function wheel(e: WheelEvent) {
    e.preventDefault();
    if (e.ctrlKey || e.metaKey || Math.abs(e.deltaY) > Math.abs(e.deltaX) * 2) {
      const before = world(e);
      zoom = Math.min(3, Math.max(0.15, zoom * Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.0015))));
      const r = svg!.getBoundingClientRect();
      panX = before.x - (e.clientX - r.left) / zoom;
      panY = before.y - (e.clientY - r.top) / zoom;
    } else {
      panX += e.deltaX / zoom;
      panY += e.deltaY / zoom;
    }
  }

  export function fit() {
    if (!p?.parts.length) return;
    const xs = p.parts.flatMap((x) => [x.x - 40, x.x + bodySize(PART[x.type]).w + 40]);
    const ys = p.parts.flatMap((x) => [x.y - 40, x.y + bodySize(PART[x.type]).h + 30]);
    const [x0, x1, y0, y1] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
    zoom = Math.min(2, Math.max(0.15, Math.min(W / (x1 - x0), H / (y1 - y0))));
    panX = x0 - (W / zoom - (x1 - x0)) / 2;
    panY = y0 - (H / zoom - (y1 - y0)) / 2;
  }

  let fitted = '';
  $effect(() => {
    if (p && W > 100 && fitted !== p.id) {
      fitted = p.id;
      fit();
    }
  });

  function key(e: KeyboardEvent) {
    if ((e.target as HTMLElement)?.closest('input, textarea, select, [contenteditable]')) return;
    if (e.key === 'Delete' || e.key === 'Backspace') {
      if (lab.selectedWire) lab.removeWire(lab.selectedWire);
      else if (lab.selected.length) lab.removeParts(lab.selected);
      else return;
      e.preventDefault();
    } else if (e.key === 'r' && one) lab.rotate(one.id);
    else if (e.key === 'Escape') {
      lab.selected = [];
      lab.selectedWire = null;
    }
  }

  // Palette: drag a part out onto the canvas (or click it to add it).
  function downPalette(e: PointerEvent, type: string) {
    if (e.button !== 0) return;
    dragNew = { type, cx: e.clientX, cy: e.clientY };
    const move = (ev: PointerEvent) => dragNew && (dragNew = { ...dragNew, cx: ev.clientX, cy: ev.clientY });
    const up = (ev: PointerEvent) => {
      window.removeEventListener('pointermove', move);
      const r = svg!.getBoundingClientRect();
      const inside = ev.clientX > r.left && ev.clientX < r.right && ev.clientY > r.top && ev.clientY < r.bottom;
      const moved = dragNew && Math.hypot(ev.clientX - e.clientX, ev.clientY - e.clientY) > 6;
      if (dragNew && inside && moved) {
        const w = world(ev);
        const s = bodySize(PART[dragNew.type]);
        lab.addPart(dragNew.type, w.x - s.w / 2, w.y - s.h / 2);
      } else if (dragNew && !moved) {
        const at = { x: panX + W / zoom / 2 - 40, y: panY + H / zoom / 2 - 40 };
        lab.addPart(dragNew.type, at.x, at.y);
      }
      dragNew = null;
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up, { once: true });
  }

  // ------------------------------------------------------------------ drawing helpers

  const MONO = 5.4; // width of a 9 px monospace character
  const low = (name: string) => name.startsWith('/');
  const bare = (name: string) => name.replace(/^\//, '');

  function pinInfo(part: Placed, n: number) {
    const def = PART[part.type];
    const pin = def.pins.find((x) => x.n === n);
    const net = netOf({ part: part.id, pin: n });
    const v = lab.view && net >= 0 ? lab.view.val[net] : Z;
    const level = !lab.power ? '' : v === HI ? ' · now high (1)' : v === LO ? ' · now low (0)' : v === X ? ' · unknown (two drivers fight, or a floating input)' : ' · floating';
    const others = sim && net >= 0 ? sim.netName(net).split(', ').filter((x) => !x.startsWith(`${part.ref}.${n}`)) : [];
    return `${part.ref} pin ${n} · ${pin?.name ?? n}${pin?.desc ? ` — ${pin.desc}` : ''}${level}${others.length ? `\nWired to: ${others.slice(0, 8).join(', ')}${others.length > 8 ? '…' : ''}` : '\nNot wired'}`;
  }

  const lightColor: Record<string, string> = { red: '#ff3b30', green: '#30d158', yellow: '#ffd60a', blue: '#0a84ff', white: '#f5f5f7' };
  const fmtOhms = (o: number) => (o >= 1e6 ? `${o / 1e6}M` : o >= 1000 ? `${o / 1000}k` : `${o}`) + 'Ω';
  const fmtF = (f: number) => (f >= 1e-6 ? `${+(f * 1e6).toPrecision(3)}µF` : f >= 1e-9 ? `${+(f * 1e9).toPrecision(3)}nF` : `${+(f * 1e12).toPrecision(3)}pF`);
  const fmtHz = (h: number) => (h >= 1e6 ? `${+(h / 1e6).toPrecision(5)} MHz` : h >= 1e3 ? `${+(h / 1e3).toPrecision(4)} kHz` : `${h} Hz`);

  function openDoc(url: string) {
    store.runAction({ kind: 'open_url', url });
  }

  function applyGal() {
    if (!galDraft) return;
    lab.setProp(galDraft.id, 'names', galDraft.names);
    lab.setProp(galDraft.id, 'equations', galDraft.equations);
  }

  $effect(() => {
    if (one?.type === 'gal16v8' && galDraft?.id !== one.id) galDraft = { id: one.id, names: String(one.props.names ?? ''), equations: String(one.props.equations ?? '') };
    if (!one || one.type !== 'gal16v8') galDraft = null;
  });

  const galNames = (part: Placed) => String(part.props.names ?? '').trim().split(/\s+/);
</script>

<svelte:window onkeydown={key} />

<div class="wrap">
  <aside class="palette">
    {#each CATEGORIES as cat (cat)}
      <button class="cat" onclick={() => (paletteOpen[cat] = !paletteOpen[cat])}>{#if paletteOpen[cat]}<ChevronDown size={12} />{:else}<ChevronRight size={12} />{/if}{cat}</button>
      {#if paletteOpen[cat]}
        {#each PARTS.filter((x) => x.category === cat) as def (def.type)}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div class="item" title="{def.desc}{'\n'}Drag it onto the board (or click to add it)." onpointerdown={(e) => downPalette(e, def.type)}>
            <span class="ico" class:chip={def.pkg.kind === 'dip'}>{def.pkg.kind === 'dip' ? def.pkg.pins : def.ref}</span>
            <span class="truncate">{def.label}</span>
          </div>
        {/each}
      {/if}
    {/each}
  </aside>

  <div class="stage" bind:clientWidth={W} bind:clientHeight={H}>
    <svg bind:this={svg} viewBox={vb} onwheel={wheel} class:wiring={!!wiring} class:busy role="application" aria-label="Circuit board">
      <defs>
        <pattern id="lab-grid" width="20" height="20" patternUnits="userSpaceOnUse"><circle cx="0" cy="0" r="0.9" class="gridDot" /></pattern>
        <filter id="lab-glow" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="4" result="b" /><feMerge><feMergeNode in="b" /><feMergeNode in="SourceGraphic" /></feMerge></filter>
      </defs>
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <rect class="bg" x={panX - 5000} y={panY - 5000} width={W / zoom + 10000} height={H / zoom + 10000} fill="url(#lab-grid)" onpointerdown={downBg} onpointermove={moveBg} onpointerup={() => (pan = null)} />

      {#if p}
        <!-- wires under the parts -->
        <g class="wires">
          {#each p.wires as w (w.id)}
            {@const d = route(w)}
            {@const f = flows.get(w.id)}
            {@const cls = wireClass(w)}
            <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
            <g class="wire {cls}" class:sel={lab.selectedWire === w.id} onclick={(e) => { e.stopPropagation(); lab.selectedWire = w.id; lab.selected = []; }}>
              <path class="hit" {d} />
              <path class="base" {d} />
              {#if lab.power && f && f.dir !== 0 && cls !== 'z'}
                <path class="flow" class:rev={f.dir < 0} {d} />
              {/if}
            </g>
          {/each}
        </g>

        {#each p.parts as part (part.id)}
          {@const def = PART[part.type] as PartDef}
          {#if def}
            {@const s = bodySize(def)}
            {@const lit = lab.view?.lights.get(part.id) ?? 0}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <g class="part {part.type}" class:sel={lab.selected.includes(part.id)} class:answer={answering === part.ref || (answering && part.type === 'z80' && lab.watching)} transform="translate({part.x},{part.y})">
              <!-- pins -->
              {#each def.pins as pin (pin.n)}
                {@const q = pinPos(def, pin.n, part.rot ?? 0)}
                {@const net = netOf({ part: part.id, pin: pin.n })}
                {@const v = lab.power && lab.view && net >= 0 ? lab.view.val[net] : -1}
                <line class="stub" x1={q.x} y1={q.y} x2={q.x - q.dx * STUB} y2={q.y - q.dy * STUB} />
                <!-- svelte-ignore a11y_no_static_element_interactions -->
                <circle class="pin {pin.kind}" class:hi={v === HI} class:lo={v === LO} class:hot={hover?.part === part.id && hover.pin === pin.n} cx={q.x} cy={q.y} r="4" onpointerdown={(e) => downPin(e, part, pin.n)} onpointerenter={() => (hover = { part: part.id, pin: pin.n })} onpointerleave={() => (hover = null)}>
                  <title>{pinInfo(part, pin.n)}</title>
                </circle>
                {#if def.pkg.kind === 'dip'}
                  <text class="pnum" x={q.dx < 0 ? q.x + 6 : q.x - 6} y={q.y - 4} text-anchor={q.dx < 0 ? 'start' : 'end'}>{pin.n}</text>
                {/if}
              {/each}

              <!-- the body -->
              {#if def.pkg.kind === 'dip'}
                <rect class="body" width={s.w} height={s.h} rx="4" onpointerdown={(e) => downPart(e, part)} onpointermove={movePart} onpointerup={(e) => upPart(e, part)} />
                <path class="notch" d="M{s.w / 2 - 9},0 a9,9 0 0,0 18,0" />
                {#each def.pins as pin (pin.n)}
                  {@const q = pinPos(def, pin.n)}
                  {@const name = part.type === 'gal16v8' ? (galNames(part)[pin.n - 1] ?? pin.name) : pin.name}
                  {@const tx = q.dx < 0 ? 8 : s.w - 8}
                  <text class="pname" x={tx} y={q.y + 3} text-anchor={q.dx < 0 ? 'start' : 'end'}>{bare(name)}</text>
                  {#if low(name)}
                    {@const len = bare(name).length * MONO}
                    <line class="bar" x1={q.dx < 0 ? tx : tx - len} x2={q.dx < 0 ? tx + len : tx} y1={q.y - 6} y2={q.y - 6} />
                  {/if}
                {/each}
                <text class="chipname" transform="translate({s.w / 2},{s.h / 2}) rotate(-90)" text-anchor="middle">{def.label.replace(/ \(.*\)$/, '')}</text>
                <text class="sub" transform="translate({s.w / 2 + 13},{s.h / 2}) rotate(-90)" text-anchor="middle">{part.type === 'osc' ? fmtHz(Number(part.props.hz)) : (def.label.match(/\((.*)\)$/)?.[1] ?? '')}</text>
              {:else if def.pkg.kind === 'two'}
                {@const v = part.rot === 90}
                <g transform={v ? `rotate(90 ${s.w / 2} ${s.h / 2})` : ''}>
                  <rect class="hitbody" x="-4" y="-6" width={s.w + 8} height={s.h + 12} onpointerdown={(e) => downPart(e, part)} onpointermove={movePart} onpointerup={(e) => upPart(e, part)} />
                  {#if part.type === 'resistor'}
                    <line class="lead" x1="0" y1={s.h / 2} x2="12" y2={s.h / 2} /><line class="lead" x1={s.w - 12} y1={s.h / 2} x2={s.w} y2={s.h / 2} />
                    <rect class="sym" x="12" y={s.h / 2 - 6} width={s.w - 24} height="12" rx="2" />
                  {:else if part.type === 'capacitor'}
                    <line class="lead" x1="0" y1={s.h / 2} x2={s.w / 2 - 4} y2={s.h / 2} /><line class="lead" x1={s.w / 2 + 4} y1={s.h / 2} x2={s.w} y2={s.h / 2} />
                    <line class="plate" x1={s.w / 2 - 4} y1="1" x2={s.w / 2 - 4} y2={s.h - 1} /><line class="plate" x1={s.w / 2 + 4} y1="1" x2={s.w / 2 + 4} y2={s.h - 1} />
                  {:else if part.type === 'led'}
                    <line class="lead" x1="0" y1={s.h / 2} x2="20" y2={s.h / 2} /><line class="lead" x1="38" y1={s.h / 2} x2={s.w} y2={s.h / 2} />
                    <path class="sym led" d="M20,{s.h / 2 - 9} L38,{s.h / 2} L20,{s.h / 2 + 9} Z" style={lit ? `fill:${lightColor[part.props.color] ?? '#ff3b30'}` : ''} filter={lit ? 'url(#lab-glow)' : ''} />
                    <line class="plate" x1="38" y1={s.h / 2 - 9} x2="38" y2={s.h / 2 + 9} />
                  {:else}
                    {@const closed = part.type === 'switch' ? !!part.props.on : lab.sim?.pressed.has(part.id)}
                    <line class="lead" x1="0" y1={s.h / 2} x2="16" y2={s.h / 2} /><line class="lead" x1={s.w - 16} y1={s.h / 2} x2={s.w} y2={s.h / 2} />
                    <circle class="contact" cx="16" cy={s.h / 2} r="2.5" /><circle class="contact" cx={s.w - 16} cy={s.h / 2} r="2.5" />
                    <line class="lever" class:closed x1="16" y1={s.h / 2} x2={s.w - 16} y2={closed ? s.h / 2 : s.h / 2 - 10} />
                    {#if part.type === 'button'}<line class="lever" x1={s.w / 2} y1={closed ? s.h / 2 - 2 : s.h / 2 - 6} x2={s.w / 2} y2="-6" />{/if}
                  {/if}
                </g>
                <text class="ref2" x={v ? s.w / 2 + 16 : s.w / 2} y={v ? s.h / 2 : -8} text-anchor={v ? 'start' : 'middle'}>{part.ref}{part.type === 'resistor' ? ` ${fmtOhms(Number(part.props.ohms))}` : part.type === 'capacitor' ? ` ${fmtF(Number(part.props.farads))}` : ''}</text>
              {:else}
                <rect class="body box" width={s.w} height={s.h} rx="6" onpointerdown={(e) => downPart(e, part)} onpointermove={movePart} onpointerup={(e) => upPart(e, part)} />
                {#each def.pins as pin (pin.n)}
                  {@const q = pinPos(def, pin.n)}
                  <text class="pname" x={s.w - 8} y={q.y + 3} text-anchor="end">{pin.name}</text>
                {/each}
                {#if part.type === 'psu'}
                  <text class="chipname" x="10" y={s.h - 12}>5 V</text>
                  <circle class="pwr" class:on={lab.power} cx="16" cy="16" r="5" />
                {:else if part.type === 'terminal'}
                  <rect class="screen" x="8" y="10" width="34" height="24" rx="2" /><text class="sub" x="10" y={s.h - 10}>USB</text>
                {:else if part.type === 'ledbar'}
                  {#each [0, 1, 2, 3, 4, 5, 6, 7] as i (i)}
                    <rect class="seg" class:lit={(lit >> i) & 1} x="10" y={PITCH + i * PITCH - 6} width="22" height="12" rx="2" filter={(lit >> i) & 1 ? 'url(#lab-glow)' : ''} />
                  {/each}
                {:else if part.type === 'rnet'}
                  <text class="sub" transform="translate(16,{s.h / 2}) rotate(-90)" text-anchor="middle">8 × {fmtOhms(Number(part.props.ohms))}</text>
                {/if}
              {/if}
              {#if def.pkg.kind !== 'two'}<text class="ref" x="0" y="-8">{part.ref}</text>{/if}
            </g>
          {/if}
        {/each}

        {#if wiring}
          {@const a = parts.get(wiring.from.part)}
          {#if a}
            {@const A = end(a, wiring.from.pin)}
            <line class="rubber" x1={A.x} y1={A.y} x2={wiring.x} y2={wiring.y} />
          {/if}
        {/if}
      {/if}
    </svg>

    <div class="tools">
      <button class="icon-btn sm" title="Zoom in" onclick={() => (zoom = Math.min(3, zoom * 1.25))}><ZoomIn size={14} /></button>
      <button class="icon-btn sm" title="Zoom out" onclick={() => (zoom = Math.max(0.15, zoom / 1.25))}><ZoomOut size={14} /></button>
      <button class="icon-btn sm" title="Fit the board" onclick={fit}><Maximize size={14} /></button>
      {#if sel.length === 2}
        <button class="btn sm" title="Wire the pins that carry the same signal (A0–A15, D0–D7, /RD to /OE, VCC, GND…)" onclick={() => { const n = lab.connectMatching(sel[0].id, sel[1].id); store.toast(n ? 'success' : 'info', n ? `Wired ${n} pins` : 'Nothing to wire', n ? `${sel[0].ref} ↔ ${sel[1].ref}` : 'Those pins are wired already, or nothing matches.'); }}><Cable size={13} /> Connect matching pins</button>
      {/if}
      {#if lab.selectedWire}
        <button class="btn sm" onclick={() => lab.removeWire(lab.selectedWire!)}><Trash2 size={13} /> Delete wire</button>
      {/if}
    </div>

    {#if lab.watching && lab.running && lab.view?.access}
      {@const a = lab.view.access}
      <div class="cycle">
        <strong>{a.kind === 'fetch' ? 'Opcode fetch' : a.kind === 'read' ? 'Memory read' : a.kind === 'write' ? 'Memory write' : a.kind === 'in' ? 'I/O read' : a.kind === 'out' ? 'I/O write' : 'Interrupt'}</strong>
        {a.kind === 'in' || a.kind === 'out' ? `port ${hex(a.addr & 0xff)}` : hex(a.addr, 4)} · {hex(a.data)} {a.kind === 'write' || a.kind === 'out' ? '→' : '←'} {a.who ?? 'nobody answered'}
      </div>
    {/if}

    {#if one}
      {@const def = PART[one.type]}
      <div class="inspect">
        <div class="ihead">
          <input class="input sm refin" value={one.ref} title="Reference" onchange={(e) => { if (!lab.setRef(one.id, e.currentTarget.value)) e.currentTarget.value = one.ref; }} />
          <strong class="truncate">{def.label}</strong>
          <span class="spacer"></span>
          {#if def.pkg.kind === 'two'}<button class="icon-btn sm" title="Turn (R)" onclick={() => lab.rotate(one.id)}><RotateCw size={13} /></button>{/if}
          {#if def.doc}<button class="icon-btn sm" title="Datasheet" onclick={() => openDoc(def.doc!)}><FileText size={13} /></button>{/if}
          <button class="icon-btn sm" title="Delete (⌫)" onclick={() => lab.removeParts([one.id])}><Trash2 size={13} /></button>
          <button class="icon-btn sm" title="Close" onclick={() => (lab.selected = [])}><Close size={13} /></button>
        </div>
        <p class="desc">{def.desc}</p>
        {#if one.type === 'osc'}
          <label class="row">Frequency <select class="input sm" value={String(one.props.hz)} onchange={(e) => lab.setProp(one.id, 'hz', Number(e.currentTarget.value))}>{#each [1_000_000, 1_843_200, 2_000_000, 4_000_000, 7_372_800, 8_000_000, 10_000_000, 20_000_000] as h (h)}<option value={String(h)}>{fmtHz(h)}</option>{/each}</select></label>
        {:else if one.type === 'resistor' || one.type === 'rnet'}
          <label class="row">Resistance <select class="input sm" value={String(one.props.ohms)} onchange={(e) => lab.setProp(one.id, 'ohms', Number(e.currentTarget.value))}>{#each [100, 220, 330, 470, 1000, 2200, 4700, 10_000, 47_000, 100_000] as o (o)}<option value={String(o)}>{fmtOhms(o)}</option>{/each}</select></label>
        {:else if one.type === 'capacitor'}
          <label class="row">Capacitance <select class="input sm" value={String(one.props.farads)} onchange={(e) => lab.setProp(one.id, 'farads', Number(e.currentTarget.value))}>{#each [1e-11, 1e-10, 1e-9, 1e-8, 1e-7, 1e-6, 1e-5, 4.7e-5] as f (f)}<option value={String(f)}>{fmtF(f)}</option>{/each}</select></label>
        {:else if one.type === 'led'}
          <label class="row">Colour <select class="input sm" value={one.props.color} onchange={(e) => lab.setProp(one.id, 'color', e.currentTarget.value)}>{#each Object.keys(lightColor) as c (c)}<option>{c}</option>{/each}</select></label>
        {:else if one.type === 'gal16v8' && galDraft}
          <label class="col">Pin names (1–20)<input class="input sm mono" bind:value={galDraft.names} /></label>
          <label class="col">Equations (/OUT = A * /B + C; “;” starts a comment)<textarea class="input mono" rows="6" bind:value={galDraft.equations}></textarea></label>
          {#each lab.sim?.galOf(one.id)?.errors ?? [] as err (err)}<p class="err">{err}</p>{/each}
          <button class="btn sm primary" onclick={applyGal}>Program the GAL</button>
        {/if}
        <details>
          <summary>Pins</summary>
          <div class="pins">
            {#each def.pins as pin (pin.n)}
              {@const net = netOf({ part: one.id, pin: pin.n })}
              {@const v = lab.power && lab.view && net >= 0 ? lab.view.val[net] : -1}
              <span class="pn">{pin.n}</span><span class="pnm" class:ovl={low(pin.name)}>{bare(pin.name)}</span><span class="lv" class:hi={v === HI} class:lo={v === LO}>{v === HI ? '1' : v === LO ? '0' : v === X ? '?' : lab.power ? '–' : ''}</span>
              <span class="to truncate" title={sim && net >= 0 ? sim.netName(net) : ''}>{sim && net >= 0 ? sim.netName(net).split(', ').filter((x) => !x.startsWith(`${one.ref}.${pin.n}`)).join(', ') || '—' : '—'}</span>
            {/each}
          </div>
        </details>
      </div>
    {/if}
  </div>

  {#if dragNew}
    <div class="ghost" style="left:{dragNew.cx}px; top:{dragNew.cy}px">{PART[dragNew.type].label}</div>
  {/if}
</div>

<style>
  .wrap {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    position: relative;
  }
  .palette {
    width: 178px;
    flex: none;
    overflow: auto;
    border-right: 1px solid var(--border);
    padding: 6px;
    font-size: 12px;
    background: var(--surface);
  }
  .cat {
    display: flex;
    align-items: center;
    gap: 3px;
    width: 100%;
    margin-top: 6px;
    padding: 2px 2px;
    border: 0;
    background: none;
    color: var(--text-3);
    font-size: 11px;
    font-weight: 650;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    cursor: pointer;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 4px 6px;
    border-radius: var(--r-sm);
    cursor: grab;
    user-select: none;
    color: var(--text-2);
  }
  .item:hover {
    background: var(--surface-2);
    color: var(--text);
  }
  .ico {
    width: 24px;
    height: 16px;
    flex: none;
    display: grid;
    place-items: center;
    font-size: 9px;
    font-weight: 700;
    border-radius: 3px;
    border: 1px solid var(--border-strong);
    color: var(--text-3);
  }
  .ico.chip {
    background: #2b2b30;
    color: #ddd;
    border-color: #2b2b30;
  }
  .stage {
    flex: 1;
    min-width: 0;
    position: relative;
    overflow: hidden;
    --board: #f4f6f1;
    --grid: #c9d2c4;
    --hi: #ef4444;
    --lo: #2563eb;
    --zw: #9ca3af;
    --vcc: #dc2626;
    --gnd: #374151;
    --chip: #26262b;
    --chip-text: #e7e7ea;
    background: var(--board);
  }
  @media (prefers-color-scheme: dark) {
    :global(:root:not([data-theme='light'])) .stage {
      --board: #15181b;
      --grid: #2c3338;
      --gnd: #9ca3af;
      --lo: #3b82f6;
    }
  }
  :global(:root[data-theme='dark']) .stage {
    --board: #15181b;
    --grid: #2c3338;
    --gnd: #9ca3af;
    --lo: #3b82f6;
  }
  svg {
    width: 100%;
    height: 100%;
    display: block;
    touch-action: none;
    user-select: none;
  }
  .gridDot {
    fill: var(--grid);
  }
  .bg {
    cursor: grab;
  }
  .wire .hit {
    stroke: transparent;
    stroke-width: 8;
    fill: none;
    cursor: pointer;
  }
  .wire .base {
    fill: none;
    stroke: var(--zw);
    stroke-width: 1.6;
    stroke-linejoin: round;
    opacity: 0.85;
  }
  .wire.idle .base {
    stroke: #8b939c;
  }
  .wire.hi .base {
    stroke: var(--hi);
  }
  .wire.lo .base {
    stroke: var(--lo);
    opacity: 0.55;
  }
  .wire.z .base {
    stroke-dasharray: 3 3;
  }
  .wire.x .base {
    stroke: #d946ef;
  }
  .wire.vcc .base {
    stroke: var(--vcc);
    stroke-width: 2.2;
  }
  .wire.gnd .base {
    stroke: var(--gnd);
    stroke-width: 2.2;
  }
  .wire.sel .base {
    stroke-width: 3.5;
    opacity: 1;
  }
  .flow {
    fill: none;
    stroke: #fff;
    stroke-width: 1.4;
    stroke-dasharray: 2 9;
    stroke-linecap: round;
    opacity: 0.85;
    animation: flow 0.9s linear infinite;
    pointer-events: none;
  }
  .wire.lo .flow {
    opacity: 0.35;
    animation-duration: 1.8s;
  }
  .wire.vcc .flow,
  .wire.gnd .flow {
    opacity: 0.6;
    animation-duration: 1.4s;
  }
  .busy .wire.hi .flow,
  .busy .wire.lo .flow {
    animation-duration: 0.25s;
  }
  .flow.rev {
    animation-direction: reverse;
  }
  @keyframes flow {
    to {
      stroke-dashoffset: -22;
    }
  }
  .stub {
    stroke: #9aa0a6;
    stroke-width: 2;
  }
  .pin {
    fill: #c0c4c9;
    stroke: #6b7280;
    stroke-width: 1;
    cursor: crosshair;
  }
  .pin.pwr {
    fill: #fca5a5;
  }
  .pin.gnd {
    fill: #9ca3af;
  }
  .pin.hi {
    fill: var(--hi);
  }
  .pin.lo {
    fill: var(--lo);
  }
  .pin.hot {
    r: 6;
    stroke: var(--accent);
    stroke-width: 2;
  }
  .pnum {
    font: 600 7px var(--font-mono, monospace);
    fill: #8b939c;
    pointer-events: none;
  }
  .body {
    fill: var(--chip);
    stroke: #111;
    stroke-width: 1;
    cursor: move;
  }
  .body.box {
    fill: #33373d;
  }
  .part.sel .body,
  .part.sel .hitbody {
    stroke: var(--accent);
    stroke-width: 2.5;
  }
  .part.answer .body {
    stroke: #facc15;
    stroke-width: 3;
    filter: url(#lab-glow);
  }
  .notch {
    fill: #46464d;
    pointer-events: none;
  }
  .pname {
    font: 9px var(--font-mono, ui-monospace, monospace);
    fill: var(--chip-text);
    pointer-events: none;
  }
  .bar {
    stroke: var(--chip-text);
    stroke-width: 0.8;
    pointer-events: none;
  }
  .chipname {
    font: 700 12px var(--font-sans, system-ui);
    fill: #fafafa;
    pointer-events: none;
  }
  .sub {
    font: 9px var(--font-sans, system-ui);
    fill: #b4b4bb;
    pointer-events: none;
  }
  .ref,
  .ref2 {
    font: 600 10px var(--font-sans, system-ui);
    fill: var(--text-2);
    pointer-events: none;
  }
  .hitbody {
    fill: transparent;
    stroke: none;
    cursor: move;
    rx: 4;
  }
  .lead,
  .plate,
  .lever {
    stroke: #6b7280;
    stroke-width: 2;
    pointer-events: none;
  }
  .plate {
    stroke-width: 3;
  }
  .sym {
    fill: #d6c7a1;
    stroke: #6b7280;
    stroke-width: 1.2;
    pointer-events: none;
  }
  .sym.led {
    fill: #7f1d1d;
  }
  .contact {
    fill: #6b7280;
    pointer-events: none;
  }
  .lever.closed {
    stroke: #16a34a;
  }
  .button .hitbody,
  .switch .hitbody {
    cursor: pointer;
  }
  .pwr {
    fill: #555;
  }
  .pwr.on {
    fill: #22c55e;
  }
  .screen {
    fill: #0b1d0e;
    stroke: #3f3f46;
  }
  .seg {
    fill: #3f1d1d;
  }
  .seg.lit {
    fill: #ff3b30;
  }
  .rubber {
    stroke: var(--accent);
    stroke-width: 2;
    stroke-dasharray: 4 3;
    pointer-events: none;
  }
  .tools {
    position: absolute;
    top: 8px;
    left: 8px;
    display: flex;
    gap: 4px;
    padding: 3px;
    border-radius: var(--r-sm);
    background: var(--glass, var(--surface));
    border: 1px solid var(--border);
  }
  .cycle {
    position: absolute;
    top: 8px;
    left: 50%;
    transform: translateX(-50%);
    padding: 5px 12px;
    border-radius: 999px;
    background: var(--surface);
    border: 1px solid var(--border-strong);
    font: 12px var(--font-mono, monospace);
    box-shadow: var(--shadow);
    white-space: nowrap;
  }
  .inspect {
    position: absolute;
    left: 8px;
    bottom: 8px;
    width: 330px;
    max-height: calc(100% - 60px);
    overflow: auto;
    padding: 10px;
    border-radius: var(--r);
    background: var(--surface);
    border: 1px solid var(--border-strong);
    box-shadow: var(--shadow);
    font-size: 12.5px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .ihead {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .refin {
    width: 56px;
    font-weight: 700;
  }
  .spacer {
    flex: 1;
  }
  .desc {
    margin: 0;
    color: var(--text-2);
    line-height: 1.45;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-3);
  }
  .col {
    display: flex;
    flex-direction: column;
    gap: 4px;
    color: var(--text-3);
  }
  .mono {
    font-family: var(--font-mono, monospace);
    font-size: 11.5px;
  }
  .err {
    margin: 0;
    color: #dc2626;
  }
  details summary {
    cursor: pointer;
    color: var(--text-3);
  }
  .pins {
    display: grid;
    grid-template-columns: 22px 64px 14px 1fr;
    gap: 2px 6px;
    margin-top: 6px;
    font: 11px var(--font-mono, monospace);
  }
  .pn {
    color: var(--text-3);
    text-align: right;
  }
  .ovl {
    text-decoration: overline;
  }
  .lv.hi {
    color: var(--hi);
    font-weight: 700;
  }
  .lv.lo {
    color: var(--lo);
    font-weight: 700;
  }
  .to {
    color: var(--text-2);
  }
  .ghost {
    position: fixed;
    z-index: 50;
    transform: translate(-50%, -50%);
    padding: 6px 10px;
    border-radius: var(--r-sm);
    background: var(--chip, #26262b);
    color: #eee;
    font-size: 12px;
    pointer-events: none;
    box-shadow: var(--shadow);
    opacity: 0.9;
  }
</style>
