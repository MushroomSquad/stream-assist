<script lang="ts">
import { onMount } from 'svelte';
import { rule_errors, onRuleErrors, type Settings } from '$lib/api';
import { errorText } from '$lib/i18n/ru';
import { ru } from '$lib/i18n/ru';
let { settings, save }: { settings: Settings; save: () => void } = $props();
let reward = $derived(settings.rules.music.triggers.find(t => t.kind === 'reward'));
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
</script>
<section><h2>{ru.rewards}</h2>{#if reward?.kind === 'reward'}<form onsubmit={e => { e.preventDefault(); save(); }}>
<label>{ru.title}<input required bind:value={reward.title} oninput={e => { const n = [...e.currentTarget.value.trim()].length; e.currentTarget.setCustomValidity(n >= 1 && n <= 45 ? '' : ru.rewardTitleInvalid); }} /></label>
<label>{ru.cost}<input required type="number" min="1" max="2147483647" bind:value={reward.cost} /></label>
<label>{ru.cooldown}<input required type="number" min="0" max="604800" bind:value={reward.cooldown_s} /></label>
<button class="primary">{ru.save}</button></form>
{#if errors[reward.id]}<p class="notice" role="alert">{errorText(errors[reward.id])}</p>{/if}
{/if}</section>
