<script lang="ts">
  import { onMount } from 'svelte';
  import { formatTime as time } from '$lib/time';
  import Icon from '$lib/Icon.svelte';
  import * as api from '$lib/api';
  import { errorMessages, errorText, ru } from '$lib/i18n/ru';
  let { snapshot, run }: { snapshot: api.WidgetState; run: (f: () => Promise<unknown>) => void } = $props();
  let volume = $state(50);
  let volumeKnown = $state(false);
  const apiWarning = (warning: string) => warning in errorMessages ? errorText(warning) : warning;
  onMount(() => { let disposed = false; void api.spotify_volume().then(value => { if (!disposed && !volumeKnown && value !== null) { volume = value; volumeKnown = true; } }).catch(() => {}); return () => { disposed = true; }; });
  let dragged: number | null = null;
  const played = $derived(snapshot.now?.track.duration_ms ? Math.min(100, Math.max(0, snapshot.progress_ms / snapshot.now.track.duration_ms * 100)) : 0);
</script>
<section>
  <header class="page-heading"><h2>{ru.now}</h2></header>
  {#if snapshot.warning && !['server_port_busy', 'server_start_failed'].includes(snapshot.warning)}<p class="notice" role="alert">{apiWarning(snapshot.warning)}</p>{/if}
  <div class="card deck">
    <div class="cover" aria-hidden="true"><Icon name="now" /></div>
    <div class="track">
      {#if snapshot.now}<p class="play-state" class:live={!!snapshot.now && snapshot.mode === 'Playing'}><span class="tally" class:live={!!snapshot.now && snapshot.mode === 'Playing'}></span>{snapshot.now ? (snapshot.mode === 'Paused' ? ru.pause : snapshot.mode === 'Suspended' ? ru.queueSuspended : ru.live) : ru.nowPlaying}{#if snapshot.now.track.source} · {'Spotify' in snapshot.now.track.source ? ru.spotify : 'YouTube'}{/if}</p>{/if}
      <h3>{snapshot.now ? snapshot.now.track.title : ru.empty}</h3>
      {#if snapshot.now}<p class="artist">{snapshot.now.track.artist}</p><p class="requester">{ru.requestedBy} <strong>{snapshot.now.requester}</strong></p>{/if}
      <div class="timeline" style:--played={`${played}%`}><progress aria-label={ru.nowPlaying} value={snapshot.progress_ms} max={snapshot.now?.track.duration_ms || 1}></progress><span class="ticks" aria-hidden="true"></span></div>
      <div class="times"><span>{time(snapshot.progress_ms)}</span><span>{time(snapshot.now?.track.duration_ms ?? null)}</span></div>
      <div class="actions"><button class="primary" disabled={!snapshot.now} onclick={() => run(api.skip)}>{ru.skip}</button><button disabled={!snapshot.now} onclick={() => run(api.toggle_pause)}>{snapshot.mode === 'Paused' ? ru.play : ru.pause}</button>{#if snapshot.mode === 'Suspended'}<button onclick={() => run(api.resume_queue)}>{ru.resume}</button>{/if}</div>
      <label class="volume">{ru.volume}<input type="range" min="0" max="100" bind:value={volume} style:--volume={`${volume}%`} onchange={() => { volumeKnown = true; run(() => api.set_volume(volume)); }} /><span class="meta">{volumeKnown ? `${volume}%` : '—'}</span></label>
    </div>
  </div>
  <header class="queue-heading"><h3>{ru.queue} <span class="meta">{snapshot.queue.length}</span></h3><p class="muted">{ru.drag}</p></header>
  <ol aria-label={ru.queue}>
    {#each snapshot.queue as item, index (item.id)}
      <li draggable="true" ondragstart={(e) => { dragged = item.id; e.dataTransfer?.setData('text/plain', String(item.id)); }} ondragover={e => e.preventDefault()} ondrop={e => { e.preventDefault(); if (dragged !== null) run(() => api.move_item(dragged!, index)); dragged = null; }}>
        <span class="meta">{index + 1}</span><span class="queue-track"><strong>{item.track.title}</strong><span class="muted">{' · '}{item.track.artist}</span></span><span class="viewer">{item.requester}</span><span class="meta">{time(item.track.duration_ms)}</span><button class="remove" aria-label={`${ru.remove}: ${item.track.title}`} onclick={() => run(() => api.remove_item(item.id))}>{ru.remove}</button>
      </li>
    {/each}
  </ol>
  {#if !snapshot.queue.length}<div class="empty"><p>{ru.emptyQueue}</p><p class="empty-hint">{ru.newRequestsHere}</p></div>{/if}
</section>
<style>
  .page-heading, .queue-heading { display:flex; align-items:center; justify-content:space-between; gap:var(--s4); }
  h2, h3 { margin:0; } .page-heading { margin-bottom:var(--s6); }
  .meta, .times { font:var(--type-meta)/var(--leading-meta) var(--font-mono); font-variant-numeric:tabular-nums; color:var(--t3); }
  .deck { display:grid; grid-template-columns:168px minmax(0,1fr); gap:var(--s6); margin:0 0 var(--s8); }
  .cover { width:168px; height:168px; border-radius:var(--r-row); background:var(--raised); color:var(--t3); display:grid; place-items:center; }
  .track { min-width:0; } .track h3 { font-size:28px; line-height:1.1; margin-block:var(--s3) var(--s2); overflow-wrap:anywhere; }
  .play-state { display:flex; align-items:center; gap:var(--s2); color:var(--t3); font-size:var(--type-meta); } .play-state.live { color:var(--accent); }
  .artist { font-size:15px; color:var(--t2); } .requester { font-size:var(--type-secondary); color:var(--t3); margin-top:var(--s2); }
  .timeline { position:relative; height:var(--s5); margin-top:var(--s5); } progress { position:absolute; width:100%; height:100%; opacity:0; }
  .ticks { display:block; height:100%; background:linear-gradient(to right,var(--accent) var(--played),rgb(from var(--t1) r g b / 14%) var(--played)); mask:repeating-linear-gradient(to right,black 0 1px,transparent 1px 6px); }
  .timeline::after { content:''; position:absolute; left:clamp(0px,var(--played),calc(100% - 2px)); top:0; bottom:0; width:2px; background:var(--t1); }
  .times { display:flex; justify-content:space-between; margin-top:var(--s2); } .actions { margin-block:var(--s5); }
  .volume { display:flex; flex-direction:row; align-items:center; gap:var(--s3); margin:0; max-width:360px; color:var(--t2); }
  .volume .meta { min-width:var(--s10); text-align:right; }
  input[type=range] { appearance:none; padding:0; border:0; border-radius:0; height:var(--s5); flex:1; background:transparent; }
  input[type=range]::-webkit-slider-runnable-track { height:var(--s1); background:linear-gradient(to right,var(--t2) var(--volume),var(--track) var(--volume)); border-radius:var(--r-control); }
  input[type=range]::-moz-range-track { height:var(--s1); background:var(--track); } input[type=range]::-moz-range-progress { height:var(--s1); background:var(--t2); }
  input[type=range]::-webkit-slider-thumb { appearance:none; height:var(--s3); width:var(--s3); border-radius:var(--r-round); background:var(--t1); margin-top:calc(-1 * var(--s1)); }
  input[type=range]::-moz-range-thumb { height:var(--s3); width:var(--s3); border:0; border-radius:var(--r-round); background:var(--t1); }
  ol { margin-block:var(--s4) 0; } li { display:grid; grid-template-columns:var(--s6) minmax(0,1fr) minmax(80px,auto) var(--s12) auto; min-height:44px; padding:var(--s1) var(--s3); gap:var(--s3); transition:background-color var(--fast) var(--ease); }
  li:hover, li:focus-within { background:var(--raised); } .queue-track { min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; } .viewer { color:var(--t2); overflow-wrap:anywhere; }
  .remove { opacity:0; background:transparent; border-color:transparent; color:var(--t3); padding-inline:var(--s1); height:var(--s8); transition:opacity var(--fast) var(--ease),background-color var(--fast) var(--ease); } li:hover .remove, li:focus-within .remove { opacity:1; }
  .empty-hint { color:var(--t3); font-size:var(--type-secondary); }
  .empty { padding:var(--s6) 0; border-top:1px solid var(--line); color:var(--t2); }
  @media (hover:none) { .remove { opacity:1; } }
  @media (max-width:700px) { .deck { grid-template-columns:1fr; } .queue-heading { align-items:flex-start; flex-direction:column; gap:var(--s2); } li { grid-template-columns:var(--s6) minmax(0,1fr) var(--s12) auto; } .viewer { grid-column:2; } .remove { grid-column:4; grid-row:1 / span 2; } }
</style>
