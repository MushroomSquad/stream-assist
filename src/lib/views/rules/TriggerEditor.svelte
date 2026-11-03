<script lang="ts">
import { untrack } from 'svelte';
import { allowedTriggers, type Trigger } from '$lib/api';
import { ru } from '$lib/i18n/ru';
let { action, initial, apply, cancel }: { action: 'music' | 'tts'; initial: Trigger | null; apply: (t: Trigger) => string; cancel: () => void } = $props();
let draft = $state<Trigger>(untrack(() => initial ? structuredClone($state.snapshot(initial)) : { id: '', enabled: true, kind: 'reward', title: '', cost: 500, cooldown_s: 0, reward_id: null }));
let error = $state('');
function change(kind: string) {
  draft = kind === 'reward' ? { id: '', enabled: true, kind, title: '', cost: 500, cooldown_s: 0, reward_id: null } : { id: '', enabled: true, kind: 'chat_marker', marker: '' };
  error = '';
}
</script>
<form class="editor" onsubmit={e => { e.preventDefault(); error = apply(draft); }}>
<label>{ru.triggerType}<select value={draft.kind} disabled={initial !== null} onchange={e => change(e.currentTarget.value)}>{#each allowedTriggers[action] as kind}<option value={kind}>{kind === 'reward' ? ru.ttsReward : ru.ttsMarker}</option>{/each}</select></label>
{#if draft.kind === 'reward'}
<label>{ru.title}<input required bind:value={draft.title} /></label>
<label>{ru.cost}<input required type="number" min="1" max="2147483647" step="1" bind:value={draft.cost} /></label>
<label>{ru.cooldown}<input required type="number" min="0" max="604800" step="1" bind:value={draft.cooldown_s} /></label>
{:else}<label>{ru.ttsMarker}<input required pattern={'\\S{1,3}'} bind:value={draft.marker} /></label><p class="muted">{ru.ttsMarkerHint}</p>{/if}
{#if error}<p role="alert">{error}</p>{/if}
<div class="actions"><button class="primary">{ru.save}</button><button type="button" onclick={cancel}>{ru.cancel}</button></div>
</form>
<style>
.editor { padding: var(--s4); background: var(--bg); border: 1px solid var(--line); border-radius: var(--r-row); display: grid; gap: var(--s4); }
label { margin: 0; max-width: none; }
.actions { margin: 0; }
p, .actions { grid-column: 1 / -1; }
@media (min-width: 1100px) { .editor { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
</style>
