<script lang="ts">
import type { Settings } from '$lib/api';
import { ru } from '$lib/i18n/ru';
let { settings, save }: { settings: Settings; save: () => void } = $props();
</script>
<section><h2>{ru.limits}</h2><form onsubmit={e => { e.preventDefault(); save(); }}>
<label>{ru.duration}<input type="number" min="1" value={settings.rules.music.cfg.limits.max_duration_ms ?? ''} oninput={e => settings.rules.music.cfg.limits.max_duration_ms = e.currentTarget.value === '' ? null : Number(e.currentTarget.value)} /></label><p class="muted">{ru.unlimited}</p>
<label>{ru.repeat}<input required type="number" min="0" bind:value={settings.rules.music.cfg.limits.no_repeat_last_n} /></label>
<label>{ru.quota}<input type="number" min="1" value={settings.rules.music.cfg.limits.max_per_user_in_queue ?? ''} oninput={e => settings.rules.music.cfg.limits.max_per_user_in_queue = e.currentTarget.value === '' ? null : Number(e.currentTarget.value)} /></label>
<label class="check"><input type="checkbox" bind:checked={settings.rules.music.cfg.limits.block_explicit} />{ru.explicit}</label>
<label>{ru.blocklist}<textarea rows="5" value={settings.rules.music.cfg.limits.blocklist.join('\n')} oninput={e => settings.rules.music.cfg.limits.blocklist = e.currentTarget.value.split('\n')}></textarea></label>
<button class="primary">{ru.save}</button></form></section>
