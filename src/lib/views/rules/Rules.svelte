<script lang="ts">
import { onMount } from 'svelte';
import { watchRuleErrors, type Settings, type Trigger } from '$lib/api';
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
<ActionCard kind="music" action={settings.rules.music} {errors} {validate} retiring={settings.rules.retiring.some(r => r.action === 'music' && errors[`retire:${r.reward_id}`])}>
{#snippet settingsContent()}<div class="settings"><MusicSettings {settings} /></div>{/snippet}
</ActionCard>
<ActionCard kind="tts" action={settings.rules.tts} {errors} {validate} retiring={settings.rules.retiring.some(r => r.action === 'tts' && errors[`retire:${r.reward_id}`])}>
{#snippet settingsContent()}<div class="settings"><TtsSettings {settings} {run} /></div>{/snippet}
{#snippet runtimeContent()}<Tts {runningPort} {run} />{/snippet}
</ActionCard>
{#if formError}<p role="alert">{formError}</p>{/if}
<div><button class="primary" onclick={submit}>{ru.save}</button></div>
</div>
<style>
.rules { display: grid; gap: var(--s8); padding-top: var(--s6); }
</style>
