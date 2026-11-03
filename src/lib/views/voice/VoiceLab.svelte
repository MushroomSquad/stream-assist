<script lang="ts">
import { onMount } from 'svelte';
import * as api from '$lib/voiceApi';
import { ru, errorText } from '$lib/i18n/ru';
import Setup from './Setup.svelte';
import Sources from './Sources.svelte';
import Voices from './Voices.svelte';
import Mixer from './Mixer.svelte';
import Explore from './Explore.svelte';
import Train from './Train.svelte';
import Renders from './Renders.svelte';
let status = $state<api.ModelStatus | null>(null);
let message = $state('');
let tab = $state('vlSources');
const tabs = ['vlSources', 'vlVoices', 'vlMixer', 'vlExplore', 'vlRenders', 'vlTrain'] as const;
async function load() { try { status = await api.vl_status(); message = ''; } catch (e) { message = errorText(e); } }
onMount(() => { void load(); });
</script>
<section class="voice-lab"><h2>{ru.voiceLab}</h2>
{#if message}<p class="notice" role="alert">{message}</p><button onclick={load}>{ru.reload}</button>{/if}
{#if status}
{#if !status.installed}<Setup {status} installed={load} />
{:else}
{#if !status.ffmpeg}<p class="notice">{ru.vlWavOnly}</p>{/if}
<nav class="segments" aria-label={ru.voiceLab}>{#each tabs as id}<button class:active={tab === id} aria-current={tab === id ? "page" : undefined} onclick={() => tab = id}>{ru[id]}</button>{/each}</nav>
{#if tab === 'vlSources'}<Sources />{:else if tab === 'vlVoices'}<Voices />{:else if tab === 'vlMixer'}<Mixer />{:else if tab === 'vlExplore'}<Explore />{:else if tab === 'vlTrain'}<Train />{:else}<Renders />{/if}
{/if}{/if}</section>

<style>
/* Unslop by Damir Sokolovsky (@heydamir), CC BY 4.0. */
.voice-lab { min-width: 0; }
h2 { margin: 0 0 var(--s6); }
.segments { display: flex; flex-wrap: wrap; gap: var(--s1); padding: var(--s1); margin-bottom: var(--s6); background: var(--raised); border-radius: var(--r-row); width: fit-content; max-width: 100%; }
.segments button { border-color: transparent; background: transparent; color: var(--t2); box-shadow: none; }
.segments button:hover { color: var(--t1); border-color: var(--line-strong); }
.segments button.active { background: var(--card); color: var(--t1); box-shadow: var(--active-highlight); }
.voice-lab :global(.voice-screen) { min-width: 0; }
.voice-lab :global(.voice-screen > :first-child) { margin-top: 0; }
.voice-lab :global(form), .voice-lab :global(fieldset) { display: grid; grid-template-columns: minmax(0, 1fr); gap: var(--s4); min-width: 0; }
.voice-lab :global(fieldset) { border: 0; padding: 0; margin: 0; }
.voice-lab :global(form > :not(label)), .voice-lab :global(fieldset > :not(label)) { grid-column: 1 / -1; }
.voice-lab :global(label) { margin: 0; max-width: none; color: var(--t2); }
.voice-lab :global(label:has(textarea)), .voice-lab :global(label.check) { grid-column: 1 / -1; }
.voice-lab :global(input), .voice-lab :global(select), .voice-lab :global(textarea) { color: var(--t1); width: 100%; }
.voice-lab :global(input[type=checkbox]) { width: var(--s5); flex-shrink: 0; }
.voice-lab :global(textarea) { min-height: var(--s12); }
.voice-lab :global(button) { justify-self: start; max-width: 100%; }
.voice-lab :global(.card) { margin-block: var(--s4); overflow-wrap: anywhere; }
.voice-lab :global(.card > h3:first-child) { margin-top: 0; }
.voice-lab :global(.card > .card) { border-radius: var(--r-control); background: var(--raised); }
.voice-lab :global(.card > form), .voice-lab :global(.card > audio) { margin-block: var(--s4); }
.voice-lab :global(.card > p + button), .voice-lab :global(.card > label + button) { margin-top: var(--s4); }
.voice-lab :global(h4) { margin: 0 0 var(--s3); font-size: var(--type-card); line-height: var(--leading-card); font-weight: 500; }
.voice-lab :global(legend) { margin-bottom: var(--s3); color: var(--t2); }
.voice-lab :global(audio) { display: block; width: 100%; max-width: 100%; margin-block: var(--s4); border-radius: var(--r-control); background: var(--card); accent-color: var(--accent); }
.voice-lab :global(progress) { display: block; margin-block: var(--s4); border: 0; border-radius: var(--r-control); overflow: hidden; background: var(--raised); accent-color: var(--accent); }
.voice-lab :global(progress::-webkit-progress-bar) { background: var(--raised); }
.voice-lab :global(progress::-webkit-progress-value) { background: var(--accent); }
.voice-lab :global(progress::-moz-progress-bar) { background: var(--accent); }
.voice-lab :global(.job-status) { display: flex; align-items: center; gap: var(--s3); }
.voice-lab :global(.job-status .failed) { background: var(--err); }
.voice-lab :global(.voice-screen > .check) { margin-bottom: var(--s4); }
.voice-lab :global(.actions button[aria-pressed=true]) { background: var(--raised); border-color: var(--t2); color: var(--t1); }
.voice-lab :global(.metrics) { font-family: var(--font-mono); font-size: var(--type-meta); line-height: var(--leading-body); font-variant-numeric: tabular-nums; color: var(--t2); }
.voice-lab :global(.muted) { max-width: 72ch; }
@media (min-width: 1100px) {
 .voice-lab :global(form), .voice-lab :global(fieldset) { grid-template-columns: repeat(2, minmax(0, 1fr)); }
}
</style>
