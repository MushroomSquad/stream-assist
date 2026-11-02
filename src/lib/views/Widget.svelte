<script lang="ts">
import * as api from '$lib/api';
import { ru } from '$lib/i18n/ru';
let { runningPort, settings, themes, save, run, refreshThemes }: { runningPort: number; settings: api.Settings; themes: api.ThemeMeta[]; save: () => void; run: (f: () => Promise<unknown>) => void; refreshThemes: () => Promise<void> } = $props();
let scale = $state<number | undefined>(1);
const effectiveScale = $derived(Number.isFinite(scale) ? Math.min(4, Math.max(0.5, scale!)) : 1);
const url = $derived(`http://127.0.0.1:${runningPort}/widget/music`);
const theme = $derived(themes.find(t => t.id === settings.active_theme));
const tw = $derived(theme?.width ?? 800);
const th = $derived(theme?.height ?? 420);
let boxW = $state(0);
const k = $derived(boxW ? Math.min(1, boxW / tw) : 1);
</script>
<section><h2>{ru.widget}</h2>
<label>{ru.theme}<select bind:value={settings.active_theme}>{#each themes as theme}<option value={theme.id}>{theme.name}</option>{/each}</select></label>
<label>{ru.port}<input type="number" min="1" max="65535" bind:value={settings.port} /></label><button class="primary" onclick={save}>{ru.save}</button>
<label>{ru.scale}<input type="number" min="0.5" max="4" step="0.25" bind:value={scale} /></label>
{#each ['now', 'queue'] as part}
<label>{part === 'now' ? ru.nowPlaying : ru.queue}<input readonly value={`${url}?part=${part}&scale=${effectiveScale}`} /></label>
<button onclick={() => run(() => navigator.clipboard.writeText(`${url}?part=${part}&scale=${effectiveScale}`))}>{ru.copy}</button>
{/each}
<p>{ru.widgetPartsHint}</p>
<div class="actions"><button onclick={() => run(() => api.obs_add_widget(effectiveScale))}>{ru.addObs}</button><button onclick={() => run(api.widget_test)}>{ru.test}</button><button onclick={() => run(async () => { const path = await api.chooseTheme(); if (typeof path === 'string') { settings.active_theme = await api.import_theme(path); await refreshThemes(); save(); } })}>{ru.import}</button></div>
<div class="preview" bind:clientWidth={boxW} style:height={`${th * k}px`}><iframe title={ru.preview} src={`${url}?preview=1`} style:width={`${tw}px`} style:height={`${th}px`} style:transform={`scale(${k})`}></iframe></div>
</section>
<style>.preview{background:#202020;border-radius:8px;overflow:hidden;margin-top:16px}.preview iframe{display:block;border:0;border-radius:0;margin:0;max-width:none;transform-origin:0 0}</style>
