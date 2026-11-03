<script lang="ts">
import { formatTime } from '$lib/time';
import { onMount } from 'svelte';
import { watchRuleErrors, type Settings, type Trigger } from '$lib/api';
import * as ttsApi from '$lib/ttsApi';
import { vl_profiles, type Profile } from '$lib/voiceApi';
import { ru } from '$lib/i18n/ru';
import ActionCard from './ActionCard.svelte';
import MusicSettings from './MusicSettings.svelte';
import TtsSettings from './TtsSettings.svelte';
import Tts from '../Tts.svelte';
let { settings, runningPort, save, run }: { settings: Settings; runningPort: number; save: () => void; run: (f: () => Promise<unknown>) => Promise<void> } = $props();
let errors = $state<Record<string, string>>({});
let formError = $state('');
let root: HTMLDivElement;
onMount(() => watchRuleErrors(e => errors = e));
let musicEditing = $state(false), ttsEditing = $state(false);
let profiles = $state<Profile[]>([]);
let view = $state<ttsApi.TtsView>({ current: null, queue: [], widget: false, warning: null });
onMount(() => {
  let disposed = false, updated = false;
  let unlisten = () => {};
  void run(async () => { const loaded = await vl_profiles(); if (!disposed) profiles = loaded; });
  void run(async () => {
    const cleanup = await ttsApi.onTts(v => { updated = true; if (!disposed) view = v; });
    if (disposed) { cleanup(); return; }
    unlisten = cleanup;
    const initial = await ttsApi.tts_state();
    if (!disposed && !updated) view = initial;
  });
  return () => { disposed = true; unlisten(); };
});
const limits = $derived(settings.rules.music.cfg.limits);
const musicSummary = $derived(`${limits.max_duration_ms === null ? ru.durationUnlimited : `${ru.durationUpTo} ${formatTime(limits.max_duration_ms)}`} · ${ru.repeatSummary} ${limits.no_repeat_last_n} · ${limits.max_per_user_in_queue === null ? ru.viewerUnlimited : `${limits.max_per_user_in_queue} ${ru.perViewer}`}`);
const ttsSummary = $derived(`${profiles.find(p => p.id === settings.rules.tts.cfg.profile_id)?.name ?? ru.noVoiceSelected} · ${settings.rules.tts.cfg.lang} · ${settings.rules.tts.cfg.speed.toFixed(1)}×`);
const orphanRetiring = $derived(Object.keys(errors).some(key => key.startsWith('retire:') && !settings.rules.retiring.some(r => key === `retire:${r.reward_id}`)));
const all = $derived([...settings.rules.music.triggers, ...settings.rules.tts.triggers]);
function integer(n: number, min: number, max: number) { return Number.isInteger(n) && n >= min && n <= max; }
function validate(t: Trigger, skip?: Trigger) {
  const others = all.filter(other => other !== skip);
  if (t.kind === 'reward') {
    const title = t.title.trim();
    if ([...title].length < 1 || [...title].length > 45) return ru.rewardTitleInvalid;
    if (others.some(o => o.kind === 'reward' && o.title.trim().toLowerCase() === title.toLowerCase())) return ru.duplicateTitle;
    if (!integer(t.cost, 1, 2147483647) || !integer(t.cooldown_s, 0, 604800)) return ru.rulesInvalid;
  } else {
    if ([...t.marker].length < 1 || [...t.marker].length > 3 || /\s/u.test(t.marker)) return ru.markerInvalid;
    if (others.some(o => o.kind === 'chat_marker' && o.marker === t.marker)) return ru.duplicateMarker;
  }
  return '';
}
function submit() {
  if (musicEditing || ttsEditing) return;
  formError = '';
  for (const field of root.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>('.settings input, .settings select, .settings textarea')) {
    if (!field.checkValidity()) {
      const details = field.closest('details'); if (details) details.open = true;
      field.reportValidity(); return;
    }
  }
  for (const action of [settings.rules.music, settings.rules.tts]) {
    if (action.triggers.length > 10) { formError = ru.triggerLimit; return; }
    for (const t of action.triggers) { const error = validate(t, t); if (error) { formError = error; return; } }
  }
  const cfg = settings.rules.tts.cfg;
  const words = cfg.stop_words.map(w => w.trim()).filter(Boolean);
  if (words.length > 200 || words.some(w => [...w].length > 50)) { formError = ru.ttsStopWordsInvalid; return; }
  if (!Number.isFinite(cfg.speed) || cfg.speed < .7 || cfg.speed > 1.5 || !integer(cfg.global_cooldown_s, 0, 3600) || !integer(cfg.user_cooldown_s, 0, 3600)) { formError = ru.rulesInvalid; return; }
  cfg.stop_words = words;
  save();
}
</script>
<div class="rules" bind:this={root}>
<ActionCard bind:editorOpen={musicEditing} summary={musicSummary} kind="music" action={settings.rules.music} {errors} {validate} retiring={settings.rules.retiring.some(r => r.action === 'music' && errors[`retire:${r.reward_id}`])}>
{#snippet warningsContent()}{#if orphanRetiring}<p class="muted">{ru.retireFailed}</p>{/if}{/snippet}
{#snippet settingsContent()}<div class="settings"><MusicSettings {settings} /></div>{/snippet}
</ActionCard>
<ActionCard bind:editorOpen={ttsEditing} summary={ttsSummary} kind="tts" action={settings.rules.tts} {errors} {validate} retiring={settings.rules.retiring.some(r => r.action === 'tts' && errors[`retire:${r.reward_id}`])}>
{#snippet settingsContent()}<div class="settings"><TtsSettings {settings} {profiles} /></div>{/snippet}
{#snippet warningsContent()}
{#if view.warning}<p class="notice" role="alert">{ru.ttsWarnings[view.warning] ?? view.warning}</p>{/if}
{#if !view.widget && (view.current || view.queue.length > 0)}<p class="notice" role="alert">{ru.ttsWidgetMissing}</p>{/if}
{/snippet}
{#snippet runtimeContent()}<Tts {runningPort} {run} {view} />{/snippet}
</ActionCard>
{#if formError}<p role="alert">{formError}</p>{/if}
<div><button class="primary" disabled={musicEditing || ttsEditing} onclick={submit}>{ru.save}</button>{#if musicEditing || ttsEditing}<p class="muted">{ru.finishTriggerEdit}</p>{/if}</div>
</div>
<style>
.rules { display: grid; gap: var(--s8); padding-top: var(--s6); }
</style>
