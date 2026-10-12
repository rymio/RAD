<script lang="ts">
  let { size = 28 }: { size?: number } = $props();
  const id = `g${Math.random().toString(36).slice(2, 8)}`;
</script>

<svg width={size} height={size} viewBox="64 64 896 896" aria-label="RAD">
  <defs>
    <linearGradient id="{id}bg" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" style="stop-color: color-mix(in oklab, var(--accent) 70%, white)" />
      <stop offset="0.55" style="stop-color: var(--accent)" />
      <stop offset="1" style="stop-color: color-mix(in oklab, var(--accent) 60%, #1a1060)" />
    </linearGradient>
  </defs>
  <rect x="64" y="64" width="896" height="896" rx="210" fill="url(#{id}bg)" />
  <circle cx="512" cy="512" r="250" fill="none" stroke="white" stroke-width="58" opacity="0.95" />
  <path d="M512 352 C 528 456, 568 496, 672 512 C 568 528, 528 568, 512 672 C 496 568, 456 528, 352 512 C 456 496, 496 456, 512 352 Z" fill="white" />
</svg>
