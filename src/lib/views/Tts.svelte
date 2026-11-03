<script lang="ts">
import { onMount } from 'svelte';
import type { Settings } from '$lib/api';
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
async function test() {
  testing = true;
  try { await run(() => api.tts_test(text.trim())); } finally { testing = false; }
}
</script>
<section>
<h2>{ru.tts}</h2>
{#if view.warning}<p class="notice" role="alert">{ru.ttsWarnings[view.warning] ?? view.warning}</p>{/if}
{#if !view.widget && (view.current || view.queue.length > 0)}<p class="notice" role="alert">{ru.ttsWidgetMissing}</p>{/if}
<form onsubmit={e => { e.preventDefault(); save(); }}>
<label>{ru.ttsVoice}<select bind:value={settings.tts.profile_id}>
<option value={null}>{ru.ttsSelectVoice}</option>
{#if settings.tts.profile_id !== null && !profiles.some(p => p.id === settings.tts.profile_id)}<option value={settings.tts.profile_id}>{ru.ttsWarnings.tts_voice_missing}</option>{/if}
{#each profiles as profile (profile.id)}<option value={profile.id}>{profile.name}</option>{/each}
</select></label>
<label>{ru.vlLanguage}<select bind:value={settings.tts.lang}>{#each languages as lang}<option value={lang}>{lang}</option>{/each}</select></label>
<label>{ru.vlSpeed}<input required type="number" min="0.7" max="1.5" step="0.01" bind:value={settings.tts.speed} /></label>
<fieldset class="card"><legend>{ru.ttsReward}</legend>
<label class="check"><input type="checkbox" bind:checked={settings.tts.reward_enabled} />{ru.ttsRewardEnabled}</label>
<label>{ru.title}<input required maxlength="45" bind:value={settings.tts.reward_title} /></label>
<label>{ru.cost}<input required type="number" min="1" max="2147483647" bind:value={settings.tts.reward_cost} /></label>
<label>{ru.cooldown}<input required type="number" min="0" max="604800" bind:value={settings.tts.reward_cooldown_s} /></label>
</fieldset>
<fieldset class="card"><legend>{ru.ttsChat}</legend>
<label class="check"><input type="checkbox" bind:checked={settings.tts.chat_enabled} />{ru.ttsChatEnabled}</label>
<label>{ru.ttsMarker}<input required pattern={'\\S{1,3}'} aria-describedby="tts-marker-hint" bind:value={settings.tts.marker} /></label>
<p id="tts-marker-hint" class="muted">{ru.ttsMarkerHint}</p>
<h3>{ru.ttsRoles}</h3>
<label class="check"><input type="checkbox" bind:checked={settings.tts.roles.everyone} />{ru.ttsEveryone}</label>
<label class="check"><input type="checkbox" bind:checked={settings.tts.roles.subscribers} />{ru.ttsSubscribers}</label>
<label class="check"><input type="checkbox" bind:checked={settings.tts.roles.vips} />{ru.ttsVips}</label>
<label class="check"><input type="checkbox" bind:checked={settings.tts.roles.moderators} />{ru.ttsModerators}</label>
<label>{ru.ttsGlobalCooldown}<input required type="number" min="0" max="3600" bind:value={settings.tts.global_cooldown_s} /></label>
<label>{ru.ttsUserCooldown}<input required type="number" min="0" max="3600" bind:value={settings.tts.user_cooldown_s} /></label>
<p class="muted">{ru.ttsCooldownHint}</p>
</fieldset>
<fieldset class="card"><legend>{ru.ttsFilters}</legend>
<label class="check"><input type="checkbox" bind:checked={settings.tts.strip_links} />{ru.ttsStripLinks}</label>
<label>{ru.ttsStopWords}<textarea rows="6" value={settings.tts.stop_words.join('\n')} oninput={e => settings.tts.stop_words = e.currentTarget.value.split('\n')} aria-describedby="tts-stop-hint"></textarea></label>
<p id="tts-stop-hint" class="muted">{ru.ttsStopWordsHint}</p>
</fieldset>
<button class="primary">{ru.save}</button>
</form>
<p class="muted">{ru.ttsBusyNote}</p>
<div class="card">
<label>{ru.url}<input readonly value={`http://127.0.0.1:${runningPort}/widget/tts`} /></label>
<button onclick={() => void run(api.obs_add_tts)}>{ru.addObs}</button>
</div>
<h3>{ru.queue}</h3>
<p class="muted">{ru.ttsLimits}</p>
{#if view.current}
<div class="card"><strong>{ru.now}</strong> · {view.current.user} · {view.current.reward ? ru.ttsReward : ru.ttsChat}
{#if view.current.synthesizing}<span role="status"> · {ru.ttsSynthesizing}</span>{/if}
<p>{view.current.text}</p></div>
{/if}
{#if view.queue.length}<ol>{#each view.queue as item (item.id)}<li><div><strong>{item.user}</strong> · {item.reward ? ru.ttsReward : ru.ttsChat}<p>{item.text}</p></div></li>{/each}</ol>
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
