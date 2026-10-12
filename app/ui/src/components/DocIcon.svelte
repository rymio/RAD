<script lang="ts">
  import { FileText, StickyNote, Image, Music, Film, FileSpreadsheet, Presentation, FileCode, Globe, Mail, FileArchive, File, BookText, MessagesSquare } from '@lucide/svelte';
  import { kindColor, type DocKind } from '../lib/util';

  let { kind, size = 18, tile = false }: { kind: DocKind; size?: number; tile?: boolean } = $props();
  const icons: Record<DocKind, any> = {
    chat: MessagesSquare, note: StickyNote, pdf: BookText, doc: FileText, sheet: FileSpreadsheet, slides: Presentation, image: Image,
    audio: Music, video: Film, code: FileCode, web: Globe, mail: Mail, archive: FileArchive, text: FileText, file: File,
  };
  const Icon = $derived(icons[kind]);
</script>

{#if tile}
  <span class="tile" style="--c:{kindColor[kind]}"><Icon size={size} strokeWidth={1.8} /></span>
{:else}
  <span style="color:{kindColor[kind]};display:inline-flex"><Icon size={size} strokeWidth={1.8} /></span>
{/if}

<style>
  .tile {
    display: inline-grid;
    place-items: center;
    width: calc(var(--s, 1) * 34px);
    height: calc(var(--s, 1) * 34px);
    border-radius: 10px;
    color: var(--c);
    background: color-mix(in oklab, var(--c) 13%, transparent);
    flex: none;
  }
</style>
