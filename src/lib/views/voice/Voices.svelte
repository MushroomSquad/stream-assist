<script lang="ts">
import { onMount } from 'svelte';
import { open, save } from '@tauri-apps/plugin-dialog';
import * as api from '$lib/voiceApi';
import { ru, errorText } from '$lib/i18n/ru';
let profiles = $state<api.Profile[]>([]), message = $state(''), url = $state(''), busy = $state(false), disposed = false;
let active = $state<number | null>(null), listenError = $state('');
let projectBusy = $state(false), reportMessage = $state('');
async function project(f: () => Promise<void>) {
  if (projectBusy) return;
  projectBusy = true;
  try { await run(f); } finally { projectBusy = false; }
}
async function run(f: () => Promise<void>) { message = ''; reportMessage = ''; try { await f(); } catch (e) { message = errorText(e); } }
async function load() { profiles = await api.vl_profiles(); }
onMount(() => { void run(load); return () => { disposed = true; if (url) URL.revokeObjectURL(url); }; });
async function exportProject() {
  const path = await save({ defaultPath: 'voices.svlab', filters: [{ name: ru.vlProject, extensions: ['svlab'] }] });
  if (path) await api.vl_export_project(path);
}
async function importProject() {
  const path = await open({ multiple: false, filters: [{ name: ru.vlProject, extensions: ['svlab'] }] });
  if (typeof path !== 'string') return;
  const report = await api.vl_import_project(path);
  await load();
  reportMessage = `${ru.vlImportedProfiles}: ${report.profiles} · ${ru.vlImportedRenders}: ${report.renders} · ${ru.vlSkipped}: ${report.skipped}${report.revisionMismatch ? `. ${ru.vlRevisionMismatch}` : ''}`;
}
async function importJson() { const path = await open({ multiple: false, filters: [{ name: ru.vlImportJson, extensions: ['json'] }] }); if (typeof path === 'string') { await api.vl_import_profile(path, path.split(/[\\/]/).at(-1)?.replace(/\.json$/i, '') ?? ru.voice); await load(); } }
async function listen(id: number) {
  active = id; listenError = ''; busy = true; try { const r = await api.vl_render(id, { text: ru.vlSamplePhrase, lang: 'ru', speed: 1, quality: 8 }); const buf = await api.vl_render_audio(r.id); if (!disposed) { if (url) URL.revokeObjectURL(url); url = api.blobUrl(buf); } } catch (e) { listenError = errorText(e); } finally { busy = false; }
}
async function exportJson(p: api.Profile, recipe = false) { const path = await save({ defaultPath: recipe ? 'recipe.json' : 'voice.json', filters: [{ name: recipe ? ru.vlExportRecipe : ru.vlExportJson, extensions: ['json'] }] }); if (path) await (recipe ? api.vl_export_recipe(p.id, path) : api.vl_export_profile(p.id, path)); }
</script>
<div class="actions"><button disabled={projectBusy} onclick={() => project(exportProject)}>{ru.vlExportProject}</button><button disabled={projectBusy} onclick={() => project(importProject)}>{ru.vlImportProject}</button></div>
<button onclick={() => run(importJson)}>{ru.vlImportJson}</button>
{#if message}<p class="notice" role="alert">{message}</p>{/if}
{#if reportMessage}<p role="status">{reportMessage}</p>{/if}
{#if !profiles.length}<p class="muted">{ru.vlEmpty}</p>{/if}
{#each profiles as p (p.id)}<div class="card"><p class="muted">{ru.vlOrigin[p.origin]}</p>
<form onsubmit={e => { e.preventDefault(); void run(() => api.vl_rename_profile(p.id, p.name, p.tags)); }}>
<label>{ru.vlName}<input required bind:value={p.name} /></label>
<label>{ru.vlTags}<input value={p.tags.join(', ')} onchange={e => p.tags = e.currentTarget.value.split(',').map(t => t.trim()).filter(Boolean)} /></label>
<button>{ru.save}</button></form>
<div class="actions"><button disabled={busy} onclick={() => void listen(p.id)}>{ru.vlListen}</button><button onclick={() => run(() => exportJson(p))}>{ru.vlExportJson}</button>
{#if p.origin === 'mix'}<button onclick={() => run(() => exportJson(p, true))}>{ru.vlExportRecipe}</button>{/if}
{#if p.origin !== 'builtin'}<button onclick={() => { if (confirm(ru.vlDeleteConfirm)) void run(async () => { await api.vl_delete_profile(p.id); await load(); }); }}>{ru.remove}</button>{/if}</div>
{#if active === p.id}{#if busy}<p class="muted" role="status">{ru.vlListening}</p>{:else if listenError}<p class="notice" role="alert">{listenError}</p>{:else if url}<audio controls autoplay src={url}></audio>{/if}{/if}</div>{/each}
