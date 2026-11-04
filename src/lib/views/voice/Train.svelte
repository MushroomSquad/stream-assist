<script lang="ts">
import { onMount } from 'svelte';
import * as api from '$lib/voiceApi';
import { ru, errorText } from '$lib/i18n/ru';
let status = $state<api.TrainStatus | null>(null), sources = $state<api.Source[]>([]), runs = $state<api.TrainRun[]>([]);
let sourceId = $state(0), startMs = $state(0), endMs = $state(10000), lang = $state('ru'), preset = $state<api.TrainPreset>('fast'), seed = $state(42);
let message = $state(''), busy = $state(false), installing = $state(false), listening = $state(false);
let installLog = $state('');
let progress = $state<api.TrainInstallProgress | null>(null);
let stopping = $state<Set<number>>(new Set());
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
  if (run.status !== 'running') stopping = new Set([...stopping].filter(id => id !== run.id));
  runs = [run, ...runs.filter(r => r.id !== run.id)].sort((a, b) => b.id - a.id);
  const last = timing.at(-1);
  if (run.status === 'running' && (!last || last.id !== run.id || run.step > last.step)) {
    if (last?.id !== run.id || run.step < (last?.step ?? 0)) timing = [];
    timing = [...timing, { id: run.id, step: run.step, time: Date.now() }].slice(-12);
  }
}
async function action(f: () => Promise<unknown>) { message = ''; busy = true; try { await f(); } catch (e) { message = errorText(e); } finally { busy = false; } }
async function load(refresh = false) {
  const results = await Promise.allSettled([
    api.vl_train_status(refresh).then(s => { status = s; installing = s.installing; }),
    api.vl_sources().then(s => { sources = s; }),
    api.vl_train_runs().then(r => { runs = r; })
  ]);
  const failed = results.find(r => r.status === 'rejected');
  if (failed?.status === 'rejected') throw failed.reason;
}
async function stop(runId: number) {
  stopping = new Set(stopping).add(runId);
  try { await api.vl_train_stop(runId); }
  catch (e) { stopping = new Set([...stopping].filter(id => id !== runId)); throw e; }
}
async function install() {
  installing = true; progress = null; installLog = '';
  try { await api.vl_train_install(); }
  catch (e) {
    if (String(e) === 'TRAINER_INSTALL_FAILED') installLog = await api.vl_train_install_log().then(log => log.trimEnd().split('\n').slice(-20).join('\n')).catch(() => '');
    throw e;
  } finally { installing = false; status = await api.vl_train_status(true); }
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
<div class="voice-screen">
<p class="notice">{ru.vlTrainWarning}</p>
{#if message}<p class="notice" role="alert">{message}</p>{/if}
<button disabled={busy || installing} onclick={() => action(() => load(true))}>{ru.status}</button>
{#if status}
<div class="card">
<h3>{ru.vlTrainPack}</h3>
{#if installing || status?.pack !== 'ok'}<p>{ru.vlTrainInstallNote}</p>{/if}
{#if installLog}<pre>{installLog}</pre>{/if}
<dl>
<dt>{ru.vlTrainUv}</dt><dd>{status.uv ?? ru.vlUnknown}{#if !status.uvOk}{' · '}{ru.vlTrainUvOld}{/if}</dd>
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
 {#if progress?.stage === 'packages'}
<p>{ru.vlTrainMegabytes} {(progress.done / 1048576).toFixed(1)} / {(progress.total / 1048576).toFixed(1)}{#if progress.item}{' · '}{progress.item}{/if}</p>
{/if}
{#if !progress || (progress.stage === 'venv' && progress.total === 0)}
<progress aria-label={ru.vlTrainInstall}></progress>
{:else}
<progress aria-label={ru.vlTrainInstall} value={progress.done} max={progress.total || 1}></progress>
{/if}
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
<label>{ru.vlLanguage}<select bind:value={lang}>{#each status?.languages ?? [lang] as language}<option value={language}>{language}</option>{/each}</select></label>
<label>{ru.vlPreset}<select bind:value={preset}><option value="fast">{ru.vlTrainFast}</option><option value="standard">{ru.vlTrainStandard}</option><option value="long">{ru.vlTrainLong}</option></select></label>
<label>{ru.vlSeed}<input required type="number" min="0" max={Number.MAX_SAFE_INTEGER} step="1" bind:value={seed} /></label>
<button class="primary" disabled={!valid || busy || installing || !!active || status?.pack !== 'ok' || !listening}>{ru.vlTrainStart}</button>
</form>
<h3>{ru.vlTrainRuns}</h3>
{#if runs.length === 0}<p>{ru.vlEmpty}</p>{/if}
{#each runs as run (run.id)}
<div class="card">
<h4 class="job-status"><span class="tally" class:live={run.status === 'running'} class:connected={run.status === 'completed'} class:failed={run.status === 'failed'} aria-hidden="true"></span>#{run.id} · {sources.find(s => s.id === run.sourceId)?.name ?? '—'} · {ru.vlTrainStatus[run.status as keyof typeof ru.vlTrainStatus]}</h4>
<p class="metrics">{ru.vlTrainStep}: {run.step}/{run.total} · {ru.vlLoss}: {number(run.history.at(-1)?.[1])} · {ru.vlTrainBest}: {number(run.bestLoss)} · {ru.vlTrainLr}: {number(run.lr)}</p>
{#if run.error}<p class="notice">{errorText(run.error)}</p>{/if}
{#if run.history.length}
<svg viewBox="0 0 600 160" role="img" aria-label={`${ru.vlLoss} / ${ru.vlTrainBest}`}><polyline points={chart(run, 1)} fill="none" stroke="var(--t2)" stroke-width="2" /><polyline points={chart(run, 2)} fill="none" stroke="var(--accent)" stroke-width="2" /></svg>
<p>{ru.vlLoss} · <span class="best">{ru.vlTrainBest}</span></p>
{/if}
{#if run.status === 'running'}
<progress aria-label={ru.vlTrainStep} value={run.step} max={run.total}></progress>
<p>{ru.vlEta}: {eta === null ? ru.vlEtaPending : `≈ ${Math.ceil(eta / 60)} ${ru.vlMinutes}`}</p>
<button disabled={busy || stopping.has(run.id)} onclick={() => action(() => stop(run.id))}>{stopping.has(run.id) ? ru.vlTrainStopping : ru.vlTrainStop}</button>
<button disabled={busy} onclick={() => { if (confirm(ru.vlTrainKillConfirm)) void action(() => api.vl_train_kill(run.id)); }}>{ru.vlTrainKill}</button>
{:else}
{#if ['stopped', 'interrupted', 'failed'].includes(run.status)}<button disabled={busy || installing || !!active || status?.pack !== 'ok' || !listening} onclick={() => action(async () => { timing = []; await api.vl_train_resume(run.id); runs = await api.vl_train_runs(); })}>{ru.vlTrainResume}</button>{/if}
<button disabled={busy} onclick={() => { if (confirm(ru.vlTrainDeleteConfirm)) void action(async () => { await api.vl_train_delete(run.id); runs = runs.filter(r => r.id !== run.id); }); }}>{ru.remove}</button>
{/if}
</div>
{/each}

</div>
<style>
pre { white-space:pre-wrap; overflow-wrap:anywhere; }
dl { display:grid; grid-template-columns:minmax(0,1fr) minmax(0,2fr); gap:var(--s2) var(--s4); margin-block:var(--s4); }
dt { color:var(--t2); } dd { margin:0; overflow-wrap:anywhere; font-family:var(--font-mono); font-size:var(--type-meta); font-variant-numeric:tabular-nums; }
svg { width:100%; max-height:220px; background:var(--card); margin-top:var(--s4); }
.vram-warning { color:var(--accent); background:var(--warn-bg); padding:var(--s3); border:1px solid var(--line); border-radius:var(--r-control); }
.best { color:var(--accent); }
</style>
