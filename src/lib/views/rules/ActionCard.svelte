<script lang="ts">
import { tick, type Snippet } from 'svelte';
import type { Action, Trigger } from '$lib/api';
import { ru } from '$lib/i18n/ru';
import TriggerRow from './TriggerRow.svelte';
import TriggerEditor from './TriggerEditor.svelte';
let { kind, action, errors, retiring, validate, settingsContent, runtimeContent, warningsContent, summary, editorOpen = $bindable(false) }: { kind: 'music' | 'tts'; action: Action<unknown>; errors: Record<string, string>; retiring: boolean; validate: (t: Trigger, skip?: Trigger) => string; settingsContent: Snippet; runtimeContent?: Snippet; warningsContent?: Snippet; summary: string; editorOpen?: boolean } = $props();
let editing = $state<number | null | undefined>(undefined);
let deleting = $state<Trigger | null>(null);
let returnFocus: HTMLButtonElement | undefined;
let addButton = $state<HTMLButtonElement>();
$effect(() => { editorOpen = editing !== undefined; });
$effect(() => { action; editing = undefined; deleting = null; });
function open(index: number | null, button: HTMLButtonElement) { returnFocus = button; editing = index; }
async function close() {
  editing = undefined;
  await tick();
  if (returnFocus?.isConnected) returnFocus.focus(); else addButton?.focus();
}
function apply(t: Trigger) {
  const original = editing != null ? action.triggers[editing] : undefined;
  if (editing != null && !original) { void close(); return ''; }
  const error = validate(t, original);
  if (error) return error;
  if (editing != null) action.triggers[editing] = t;
  else if (action.triggers.length < 10) action.triggers.push(t);
  void close();
  return '';
}
function remove(t: Trigger) {
  const index = action.triggers.indexOf(t);
  if (index < 0) return;
  action.triggers.splice(index, 1); deleting = null;
  if (editing === index) void close();
  else if (editing != null && editing > index) editing--;
}
function focusConfirm(node: HTMLButtonElement) { node.focus(); }
</script>
<section class="card">
<header><span class="tally" class:live={action.enabled} aria-hidden="true"></span><h2>{kind === 'music' ? ru.music : ru.tts}</h2><label class="check"><input type="checkbox" role="switch" bind:checked={action.enabled} aria-label={`${ru.enabled}: ${kind === 'music' ? ru.music : ru.tts}`} /></label></header>
{#if retiring}<p class="notice" role="alert">{ru.retireFailed}</p>{/if}
{#if warningsContent}{@render warningsContent()}{/if}
<div class="triggers">
{#each action.triggers as trigger, index}
<TriggerRow {trigger} active={action.enabled} error={errors[trigger.id]} edit={button => open(index, button)} remove={() => trigger.kind === 'reward' && trigger.reward_id ? deleting = trigger : remove(trigger)} />
{#if deleting === trigger}<div class="confirm"><p>{ru.deleteRewardConfirm}</p><div class="actions"><button use:focusConfirm type="button" onclick={() => remove(trigger)}>{ru.remove}</button><button type="button" onclick={() => deleting = null}>{ru.cancel}</button></div></div>{/if}
{/each}
{#if editing !== undefined}{#key editing}<TriggerEditor action={kind} initial={editing === null ? null : action.triggers[editing]} {apply} cancel={() => void close()} />{/key}{:else}<button class="add" type="button" disabled={action.triggers.length >= 10} bind:this={addButton} onclick={e => open(null, e.currentTarget)}>{ru.addTrigger}</button>{/if}
{#if action.triggers.length >= 10}<p class="muted">{ru.triggerLimit}</p>{/if}
</div>
<details><summary>{ru.settingsLimits}<span class="mono">{summary}</span></summary><div class="contents">{@render settingsContent()}</div></details>
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
