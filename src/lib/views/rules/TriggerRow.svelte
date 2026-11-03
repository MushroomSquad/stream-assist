<script lang="ts">
import type { Trigger } from '$lib/api';
import { ru } from '$lib/i18n/ru';
let { trigger, active, error, edit, remove }: { trigger: Trigger; active: boolean; error?: string; edit: () => void; remove: () => void } = $props();
</script>
<div class="row">
<span class="tally" class:live={active && trigger.enabled} aria-hidden="true"></span>
<span class="kind">{trigger.kind === 'reward' ? ru.ttsReward : ru.ttsMarker}</span>
<div class="parameters" class:off={!active || !trigger.enabled}>
{#if trigger.kind === 'reward'}<span class="title">{trigger.title}</span><span class="meta mono">{trigger.cost} {ru.points} · {ru.cooldownShort} {trigger.cooldown_s} {ru.secondsShort}</span>
{:else}<span class="mono">{trigger.marker}</span>{/if}
</div>
<input class="switch" type="checkbox" role="switch" aria-label={`${ru.enabled}: ${trigger.kind === 'reward' ? trigger.title : trigger.marker}`} bind:checked={trigger.enabled} />
<button type="button" onclick={edit}>{ru.edit}</button><button type="button" onclick={remove}>{ru.remove}</button>
</div>
{#if error}<p class="row-error" role="alert">{error}</p>{/if}
<style>
.row { display: flex; align-items: center; gap: var(--s3); min-height: 44px; padding: var(--s1) var(--s3); border-radius: var(--r-row); background: var(--raised); }
.kind { width: 120px; flex-shrink: 0; color: var(--t3); font-size: var(--type-secondary); }
.parameters { flex: 1; min-width: 0; display: flex; flex-wrap: wrap; gap: var(--s2) var(--s4); overflow-wrap: anywhere; }
.parameters.off { opacity: .5; }
.title { font-weight: 500; }
.meta { color: var(--t3); font-size: var(--type-meta); }
button { background: transparent; border-color: transparent; color: var(--t3); padding-inline: var(--s1); }
.row-error { font-size: var(--type-meta); margin: var(--s1) var(--s3) 0; }
.switch { appearance: none; width: var(--s8); height: 18px; padding: 2px; border: 0; border-radius: var(--r-switch); background: var(--tally-off); flex-shrink: 0; cursor: pointer; }
.switch::before { content: ''; display: block; width: 14px; height: 14px; border-radius: var(--r-round); background: var(--t2); transition: transform var(--fast) var(--ease); }
.switch:checked { background: var(--accent); }
.switch:checked::before { background: var(--accent-ink); transform: translateX(14px); }
@media (max-width: 1099px) { .kind { width: auto; } .row { flex-wrap: wrap; } .parameters { flex-basis: 45%; } }
</style>
