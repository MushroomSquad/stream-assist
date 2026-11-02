<script lang="ts">
import type { Settings } from '$lib/api';
import { ru } from '$lib/i18n/ru';
let { settings, save }: { settings: Settings; save: () => void } = $props();
</script>
<section><h2>{ru.rewards}</h2><form onsubmit={e => { e.preventDefault(); save(); }}>
<label>{ru.title}<input required bind:value={settings.reward_title} /></label>
<label>{ru.cost}<input required type="number" min="1" max="2147483647" bind:value={settings.reward_cost} /></label>
<label>{ru.cooldown}<input required type="number" min="0" max="604800" bind:value={settings.reward_cooldown_s} /></label>
<button class="primary">{ru.save}</button></form></section>
