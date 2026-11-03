<script lang="ts">
import { onMount } from 'svelte';
import * as api from '$lib/voiceApi';
import { ru, errorText } from '$lib/i18n/ru';
let profiles = $state<api.Profile[]>([]), sources = $state<api.Source[]>([]);
let mode = $state<'interp' | 'random'>('interp'), a = $state(0), b = $state(0), points = $state(5);
let ids = $state<number[]>([]), k = $state(4), seed = $state(7), dp = $state<'dominant' | 'weighted'>('weighted');
let params = $state<api.RenderParams>({ text: ru.vlSamplePhrase, lang: 'ru', speed: 1, quality: 8 });
let compare = $state(false), sourceId = $state(0), startMs = $state(0), endMs = $state(10000);
let busy = $state(false), listening = $state(false), message = $state(''), cancelled = $state(false);
let job = $state<api.JobState | null>(null), total = $state(0), done = $state(0);
let items = $state<api.ExploreItem[]>([]), lastExplore = $state<api.Explore | null>(null);
let urls = $state<Record<number, string>>({}), names = $state<Record<number, string>>({});
let saving = $state<Record<number, boolean>>({}), saved = $state<Record<number, boolean>>({});
let restored = $state(false);
const timers = new Set<ReturnType<typeof setTimeout>>();
let disposed = false;
const source = $derived(sources.find(s => s.id === sourceId));
const valid = $derived(Array.from(params.text.trim()).length >= 1 && Array.from(params.text.trim()).length <= 300 &&
  (mode === 'interp' ? a !== b && !!a && !!b && Number.isInteger(points) && points >= 3 && points <= 9 : ids.length >= 2 && ids.length <= 8 && Number.isInteger(k) && k >= 2 && k <= 8 && Number.isInteger(seed) && seed >= 0 && seed <= 2 ** 32 - 1) &&
  (!compare || (!!source?.consent && Number.isInteger(startMs) && Number.isInteger(endMs) && startMs >= 0 && endMs <= source.durationMs && endMs - startMs >= 1000 && endMs - startMs <= 60000)));
function ranked(items: api.ExploreItem[]) {
  return items.map((item, index) => ({ ...item, index })).sort((a, b) => (b.similarity ?? -Infinity) - (a.similarity ?? -Infinity) || a.index - b.index);
}
function interpolation(item: api.ExploreItem) {
  return (item.render.recipe?.inputs[1]?.weight ?? 0) / 100;
}
function snapshotInputs(recipe: api.Recipe): [number, number][] {
  return recipe.inputs.map(input => { const profile = profiles.find(p => p.styleSha256 === input.profileSha256); if (!profile) throw new Error('NOT_FOUND'); return [profile.id, input.weight]; });
}
async function run(f: () => Promise<void>) { message = ''; try { await f(); } catch (e) { if (!disposed) message = errorText(e); } }
function clearAudio() { Object.values(urls).forEach(URL.revokeObjectURL); urls = {}; }
onMount(() => {
  let stop = () => {};
  const observed = new Map<number, api.JobState>();
  function update(next: api.JobState) {
    job = next;
    if (next.progress?.event === 'item' && next.progress.index !== undefined) done = next.progress.index + 1;
    if (restored) {
      busy = next.status === 'running';
      if (next.error) message = errorText(next.error);
    }
  }
  void run(async () => {
    const unlisten = await api.onJob(next => {
      if (next.kind !== 'batch') return;
      observed.set(next.id, next);
      if (!busy || (job && job.id !== next.id)) return;
      update(next);
    });
    if (disposed) { unlisten(); return; } stop = unlisten;
    const loaded = await Promise.all([api.vl_profiles(), api.vl_sources(), api.vl_jobs()]);
    if (disposed) return;
    const running = loaded[2].map(j => observed.get(j.id) ?? j).find(j => j.kind === 'batch' && j.status === 'running')
      ?? [...observed.values()].find(j => j.status === 'running');
    if (running) { restored = true; busy = true; update(running); }
    else if (loaded[2].some(j => j.kind === 'batch' && j.status === 'running')) restored = true;
    listening = true;
    [profiles, sources] = loaded; a = profiles[0]?.id ?? 0; b = profiles[1]?.id ?? 0;
    sourceId = sources[0]?.id ?? 0;
  });
  return () => { disposed = true; stop(); clearAudio(); timers.forEach(clearTimeout); };
});
async function generate() {
  if (!valid || busy) return;
  const explore: api.Explore = mode === 'interp' ? { mode, a, b, points } : { mode, ids: [...ids], k, seed, dp };
  restored = false; busy = true; job = null; done = 0; total = explore.mode === 'interp' ? explore.points : explore.k;
  clearAudio(); items = []; names = {}; saved = {}; cancelled = false; lastExplore = explore;
  try {
    const result = await api.vl_explore(explore, $state.snapshot(params), compare ? { sourceId, startMs, endMs } : null);
    if (!disposed) { items = result.items; cancelled = result.cancelled; }
  } finally { busy = false; }
}
async function audio(id: number) {
  const buf = await api.vl_render_audio(id);
  if (disposed || !items.some(item => item.render.id === id)) return;
  if (urls[id]) URL.revokeObjectURL(urls[id]); urls[id] = api.blobUrl(buf);
}
async function snapshot(item: api.ExploreItem) {
  const id = item.render.id;
  if (!item.render.recipe || saving[id]) return;
  saving[id] = true; saved[id] = false;
  try {
    await api.vl_save_snapshot(snapshotInputs(item.render.recipe), item.render.recipe.dpStrategy, null, names[id]);
    if (disposed) return;
    names[id] = ''; saved[id] = true;
    const timer = setTimeout(() => { saved[id] = false; timers.delete(timer); }, 2500);
    timers.add(timer);
    profiles = await api.vl_profiles();
  } finally { saving[id] = false; }
}
</script>
<div class="voice-screen">
<h3>{ru.vlExplore}</h3>
<p class="muted">{ru.vlSeedNote}</p>
{#if message}<p class="notice" role="alert">{message}</p>{/if}
<form onsubmit={e => { e.preventDefault(); void run(generate); }}>
<fieldset disabled={busy}>
<label>{ru.vlMode}<select bind:value={mode}><option value="interp">{ru.vlInterp}</option><option value="random">{ru.vlRandom}</option></select></label>
{#if mode === 'interp'}
<label>A<select bind:value={a}>{#each profiles as p}<option value={p.id}>{p.name}</option>{/each}</select></label>
<label>B<select bind:value={b}>{#each profiles as p}<option value={p.id}>{p.name}</option>{/each}</select></label>
<label>{ru.vlPoints}<input required type="number" min="3" max="9" step="1" bind:value={points} /></label>
<p title={ru.vlTNote} class="muted">{ru.vlTNote}</p>
{:else}
<fieldset><legend>{ru.vlProfiles}</legend>
{#each profiles as p}<label class="check"><input type="checkbox" value={p.id} bind:group={ids} disabled={!ids.includes(p.id) && ids.length >= 8} />{p.name}</label>{/each}</fieldset>
<label>{ru.vlCount}<input required type="number" min="2" max="8" step="1" bind:value={k} /></label>
<label>{ru.vlSeed}<input required type="number" min="0" max={2 ** 32 - 1} step="1" bind:value={seed} /></label>
<button type="button" onclick={() => seed = Math.floor(Math.random() * 2 ** 32)}>{ru.vlNewSeed}</button>
<label>{ru.vlDp}<select bind:value={dp}><option value="dominant">{ru.vlDominant}</option><option value="weighted">{ru.vlWeighted}</option></select></label>
{/if}
<label>{ru.vlText} (≤ 300)<textarea required bind:value={params.text}></textarea></label>
<label>{ru.vlLanguage}<input required bind:value={params.lang} /></label>
<label>{ru.vlSpeed}<input required type="number" min="0.7" max="1.5" step="0.1" bind:value={params.speed} /></label>
<label>{ru.vlQuality}<input required type="number" min="5" max="12" step="1" bind:value={params.quality} /></label>
<label class="check"><input type="checkbox" bind:checked={compare} />{ru.vlReference}</label>
{#if compare}
<label>{ru.vlSources}<select bind:value={sourceId} onchange={() => { startMs = 0; endMs = Math.min(10000, sources.find(s => s.id === sourceId)?.durationMs ?? 0); }}>{#each sources as s}<option value={s.id}>{s.name}</option>{/each}</select></label>
{#if !source?.consent}<p class="notice">{ru.vlConsent}</p>{/if}
<label>{ru.vlStart}<input required type="number" min="0" step="1" bind:value={startMs} /></label>
<label>{ru.vlEnd}<input required type="number" min="0" max={source?.durationMs} step="1" bind:value={endMs} /></label>
<p class="muted">{ru.vlRefRegion} · {ru.vlSimilarityNote}</p>
{/if}
<button class="primary" disabled={!valid || busy || !listening}>{ru.vlGenerate}</button>
</fieldset></form>
{#if busy}<p class="job-status metrics" role="status"><span class="tally live" aria-hidden="true"></span>{done}{total ? `/${total}` : ''}</p><progress aria-label={ru.vlBatchProgress} value={total ? done : undefined} max={total || 1}></progress>
{#if job && job.status === 'running'}<button onclick={() => run(async () => { if (job) await api.vl_cancel_job(job.id); })}>{ru.vlCancel}</button>{/if}{/if}
{#if restored && !busy}<p role="status">{ru.vlResultsInRenders}</p>{/if}
{#if cancelled}<p role="status">{ru.vlCancelledPartial}</p>{/if}
<div class="ribbon">{#each ranked(items) as item (item.render.id)}
{@const recipe = item.render.recipe}
{@const endpoint = lastExplore?.mode === 'interp' && recipe?.inputs.some(input => input.weight === 0)}
{@const label = lastExplore?.mode === 'interp' ? `t=${Number(interpolation(item).toFixed(3))}` : `#${item.index + 1} · seed ${lastExplore?.mode === 'random' ? lastExplore.seed : ''}`}
<div class="card">
<p class="metrics" title={lastExplore?.mode === 'interp' ? ru.vlTNote : undefined}>{label}</p>
{#if recipe}{#each recipe.inputs as input}<p class="metrics">{input.name}: {input.weight.toFixed(2)}%</p>{/each}{/if}
{#if item.similarity !== null}<p title={ru.vlSimilarityNote}>{ru.vlSimilarity}: {item.similarity.toFixed(3)}</p>{/if}
<button aria-label={`${ru.vlListen}: ${label}`} onclick={() => run(() => audio(item.render.id))}>{ru.vlListen}</button>
{#if urls[item.render.id]}<audio controls src={urls[item.render.id]}></audio>{/if}
{#if endpoint}<p>{interpolation(item) === 0 ? ru.vlEndpointA : ru.vlEndpointB}</p>
{:else if recipe}<form onsubmit={e => { e.preventDefault(); void run(() => snapshot(item)); }}>
<label>{ru.vlName}<input required maxlength="200" bind:value={names[item.render.id]} /></label><button disabled={saving[item.render.id]}>{ru.vlSaveSnapshot}</button>
{#if saved[item.render.id]}<span role="status">{ru.vlSaved}</span>{/if}</form>{/if}
</div>{/each}</div>

</div>
<style>fieldset{border:0;padding:0;min-width:0}.ribbon{display:flex;gap:var(--s4);overflow-x:auto;align-items:flex-start}.ribbon>.card{min-width:260px}audio{display:block;max-width:100%;margin:var(--s4) 0}</style>
