<script lang="ts">
import { onMount } from 'svelte';
import { open } from '@tauri-apps/plugin-dialog';
import * as api from '$lib/voiceApi';
import { ru, errorText } from '$lib/i18n/ru';
let sources = $state<api.Source[]>([]), selected = $state<api.Source | null>(null);
let consent = $state(false), startMs = $state(0), endMs = $state(10000);
let transcript = $state(''), lang = $state('ru'), name = $state(''), preset = $state<api.ClonePreset>('fast'), seed = $state(42);
let peaks = $state<number[]>([]), canvas = $state<HTMLCanvasElement>(), url = $state('');
let job = $state<api.JobState | null>(null), message = $state(''), busy = $state(false), listening = $state(false);
let anchor = 0, selectionVersion = 0;
const stages = ['select-preset', 'mix-presets', 'mix-rows', 'refine'];
let startedAt = $state(0), now = $state(0), stageTotals = $state<Record<string, number>>({});
const elapsed = $derived(Math.max(0, now - startedAt));
const iterations = $derived.by(() => {
  const p = job?.progress;
  if (p?.event !== 'progress' || p.stage === undefined || p.iteration === undefined || p.total === undefined) return { done: 0, total: 0 };
  const index = stages.indexOf(p.stage);
  if (index < 0) return { done: 0, total: 0 };
  // Until a stage reports its total, use the current stage as a rough estimate.
  const totals = stages.map(stage => stageTotals[stage] ?? p.total);
  return { done: totals.slice(0, index).reduce((a, b) => a + b, 0) + p.iteration,
    total: totals.reduce((a, b) => a + b, 0) };
});
const eta = $derived(iterations.done > 0 ? elapsed / iterations.done * Math.max(0, iterations.total - iterations.done) : null);
function duration(ms: number) { const seconds = Math.round(ms / 1000); return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`; }
function updateJob(next: api.JobState) {
  if (next.kind !== 'clone') return;
  if (job?.id !== next.id) { startedAt = Date.now(); stageTotals = {}; }
  now = Date.now();
  const p = next.progress;
  if (p?.event === 'progress' && p.stage !== undefined && p.total !== undefined) stageTotals[p.stage] = p.total;
  job = next;
  if (next.error) message = errorText(next.error);
}
const regionValid = $derived(!!selected && Number.isInteger(startMs) && Number.isInteger(endMs) && startMs >= 0 && endMs <= selected.durationMs && endMs > startMs);
const cloneValid = $derived(regionValid && endMs - startMs >= 1000 && endMs - startMs <= 15000 && !!selected?.consent);
async function run(f: () => Promise<void>) { message = ''; try { await f(); } catch (e) { message = errorText(e); } }
function replaceAudio(next: string) { if (url) URL.revokeObjectURL(url); url = next; }
async function choose(source: api.Source) {
  const version = ++selectionVersion; selected = source; startMs = 0; endMs = Math.min(10000, source.durationMs); name = source.name; peaks = []; replaceAudio('');
  const result = await api.vl_peaks(source.id); if (version === selectionVersion) peaks = result;
}
async function load() { sources = await api.vl_sources(); }
onMount(() => {
  let disposed = false, stop = () => {};
  const timer = setInterval(() => { if (job?.status === 'running') now = Date.now(); }, 1000);
  void run(async () => {
    const fn = await api.onJob(updateJob);
    if (disposed) { fn(); return; } stop = fn; listening = true;
    const jobs = await api.vl_jobs(); const latest = jobs.filter(j => j.kind === 'clone').at(-1); if (!disposed && latest) updateJob(latest);
    await load();
  });
  return () => { disposed = true; clearInterval(timer); selectionVersion++; stop(); replaceAudio(''); };
});
$effect(() => {
  const c = canvas, p = peaks, start = startMs, end = endMs, duration = selected?.durationMs ?? 1;
  const ctx = c?.getContext('2d'); if (!c || !ctx) return;
  ctx.clearRect(0, 0, c.width, c.height);
  ctx.fillStyle = '#246b55'; p.forEach((v, i) => ctx.fillRect(i * c.width / p.length, c.height * (1 - v) / 2, Math.max(1, c.width / p.length), Math.max(1, v * c.height)));
  ctx.fillStyle = '#6495ed55'; ctx.fillRect(start / duration * c.width, 0, (end - start) / duration * c.width, c.height);
});
function position(e: PointerEvent) { const rect = e.currentTarget instanceof HTMLCanvasElement ? e.currentTarget.getBoundingClientRect() : null; return rect ? Math.round(Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width)) * (selected?.durationMs ?? 0)) : 0; }
async function importAudio() { const path = await open({ multiple: false, filters: [{ name: ru.vlAudio, extensions: ['wav','mp3','flac','ogg','m4a'] }] }); if (typeof path === 'string') { const s = await api.vl_import_audio(path, consent); await load(); await choose(s); } }
async function clone() {
  if (!selected || !cloneValid) return;
  busy = true; try { const id = await api.vl_clone(selected.id, startMs, endMs, transcript, lang, name, preset, seed); const jobs = await api.vl_jobs(); const current = jobs.find(j => j.id === id); if (current) updateJob(current); } finally { busy = false; }
}
</script>
<label class="check"><input type="checkbox" bind:checked={consent} />{ru.vlConsent}</label>
<button onclick={() => run(importAudio)}>{ru.vlImportAudio}</button>
{#if message}<p class="notice" role="alert">{message}</p>{/if}
<div class="actions">{#each sources as source}<button onclick={() => run(() => choose(source))}>{source.name}</button>{/each}</div>
{#if selected}<div class="card"><h3>{selected.name}</h3>
<label class="check"><input type="checkbox" checked={selected.consent} onchange={e => { const s = selected!; const checked = e.currentTarget.checked; void run(async () => { await api.vl_set_consent(s.id, checked); s.consent = checked; await load(); }); }} />{ru.vlConsent}</label>
<p class="muted">{ru.vlRegion}</p>
<canvas bind:this={canvas} width="800" height="120" aria-label={ru.vlRegion} onpointerdown={e => { anchor = position(e); startMs = anchor; endMs = anchor; e.currentTarget.setPointerCapture(e.pointerId); }} onpointermove={e => { if (e.currentTarget.hasPointerCapture(e.pointerId)) { const end = position(e); startMs = Math.min(anchor, end); endMs = Math.max(anchor, end); } }}></canvas>
<label>{ru.vlStart}<input type="number" min="0" max={selected.durationMs} step="1" bind:value={startMs} /></label>
<label>{ru.vlEnd}<input type="number" min="0" max={selected.durationMs} step="1" bind:value={endMs} /></label>
<button disabled={!regionValid || endMs - startMs > 60000} onclick={() => run(async () => { const version = selectionVersion; const audio = await api.vl_source_audio(selected!.id, startMs, endMs); if (version === selectionVersion) replaceAudio(api.blobUrl(audio)); })}>{ru.vlListenRegion}</button>
{#if url}<audio controls src={url}></audio>{/if}
<form onsubmit={e => { e.preventDefault(); void run(clone); }}>
<label>{ru.vlTranscript}<textarea bind:value={transcript}></textarea></label>
<label>{ru.vlLanguage}<input required bind:value={lang} /></label>
<label>{ru.vlName}<input required bind:value={name} /></label>
<label>{ru.vlPreset}<select bind:value={preset}><option value="fast">{ru.vlFast}</option><option value="standard">{ru.vlStandard}</option><option value="precise">{ru.vlPrecise}</option></select></label>
<p class="muted">{ru.vlCloneCpuNote}</p>
<label>{ru.vlSeed}<input type="number" min="0" max={Number.MAX_SAFE_INTEGER} step="1" required bind:value={seed} /></label>
<button class="primary" disabled={!cloneValid || busy || !listening || job?.status === 'running' || (lang === 'ru' && !transcript.trim())}>{ru.vlClone}</button>
</form></div>{/if}
{#if job}<div class="card"><p>{ru.vlJobStatus[job.status as keyof typeof ru.vlJobStatus]}</p>
{#if job.progress?.event === 'progress'}<p>{ru.vlStage}: {ru.vlStages[job.progress.stage as keyof typeof ru.vlStages] ?? ru.vlClone} · {ru.vlIteration}: {job.progress.iteration}/{job.progress.total} · {ru.vlLoss}: {job.progress.loss}</p><progress value={job.progress.iteration} max={job.progress.total}></progress>{/if}
{#if job.status === 'running'}<p>{ru.vlElapsed}: {duration(elapsed)} · {ru.vlEta}: {eta === null ? ru.vlEtaPending : `≈ ${duration(eta)}`}</p><button onclick={() => run(() => api.vl_cancel_job(job!.id))}>{ru.vlCancel}</button>{/if}</div>{/if}
<style>canvas{width:100%;touch-action:none;border:1px solid var(--border)}audio{display:block;margin:16px 0;max-width:100%}</style>
