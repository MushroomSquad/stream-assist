<script lang="ts">
  import * as api from '$lib/api';
  import { ru } from '$lib/i18n/ru';
  let { snapshot, run }: { snapshot: api.WidgetState; run: (f: () => Promise<unknown>) => void } = $props();
  let volume = $state(50);
  let dragged: number | null = null;
</script>
<section>
  <h2>{ru.now}</h2>
  {#if snapshot.warning}<p class="notice" role="alert">{snapshot.warning}</p>{/if}
  {#if snapshot.mode === 'Suspended'}<button class="primary" onclick={() => run(api.resume_queue)}>{ru.resume}</button>{/if}
  <div class="card">
    <h3>{snapshot.now ? snapshot.now.track.title : ru.empty}</h3>
    {#if snapshot.now}<p>{snapshot.now.track.artist} · {snapshot.now.requester}</p>{/if}
    <progress aria-label={ru.now} value={snapshot.progress_ms} max={snapshot.now?.track.duration_ms || 1}></progress>
    <div class="actions"><button disabled={!snapshot.now} onclick={() => run(api.skip)}>{ru.skip}</button><button disabled={!snapshot.now} onclick={() => run(api.toggle_pause)}>{snapshot.mode === 'Paused' ? ru.play : ru.pause}</button></div>
    <label>{ru.volume} · {volume}%<input type="range" min="0" max="100" bind:value={volume} onchange={() => run(() => api.set_volume(volume))} /></label>
  </div>
  <h3>{ru.queue} · {snapshot.queue.length}</h3><p class="muted">{ru.drag}</p>
  <ol aria-label={ru.queue}>
    {#each snapshot.queue as item, index (item.id)}
      <li draggable="true" ondragstart={(e) => { dragged = item.id; e.dataTransfer?.setData('text/plain', String(item.id)); }} ondragover={e => e.preventDefault()} ondrop={e => { e.preventDefault(); if (dragged !== null) run(() => api.move_item(dragged!, index)); dragged = null; }}>
        <span><strong>{item.track.title}</strong><br />{item.track.artist} · {item.requester}</span><button onclick={() => run(() => api.remove_item(item.id))}>{ru.remove}</button>
      </li>
    {:else}<p class="muted">{ru.emptyQueue}</p>{/each}
  </ol>
</section>
