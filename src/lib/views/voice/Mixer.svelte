<script lang="ts">
import { onMount } from 'svelte';
import * as api from '$lib/voiceApi';
import { createHistory } from '$lib/mixerHistory';
import { ru, errorText } from '$lib/i18n/ru';
type Input = { id: number; weight: number };
let profiles = $state<api.Profile[]>([]), inputs = $state<Input[]>([]), addId = $state(0);
let dp = $state<api.DpStrategy>('dominant'), selectId = $state<number | null>(null);
let params = $state<api.RenderParams>({ text: ru.vlSamplePhrase, lang: 'ru', speed: 1, quality: 8 });
let name = $state(''), message = $state(''), busy = $state(false), url = $state(''), disposed = false;
const history = createHistory<Input[]>([]);
let canUndo = $state(false), canRedo = $state(false);
const sum = $derived(inputs.reduce((a, i) => a + i.weight, 0));
const valid = $derived(inputs.length > 0 && inputs.length <= 8 && inputs.every(i => Number.isFinite(i.weight) && i.weight >= 0 && i.weight <= 100) && sum > 0 && (dp !== 'select' || inputs.some(i => i.id === selectId)));
function sync() { if (inputs.some(i => i.id === addId) || !addId) addId = profiles.find(p => !inputs.some(i => i.id === p.id))?.id ?? 0; canUndo = history.canUndo; canRedo = history.canRedo; if (!inputs.some(i => i.id === selectId)) selectId = inputs[0]?.id ?? null; }
function push() { history.push($state.snapshot(inputs)); sync(); }
function undo() { inputs = history.undo(); sync(); }
function redo() { inputs = history.redo(); sync(); }
function key(e: KeyboardEvent) { if (!(e.ctrlKey || e.metaKey) || e.key.toLowerCase() !== 'z') return; const target = e.target; if (target instanceof HTMLElement && (target.isContentEditable || ['TEXTAREA','SELECT'].includes(target.tagName) || (target instanceof HTMLInputElement && !target.hasAttribute('data-weight')))) return; e.preventDefault(); if (e.shiftKey) redo(); else undo(); }
async function run(f: () => Promise<void>) { message = ''; try { await f(); } catch (e) { message = errorText(e); } }
onMount(() => { void run(async () => { profiles = await api.vl_profiles(); addId = profiles[0]?.id ?? 0; }); return () => { disposed = true; if (url) URL.revokeObjectURL(url); }; });
function tuples(): [number, number][] { return inputs.map(i => [i.id, i.weight]); }
async function generate() { busy = true; try { const r = await api.vl_mix_preview(tuples(), dp, dp === 'select' ? selectId : null, $state.snapshot(params)); const buf = await api.vl_render_audio(r.id); if (!disposed) { if (url) URL.revokeObjectURL(url); url = api.blobUrl(buf); } } finally { busy = false; } }
</script>
<svelte:window onkeydown={key} />
{#if message}<p class="notice" role="alert">{message}</p>{/if}
<div class="card"><label>{ru.vlAdd}<select bind:value={addId}>{#each profiles.filter(p => !inputs.some(i => i.id === p.id)) as p}<option value={p.id}>{p.name}</option>{/each}</select></label>
<button disabled={inputs.length >= 8 || !profiles.some(p => p.id === addId) || inputs.some(i => i.id === addId)} onclick={() => { inputs = [...inputs, { id: addId, weight: 50 }]; push(); addId = profiles.find(p => !inputs.some(i => i.id === p.id))?.id ?? 0; }}>{ru.vlAdd}</button>
{#each inputs as input (input.id)}
<div class="card"><p>{profiles.find(p => p.id === input.id)?.name}</p>
<label>{ru.vlWeight}<input data-weight type="range" min="0" max="100" step="1" value={input.weight} oninput={e => { input.weight = e.currentTarget.valueAsNumber; push(); }} /></label>
<label>{ru.vlWeight}<input data-weight type="number" min="0" max="100" step="1" value={input.weight} oninput={e => { input.weight = e.currentTarget.valueAsNumber; push(); }} /></label>
<p class="muted">{ru.vlShare}: {sum > 0 ? (100 * input.weight / sum).toFixed(1) : '0'}%</p>
<button onclick={() => { inputs = inputs.filter(i => i.id !== input.id); push(); addId = input.id; }}>{ru.remove}</button></div>{/each}
<div class="actions"><button disabled={!canUndo} onclick={undo}>{ru.vlUndo}</button><button disabled={!canRedo} onclick={redo}>{ru.vlRedo}</button></div>
<label>{ru.vlDp}<select bind:value={dp}><option value="dominant">{ru.vlDominant}</option><option value="weighted">{ru.vlWeighted}</option><option value="select">{ru.vlSelect}</option></select></label>
{#if dp === 'select'}<label>{ru.vlSelect}<select bind:value={selectId}>{#each inputs as input}<option value={input.id}>{profiles.find(p => p.id === input.id)?.name}</option>{/each}</select></label>{/if}
<form onsubmit={e => { e.preventDefault(); void run(generate); }}>
<label>{ru.vlText}<textarea required bind:value={params.text}></textarea></label>
<label>{ru.vlLanguage}<input required bind:value={params.lang} /></label>
<label>{ru.vlSpeed}<input required type="number" min="0.7" max="1.5" step="0.1" bind:value={params.speed} /></label>
<label>{ru.vlQuality}<input required type="number" min="5" max="12" step="1" bind:value={params.quality} /></label>
<button class="primary" disabled={!valid || busy || !params.text.trim() || Array.from(params.text.trim()).length > 1000}>{ru.vlGenerate}</button></form>
{#if url}<audio controls src={url}></audio>{/if}
<form onsubmit={e => { e.preventDefault(); void run(async () => { await api.vl_save_snapshot(tuples(), dp, dp === 'select' ? selectId : null, name); profiles = await api.vl_profiles(); }); }}>
<label>{ru.vlName}<input required bind:value={name} /></label><button disabled={!valid}>{ru.vlSnapshot}</button></form></div>
