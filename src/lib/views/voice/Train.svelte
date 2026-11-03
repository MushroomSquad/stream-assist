<script lang="ts">
import { onMount } from 'svelte';
import * as api from '$lib/voiceApi';
import { ru, errorText } from '$lib/i18n/ru';
let status = $state<api.TrainStatus | null>(null), sources = $state<api.Source[]>([]), runs = $state<api.TrainRun[]>([]);
let sourceId = $state(0), startMs = $state(0), endMs = $state(10000), lang = $state('ru'), preset = $state<api.TrainPreset>('fast'), seed = $state(42);
let message = $state(''), busy = $state(false), installing = $state(false), listening = $state(false);
let progress = $state<api.TrainInstallProgress | null>(null);
let timing = $state<{ id: number; step: number; time: number }[]>([]);
const selected = $derived(sources.find(s => s.id === sourceId));
const active = $derived(runs.find(r => r.status === 'running'));
const valid = $derived(!!selected?.consent && Number.isInteger(startMs) && Number.isInteger(endMs) && startMs >= 0 && endMs <= (selected?.durationMs ?? 0) && endMs - startMs >= 3000 && endMs - startMs <= 30000 && Number.isSafeInteger(seed) && seed >= 0);
const eta = $derived.by(() => {
  const points = timing.filter(p => p.id === active?.id), first = points[0], last = points.at(-1);
  if (!active || !first || !last || last.step <= first.step) return null;
  return Math.max(0, active.total - last.step) * (last.time - first.time) / (last.step - first.step) / 1000;
});
function update(run: api.TrainRun) {
  runs = [run, ...runs.filter(r => r.id !== run.id)].sort((a, b) => b.id - a.id);
  const last = timing.at(-1);
  if (run.status === 'running' && (!last || last.id !== run.id || run.step > last.step)) {
    if (last?.id !== run.id || run.step < (last?.step ?? 0)) timing = [];
    timing = [...timing, { id: run.id, step: run.step, time: Date.now() }].slice(-12);
  }
}
async function action(f: () => Promise<unknown>) { message = ''; busy = true; try { await f(); } catch (e) { message = errorText(e); } finally { busy = false; } }
async function load(refresh = false) { status = await api.vl_train_status(refresh); sources = await api.vl_sources(); runs = await api.vl_train_runs(); }
async function install() {
  installing = true; progress = null;
  try { await api.vl_train_install(); } finally { installing = false; status = await api.vl_train_status(true); }
}
function chart(run: api.TrainRun, index: 1 | 2) {
  const values = run.history.flatMap(p => [p[1], p[2]]), low = Math.min(...values), high = Math.max(...values);
  return run.history.map(p => `${10 + p[0] / Math.max(1, run.total) * 580},${150 - (p[index] - low) / Math.max(0.000001, high - low) * 140}`).join(' ');
}
function number(value: number | null | undefined) { return value == null ? '—' : value.toPrecision(5); }
onMount(() => {
  let disposed = false;
  const stops: (() => void)[] = [];
  void action(async () => {
    const stopRun = await api.onTrain(update);
    if (disposed) { stopRun(); return; } stops.push(stopRun);
    const stopInstall = await api.onTrainInstall(p => { progress = p; });
    if (disposed) { stopInstall(); return; } stops.push(stopInstall);
    listening = true;
    await load();
  });
  return () => { disposed = true; stops.forEach(stop => stop()); };
});
</script>
<p class="notice">{ru.vlTrainWarning}</p>
{#if message}<p class="notice" role="alert">{message}</p>{/if}
<button disabled={busy || installing} onclick={() => action(() => load(true))}>{ru.status}</button>
{#if status}
<div class="card">
<h3>{ru.vlTrainPack}</h3>
<dl>
<dt>{ru.vlTrainUv}</dt><dd>{status.uv ?? ru.vlUnknown}{#if !status.uvOk} · {ru.vlTrainUvOld}{/if}</dd>
<dt>{ru.vlTrainPack}</dt><dd>{ru.vlTrainPackState[status.pack]}</dd>
<dt>{ru.vlTrainGpu}</dt><dd>{status.gpu?.name ?? ru.vlUnknown}</dd>
<dt>{ru.vlTrainDriver}</dt><dd>{status.gpu?.driver ?? ru.vlUnknown}</dd>
<dt>{ru.vlTrainVram}</dt><dd>{status.gpu ? `${status.gpu.vramFreeMb} / ${status.gpu.vramTotalMb}` : ru.vlUnknown}</dd>
<dt>{ru.vlTrainCuda}</dt><dd>{status.torchCuda ?? ru.vlUnknown}</dd>
<dt>{ru.vlTrainDisk}</dt><dd>{status.diskFreeMb ?? ru.vlUnknown}</dd>
</dl>
{#if status.gpu && status.gpu.vramFreeMb < 6144}<p class="vram-warning" role="alert">{ru.vlTrainLowVram}</p>{/if}
{#if installing}
<p>{progress ? ru.vlTrainStages[progress.stage as keyof typeof ru.vlTrainStages] : ru.vlTrainInstall}</p>
{#if progress}<progress value={progress.done} max={progress.total || 1}></progress>{/if}
<button onclick={() => action(api.vl_train_install_cancel)}>{ru.vlCancel}</button>
{:else}
{#if status.pack !== 'ok'}<button disabled={busy || !!active || !listening} onclick={() => action(install)}>{status.pack === 'outdated' ? ru.vlTrainUpdate : ru.vlTrainInstall}</button>{/if}
<button disabled={busy || !!active} onclick={() => action(async () => { await api.vl_train_uninstall(); await load(true); })}>{ru.vlTrainUninstall}</button>
{/if}
</div>
{/if}
<form class="card" onsubmit={e => { e.preventDefault(); void action(async () => { await api.vl_train_start(sourceId, startMs, endMs, lang, preset, seed); runs = await api.vl_train_runs(); }); }}>
<p>{ru.vlTrainRegion}</p>
<label>{ru.vlSources}<select bind:value={sourceId} onchange={() => { startMs = 0; endMs = Math.min(10000, sources.find(s => s.id === sourceId)?.durationMs ?? 10000); }}><option value={0}>—</option>{#each sources as source}<option value={source.id}>{source.name}</option>{/each}</select></label>
{#if selected && !selected.consent}<p class="notice">{ru.vlConsent}</p>{/if}
<label>{ru.vlStart}<input required type="number" min="0" step="1" bind:value={startMs} /></label>
<label>{ru.vlEnd}<input required type="number" min="0" max={selected?.durationMs} step="1" bind:value={endMs} /></label>
<label>{ru.vlLanguage}<input required bind:value={lang} /></label>
<label>{ru.vlPreset}<select bind:value={preset}><option value="fast">{ru.vlTrainFast}</option><option value="standard">{ru.vlTrainStandard}</option><option value="long">{ru.vlTrainLong}</option></select></label>
<label>{ru.vlSeed}<input required type="number" min="0" max={Number.MAX_SAFE_INTEGER} step="1" bind:value={seed} /></label>
<button class="primary" disabled={!valid || busy || installing || !!active || status?.pack !== 'ok' || !listening}>{ru.vlTrainStart}</button>
</form>
<h3>{ru.vlTrainRuns}</h3>
{#if runs.length === 0}<p>{ru.vlEmpty}</p>{/if}
{#each runs as run (run.id)}
<div class="card">
<h4>#{run.id} · {sources.find(s => s.id === run.sourceId)?.name ?? ru.vlSources} · {ru.vlTrainStatus[run.status as keyof typeof ru.vlTrainStatus]}</h4>
<p>{ru.vlTrainStep}: {run.step}/{run.total} · {ru.vlLoss}: {number(run.history.at(-1)?.[1])} · {ru.vlTrainBest}: {number(run.bestLoss)} · {ru.vlTrainLr}: {number(run.lr)}</p>
{#if run.error}<p class="notice">{errorText(run.error)}</p>{/if}
{#if run.history.length}
<svg viewBox="0 0 600 160" role="img" aria-label={`${ru.vlLoss} / ${ru.vlTrainBest}`}><polyline points={chart(run, 1)} fill="none" stroke="currentColor" stroke-width="2" /><polyline points={chart(run, 2)} fill="none" stroke="#47b58b" stroke-width="2" /></svg>
<p>{ru.vlLoss} · <span class="best">{ru.vlTrainBest}</span></p>
{/if}
{#if run.status === 'running'}
<progress value={run.step} max={run.total}></progress>
<p>{ru.vlEta}: {eta === null ? ru.vlEtaPending : `≈ ${Math.ceil(eta / 60)} ${ru.vlMinutes}`}</p>
<button disabled={busy} onclick={() => action(() => api.vl_train_stop(run.id))}>{ru.vlTrainStop}</button>
<button disabled={busy} onclick={() => { if (confirm(ru.vlTrainKillConfirm)) void action(() => api.vl_train_kill(run.id)); }}>{ru.vlTrainKill}</button>
{:else}
{#if ['stopped', 'interrupted', 'failed'].includes(run.status)}<button disabled={busy || installing || !!active || status?.pack !== 'ok' || !listening} onclick={() => action(async () => { timing = []; await api.vl_train_resume(run.id); runs = await api.vl_train_runs(); })}>{ru.vlTrainResume}</button>{/if}
<button disabled={busy} onclick={() => { if (confirm(ru.vlTrainDeleteConfirm)) void action(async () => { await api.vl_train_delete(run.id); runs = runs.filter(r => r.id !== run.id); }); }}>{ru.remove}</button>
{/if}
</div>
{/each}
<style>
dl{display:grid;grid-template-columns:minmax(120px,1fr) 2fr;gap:8px}dd{margin:0;overflow-wrap:anywhere}svg{width:100%;max-height:220px}.vram-warning{color:#f6c453;background:#493b1a;padding:12px}.best{color:#47b58b}
</style>
