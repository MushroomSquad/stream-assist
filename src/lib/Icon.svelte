<script lang="ts">
const paths = {
  now: 'M9 18V5l12-2v13 M9 18c0 1.66-1.79 3-4 3s-4-1.34-4-3 1.79-3 4-3 4 1.34 4 3 M21 16c0 1.66-1.79 3-4 3s-4-1.34-4-3 1.79-3 4-3 4 1.34 4 3',
  rules: 'M4 7h16 M4 17h16 M8 4v6 M16 14v6',
  widget: 'M3 4h18v13H3z M8 21h8 M12 17v4',
  connections: 'M8 3v5 M16 3v5 M6 8h12v3a6 6 0 0 1-12 0z M12 17v4',
  voice: 'M9 5a3 3 0 0 1 6 0v7a3 3 0 0 1-6 0z M5 10v2a7 7 0 0 0 14 0v-2 M12 19v3 M8 22h8'
};
let { name }: { name: keyof typeof paths } = $props();
</script>

<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d={paths[name]} /></svg>
