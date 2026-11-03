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
<section><h2>{ru.voice}</h2>
{#if message}<p class="notice" role="alert">{message}</p><button onclick={load}>{ru.reload}</button>{/if}
{#if status}
{#if !status.installed}<Setup {status} installed={load} />
{:else}
{#if !status.ffmpeg}<p class="notice">{ru.vlWavOnly}</p>{/if}
<div class="actions">{#each tabs as id}<button class:primary={tab === id} onclick={() => tab = id}>{ru[id]}</button>{/each}</div>
{#if tab === 'vlSources'}<Sources />{:else if tab === 'vlVoices'}<Voices />{:else if tab === 'vlMixer'}<Mixer />{:else if tab === 'vlExplore'}<Explore />{:else if tab === 'vlTrain'}<Train />{:else}<Renders />{/if}
{/if}{/if}</section>
