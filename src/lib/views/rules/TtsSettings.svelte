<script lang="ts">
import type { Settings } from '$lib/api';
import { type Profile } from '$lib/voiceApi';
import { ru } from '$lib/i18n/ru';
let { settings, profiles }: { settings: Settings; profiles: Profile[] } = $props();
const languages = ['en', 'ko', 'ja', 'ar', 'bg', 'cs', 'da', 'de', 'el', 'es', 'et', 'fi', 'fr', 'hi', 'hr', 'hu', 'id', 'it', 'lt', 'lv', 'nl', 'pl', 'pt', 'ro', 'ru', 'sk', 'sl', 'sv', 'tr', 'uk', 'vi', 'na'];
</script>
<div class="fields"><label>{ru.ttsVoice}<select bind:value={settings.rules.tts.cfg.profile_id}>
<option value={null}>{ru.ttsSelectVoice}</option>
{#if settings.rules.tts.cfg.profile_id !== null && !profiles.some(p => p.id === settings.rules.tts.cfg.profile_id)}<option value={settings.rules.tts.cfg.profile_id}>{ru.ttsVoiceDeleted}</option>{/if}
{#each profiles as profile (profile.id)}<option value={profile.id}>{profile.name}</option>{/each}
</select></label>
<label>{ru.vlLanguage}<select bind:value={settings.rules.tts.cfg.lang}>{#each languages as lang}<option value={lang}>{lang}</option>{/each}</select></label>
<label>{ru.vlSpeed}<input required type="number" min="0.7" max="1.5" step="0.01" bind:value={settings.rules.tts.cfg.speed} /></label>
<h3>{ru.ttsRoles}</h3>
<label class="check"><input type="checkbox" bind:checked={settings.rules.tts.cfg.roles.everyone} />{ru.ttsEveryone}</label>
<label class="check"><input type="checkbox" bind:checked={settings.rules.tts.cfg.roles.subscribers} />{ru.ttsSubscribers}</label>
<label class="check"><input type="checkbox" bind:checked={settings.rules.tts.cfg.roles.vips} />{ru.ttsVips}</label>
<label class="check"><input type="checkbox" bind:checked={settings.rules.tts.cfg.roles.moderators} />{ru.ttsModerators}</label>
<label>{ru.ttsGlobalCooldown}<input required type="number" min="0" max="3600" bind:value={settings.rules.tts.cfg.global_cooldown_s} /></label>
<label>{ru.ttsUserCooldown}<input required type="number" min="0" max="3600" bind:value={settings.rules.tts.cfg.user_cooldown_s} /></label>
<p class="muted">{ru.ttsCooldownHint}</p>

<h3>{ru.ttsFilters}</h3>
<label class="check"><input type="checkbox" bind:checked={settings.rules.tts.cfg.strip_links} />{ru.ttsStripLinks}</label>
<label>{ru.ttsStopWords}<textarea rows="6" value={settings.rules.tts.cfg.stop_words.join('\n')} oninput={e => settings.rules.tts.cfg.stop_words = e.currentTarget.value.split('\n')} aria-describedby="tts-stop-hint"></textarea></label>
<p id="tts-stop-hint" class="muted">{ru.ttsStopWordsHint}</p>

</div>
<style>
.fields { display: grid; align-items: start; gap: var(--s4); }
label { margin: 0; max-width: none; }
h3, p { grid-column: 1 / -1; margin: 0; }
@media (min-width: 1100px) { .fields { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
</style>
