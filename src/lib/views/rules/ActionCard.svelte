<script lang="ts">
import type { Snippet } from 'svelte';
import type { Action, Trigger } from '$lib/api';
import { ru } from '$lib/i18n/ru';
import TriggerRow from './TriggerRow.svelte';
import TriggerEditor from './TriggerEditor.svelte';
let { kind, action, errors, retiring, validate, settingsContent, runtimeContent }: { kind: 'music' | 'tts'; action: Action<unknown>; errors: Record<string, string>; retiring: boolean; validate: (t: Trigger, skip?: Trigger) => string; settingsContent: Snippet; runtimeContent?: Snippet } = $props();
let editing = $state<Trigger | null | undefined>(undefined);
let deleting = $state<Trigger | null>(null);
function apply(t: Trigger) {
  const error = validate(t, editing ?? undefined);
  if (error) return error;
  if (editing) action.triggers[action.triggers.indexOf(editing)] = t;
  else if (action.triggers.length < 10) action.triggers.push(t);
  editing = undefined;
  return '';
}
function remove(t: Trigger) { action.triggers.splice(action.triggers.indexOf(t), 1); deleting = null; if (editing === t) editing = undefined; }
</script>
<section class="card">
<header><span class="tally" class:live={action.enabled} aria-hidden="true"></span><h2>{kind === 'music' ? ru.music : ru.tts}</h2><label class="check"><input type="checkbox" role="switch" bind:checked={action.enabled} aria-label={`${ru.enabled}: ${kind === 'music' ? ru.music : ru.tts}`} /></label></header>
{#if retiring}<p class="notice" role="alert">{ru.retireFailed}</p>{/if}
<div class="triggers">
{#each action.triggers as trigger}
<TriggerRow {trigger} active={action.enabled} error={errors[trigger.id]} edit={() => editing = trigger} remove={() => trigger.kind === 'reward' && trigger.reward_id ? deleting = trigger : remove(trigger)} />
{#if deleting === trigger}<div class="confirm"><p>{ru.deleteRewardConfirm}</p><div class="actions"><button type="button" onclick={() => remove(trigger)}>{ru.remove}</button><button type="button" onclick={() => deleting = null}>{ru.cancel}</button></div></div>{/if}
{/each}
{#if editing !== undefined}{#key editing}<TriggerEditor action={kind} initial={editing} {apply} cancel={() => editing = undefined} />{/key}{:else}<button class="add" type="button" disabled={action.triggers.length >= 10} title={ru.triggerLimit} onclick={() => editing = null}>{ru.addTrigger}</button>{/if}
</div>
<details><summary>{ru.settingsLimits}<span class="mono">{action.triggers.length} / 10</span></summary><div class="contents">{@render settingsContent()}</div></details>
{#if runtimeContent}<details><summary>{ru.queueWidget}</summary><div class="contents">{@render runtimeContent()}</div></details>{/if}
</section>
<style>
.card { margin: 0; }
header { display: flex; align-items: center; gap: var(--s3); margin-bottom: var(--s5); }
h2 { font-size: var(--type-card); line-height: var(--leading-card); margin: 0; }
header label { margin: 0 0 0 auto; }
header input { appearance: none; width: var(--s8); height: 18px; padding: 2px; border: 0; border-radius: var(--r-switch); background: var(--tally-off); cursor: pointer; }
header input::before { content: ''; display: block; width: 14px; height: 14px; border-radius: var(--r-round); background: var(--t2); transition: transform var(--fast) var(--ease); }
header input:checked { background: var(--accent); }
header input:checked::before { background: var(--accent-ink); transform: translateX(14px); }
.triggers { display: grid; gap: var(--s2); }
.add { border: 1px dashed var(--line-strong); background: transparent; color: var(--t2); border-radius: var(--r-row); }
details { border-top: 1px solid var(--line); margin-top: var(--s5); padding-top: var(--s4); }
summary { cursor: pointer; color: var(--t2); }
summary span { float: right; color: var(--t3); font-size: var(--type-meta); }
.contents { padding-top: var(--s5); }
details[open] .contents { animation: reveal var(--mid) var(--ease); }
.confirm { padding: var(--s4); border: 1px solid var(--line); background: var(--warn-bg); border-radius: var(--r-row); }
@keyframes reveal { from { opacity: 0; transform: translateY(4px); } to { opacity: 1; transform: translateY(0); } }
</style>
