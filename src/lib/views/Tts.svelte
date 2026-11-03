<script lang="ts">
import { onMount } from 'svelte';
import { rule_errors, onRuleErrors, type Settings } from '$lib/api';
import { errorText } from '$lib/i18n/ru';
import * as api from '$lib/ttsApi';
import { vl_profiles, type Profile } from '$lib/voiceApi';
import { ru } from '$lib/i18n/ru';
let { settings, runningPort, save, run }: { settings: Settings; runningPort: number; save: () => void; run: (f: () => Promise<unknown>) => Promise<void> } = $props();
let view = $state<api.TtsView>({ current: null, queue: [], widget: false, warning: null });
let profiles = $state<Profile[]>([]);
let text = $state('');
let testing = $state(false);
const languages = ['en', 'ko', 'ja', 'ar', 'bg', 'cs', 'da', 'de', 'el', 'es', 'et', 'fi', 'fr', 'hi', 'hr', 'hu', 'id', 'it', 'lt', 'lv', 'nl', 'pl', 'pt', 'ro', 'ru', 'sk', 'sl', 'sv', 'tr', 'uk', 'vi', 'na'];
onMount(() => {
  let disposed = false, updated = false;
  let unlisten = () => {};
  void run(async () => {
    const cleanup = await api.onTts(v => { updated = true; if (!disposed) view = v; });
    if (disposed) { cleanup(); return; }
    unlisten = cleanup;
    const initial = await api.tts_state();
    if (!disposed && !updated) view = initial;
  });
  void run(async () => { const loaded = await vl_profiles(); if (!disposed) profiles = loaded; });
  return () => { disposed = true; unlisten(); };
});
let errors = $state<Record<string, string>>({});
onMount(() => {
  let disposed = false, updated = false;
  let unlisten = () => {};
  void (async () => {
    const cleanup = await onRuleErrors(e => { updated = true; if (!disposed) errors = e; });
    if (disposed) { cleanup(); return; }
    unlisten = cleanup;
    const initial = await rule_errors();
    if (!disposed && !updated) errors = initial;
  })().catch(() => {});
  return () => { disposed = true; unlisten(); };
});
let reward = $derived(settings.rules.tts.triggers.find(t => t.kind === 'reward'));
let marker = $derived(settings.rules.tts.triggers.find(t => t.kind === 'chat_marker'));
let formError = $state('');
function submit() {
  const t = settings.rules.tts.cfg;
  t.stop_words = t.stop_words.map(w => w.trim()).filter(Boolean);
  const title = reward?.kind === 'reward' ? reward.title.trim().toLowerCase() : null;
  const otherTitles = [...settings.rules.music.triggers, ...settings.rules.tts.triggers].filter(r => r.id !== reward?.id && r.kind === 'reward').map(r => r.kind === 'reward' ? r.title.trim().toLowerCase() : '');
  formError = title === '' ? ru.ttsTitleEmpty
    : title !== null && otherTitles.includes(title) ? ru.ttsTitleConflict
    : t.stop_words.length > 200 || t.stop_words.some(w => [...w].length > 50) ? ru.ttsStopWordsInvalid
    : '';
  if (!formError) save();
}
async function test() {
  testing = true;
  try { await run(() => api.tts_test(text.trim())); } finally { testing = false; }
}
</script>
<section>
<h2>{ru.tts}</h2>
{#if view.warning}<p class="notice" role="alert">{ru.ttsWarnings[view.warning] ?? view.warning}</p>{/if}
{#if !view.widget && (view.current || view.queue.length > 0)}<p class="notice" role="alert">{ru.ttsWidgetMissing}</p>{/if}
{#if formError}<p class="notice" role="alert">{formError}</p>{/if}
<form onsubmit={e => { e.preventDefault(); submit(); }}>
<label>{ru.ttsVoice}<select bind:value={settings.rules.tts.cfg.profile_id}>
<option value={null}>{ru.ttsSelectVoice}</option>
{#if settings.rules.tts.cfg.profile_id !== null && !profiles.some(p => p.id === settings.rules.tts.cfg.profile_id)}<option value={settings.rules.tts.cfg.profile_id}>{ru.ttsVoiceDeleted}</option>{/if}
{#each profiles as profile (profile.id)}<option value={profile.id}>{profile.name}</option>{/each}
</select></label>
<label>{ru.vlLanguage}<select bind:value={settings.rules.tts.cfg.lang}>{#each languages as lang}<option value={lang}>{lang}</option>{/each}</select></label>
<label>{ru.vlSpeed}<input required type="number" min="0.7" max="1.5" step="0.01" bind:value={settings.rules.tts.cfg.speed} /></label>
{#if reward?.kind === 'reward'}<fieldset class="card"><legend>{ru.ttsReward}</legend>
<label class="check"><input type="checkbox" bind:checked={reward.enabled} />{ru.ttsRewardEnabled}</label>
<label>{ru.title}<input required bind:value={reward.title} oninput={e => { const n = [...e.currentTarget.value.trim()].length; e.currentTarget.setCustomValidity(n >= 1 && n <= 45 ? '' : ru.rewardTitleInvalid); }} /></label>
<label>{ru.cost}<input required type="number" min="1" max="2147483647" bind:value={reward.cost} /></label>
<label>{ru.cooldown}<input required type="number" min="0" max="604800" bind:value={reward.cooldown_s} /></label>
</fieldset>
{#if errors[reward.id]}<p class="notice" role="alert">{errorText(errors[reward.id])}</p>{/if}
{/if}
<fieldset class="card"><legend>{ru.ttsChat}</legend>
{#if marker?.kind === 'chat_marker'}
<label class="check"><input type="checkbox" bind:checked={marker.enabled} />{ru.ttsChatEnabled}</label>
<label>{ru.ttsMarker}<input required pattern={'\\S{1,3}'} aria-describedby="tts-marker-hint" bind:value={marker.marker} /></label>
<p id="tts-marker-hint" class="muted">{ru.ttsMarkerHint}</p>
{/if}
<h3>{ru.ttsRoles}</h3>
<label class="check"><input type="checkbox" bind:checked={settings.rules.tts.cfg.roles.everyone} />{ru.ttsEveryone}</label>
<label class="check"><input type="checkbox" bind:checked={settings.rules.tts.cfg.roles.subscribers} />{ru.ttsSubscribers}</label>
<label class="check"><input type="checkbox" bind:checked={settings.rules.tts.cfg.roles.vips} />{ru.ttsVips}</label>
<label class="check"><input type="checkbox" bind:checked={settings.rules.tts.cfg.roles.moderators} />{ru.ttsModerators}</label>
<label>{ru.ttsGlobalCooldown}<input required type="number" min="0" max="3600" bind:value={settings.rules.tts.cfg.global_cooldown_s} /></label>
<label>{ru.ttsUserCooldown}<input required type="number" min="0" max="3600" bind:value={settings.rules.tts.cfg.user_cooldown_s} /></label>
<p class="muted">{ru.ttsCooldownHint}</p>
</fieldset>
<fieldset class="card"><legend>{ru.ttsFilters}</legend>
<label class="check"><input type="checkbox" bind:checked={settings.rules.tts.cfg.strip_links} />{ru.ttsStripLinks}</label>
<label>{ru.ttsStopWords}<textarea rows="6" value={settings.rules.tts.cfg.stop_words.join('\n')} oninput={e => settings.rules.tts.cfg.stop_words = e.currentTarget.value.split('\n')} aria-describedby="tts-stop-hint"></textarea></label>
<p id="tts-stop-hint" class="muted">{ru.ttsStopWordsHint}</p>
</fieldset>
<button class="primary">{ru.save}</button>
</form>
<p class="muted">{ru.ttsBusyNote}</p>
<div class="card">
<label>{ru.url}<input readonly value={`http://127.0.0.1:${runningPort}/widget/tts`} /></label>
<div class="actions"><button onclick={() => void run(() => navigator.clipboard.writeText(`http://127.0.0.1:${runningPort}/widget/tts`))}>{ru.copy}</button>
<button onclick={() => void run(api.obs_add_tts)}>{ru.addObs}</button></div>
</div>
<h3>{ru.queue}</h3>
<p class="muted">{ru.ttsLimits}</p>
{#if view.current}
<div class="card"><strong>{ru.now}</strong> · {view.current.user || ru.ttsTest} · {view.current.reward ? ru.ttsReward : ru.ttsChat}
{#if view.current.synthesizing}<span role="status"> · {ru.ttsSynthesizing}</span>{/if}
<p>{view.current.text}</p></div>
{/if}
{#if view.queue.length}<ol>{#each view.queue as item (item.id)}<li><div><strong>{item.user || ru.ttsTest}</strong> · {item.reward ? ru.ttsReward : ru.ttsChat}<p>{item.text}</p></div></li>{/each}</ol>
{:else if !view.current}<p class="muted">{ru.ttsEmpty}</p>{/if}
<div class="actions">
<button disabled={!view.current} onclick={() => void run(api.tts_skip)}>{ru.skip}</button>
<button disabled={!view.queue.length} onclick={() => void run(api.tts_clear)}>{ru.ttsClear}</button>
</div>
<form onsubmit={e => { e.preventDefault(); void test(); }}>
<label>{ru.ttsTestText}<textarea required rows="3" bind:value={text}></textarea></label>
<button disabled={testing || !text.trim() || [...text.trim()].length > 300}>{ru.ttsTest}</button>
</form>
</section>
