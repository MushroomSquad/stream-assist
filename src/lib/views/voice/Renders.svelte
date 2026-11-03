<script lang="ts">
import { onMount } from 'svelte';
import { save } from '@tauri-apps/plugin-dialog';
import * as api from '$lib/voiceApi';
import { ru, errorText } from '$lib/i18n/ru';
let renders = $state<api.Render[]>([]), profiles = $state<api.Profile[]>([]), selected = $state<number[]>([]);
let player = $state<{ id: number; url: string } | null>(null);
let urls = $state<Record<number, string>>({}), message = $state(''), disposed = false;
async function run(f: () => Promise<void>) { message = ''; try { await f(); } catch (e) { message = errorText(e); } }
async function load() { [renders, profiles] = await Promise.all([api.vl_renders(), api.vl_profiles()]); selected = selected.filter(id => renders.some(r => r.id === id)); if (player && !renders.some(r => r.id === player!.id)) { URL.revokeObjectURL(player.url); player = null; } for (const id of Object.keys(urls).map(Number)) if (!renders.some(r => r.id === id)) { URL.revokeObjectURL(urls[id]); delete urls[id]; } }
onMount(() => { void run(load); return () => { disposed = true; if (player) URL.revokeObjectURL(player.url); Object.values(urls).forEach(URL.revokeObjectURL); }; });
$effect(() => {
  for (const id of Object.keys(urls).map(Number)) if (selected.length !== 2 || !selected.includes(id)) { URL.revokeObjectURL(urls[id]); delete urls[id]; }
});
async function audio(id: number, compare = false) {
  const buf = await api.vl_render_audio(id);
  if (disposed || !renders.some(r => r.id === id)) return;
  if (compare) {
    if (selected.length !== 2 || !selected.includes(id)) return;
    if (urls[id]) URL.revokeObjectURL(urls[id]); urls[id] = api.blobUrl(buf);
  } else {
    if (player) URL.revokeObjectURL(player.url); player = { id, url: api.blobUrl(buf) };
  }
}
async function favorite(r: api.Render, fav: boolean) { await api.vl_set_favorite(r.id, fav); r.favorite = fav; }
async function exportWav(id: number) { const path = await save({ defaultPath: 'render.wav', filters: [{ name: ru.vlExportWav, extensions: ['wav'] }] }); if (path) await api.vl_export_render(id, path); }
</script>
<div class="voice-screen">
<button onclick={() => run(load)}>{ru.reload}</button>
{#if message}<p class="notice" role="alert">{message}</p>{/if}
{#if selected.length === 2}<h3>{ru.vlCompare}</h3><div class="comparison">{#each selected as id, i}{@const r = renders.find(r => r.id === id)!}
<div class="card"><p>{i === 0 ? 'A' : 'B'}: {r.text}</p><button onclick={() => run(() => audio(id, true))}>{ru.vlListen}</button>
{#if urls[id]}<audio controls src={urls[id]}></audio>{/if}<button onclick={() => run(() => favorite(r, true))}>{i === 0 ? ru.vlABetter : ru.vlBBetter}</button></div>{/each}</div>{/if}
{#if !renders.length}<p class="muted">{ru.vlEmpty}</p>{/if}
{#each renders as r (r.id)}<div class="card"><p>{r.text}</p><p class="muted metrics">{profiles.find(p => p.id === r.profileId)?.name ?? ru.vlMix} · {(r.durationMs / 1000).toFixed(1)} с · {ru.vlRtf} {r.durationMs > 0 ? (r.wallMs / r.durationMs).toFixed(2) : '—'}</p>
<div class="actions"><label class="check compare"><input type="checkbox" checked={selected.includes(r.id)} disabled={selected.length === 2 && !selected.includes(r.id)} onchange={e => selected = e.currentTarget.checked ? [...selected, r.id] : selected.filter(id => id !== r.id)} />{ru.vlCompare}</label>
<button onclick={() => run(() => audio(r.id))}>{ru.vlListen}</button><button aria-pressed={r.favorite} onclick={() => run(() => favorite(r, !r.favorite))}>{r.favorite ? ru.vlFavoriteAdded : ru.vlFavoriteAdd}</button><button onclick={() => run(() => exportWav(r.id))}>{ru.vlExportWav}</button></div>
{#if player?.id === r.id}<audio controls src={player.url}></audio>{/if}</div>{/each}

</div>
<style>
.voice-screen .card{margin-block:var(--s3)}
.actions{align-items:center;margin-block:var(--s3) 0;gap:var(--s2)}
.actions .compare{order:1;margin:0 0 0 auto}
.metrics{font-family:var(--font-mono);color:var(--t3);font-size:var(--type-meta)}
.comparison{display:flex;gap:var(--s4);flex-wrap:wrap}.comparison>.card{flex:1;min-width:240px}audio{max-width:100%;display:block;margin:var(--s4) 0}
button[aria-pressed=true] { color:var(--accent); border-color:var(--line-strong); }</style>
