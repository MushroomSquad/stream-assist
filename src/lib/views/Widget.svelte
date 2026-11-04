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
<div class="widget-layout"><div class="card settings">
<label>{ru.theme}<select bind:value={settings.active_theme}>{#each themes as theme}<option value={theme.id}>{theme.name}</option>{/each}</select></label>
<label>{ru.port}<input type="number" min="1" max="65535" bind:value={settings.port} /></label><button class="primary" onclick={save}>{ru.save}</button>
<div class="widget-tools"><label>{ru.scale}<input type="number" min="0.5" max="4" step="0.25" bind:value={scale} /></label>
<div class="actions"><button onclick={() => run(() => api.obs_add_widget(effectiveScale))}>{ru.addObs}</button><button onclick={() => run(api.widget_test)}>{ru.test}</button><button onclick={() => run(async () => { const path = await api.chooseTheme(); if (typeof path === 'string') { settings.active_theme = await api.import_theme(path); await refreshThemes(); save(); } })}>{ru.import}</button></div></div>
</div><div class="output"><h3>{ru.preview}</h3>
<div class="preview" bind:clientWidth={boxW} style:height={`${th * k}px`}><iframe title={ru.preview} src={`${url}?preview=1`} style:width={`${tw}px`} style:height={`${th}px`} style:transform={`scale(${k})`}></iframe></div>

<div class="sources">
{#each ['now', 'queue'] as part}
<div class="source"><label>{part === 'now' ? ru.nowPlaying : ru.queue}<input readonly value={`${url}?part=${part}&scale=${effectiveScale}`} /></label><button aria-label={`${ru.copy}: ${part === 'now' ? ru.nowPlaying : ru.queue}`} onclick={() => run(() => navigator.clipboard.writeText(`${url}?part=${part}&scale=${effectiveScale}`))}>{ru.copy}</button></div>
{/each}
<p class="muted">{ru.widgetPartsHint}</p>
</div></div></div></section>
<style>
 h2 { margin:0 0 var(--s6); } h3 { margin:0 0 var(--s4); }
 .widget-layout { display:grid; grid-template-columns:minmax(240px, 1fr) minmax(0,2fr); gap:var(--s6); align-items:start; }
 .settings { margin:0; } label { margin:0 0 var(--s4); max-width:none; } .settings .actions { margin-bottom:0; }
 .widget-tools { border-top:1px solid var(--line); margin-top:var(--s6); padding-top:var(--s5); } .output { min-width:0; }
 .preview:focus-within { outline:2px solid var(--accent); outline-offset:2px; }
 .preview { background:#202020; border-radius:var(--r-row); overflow:hidden; }
 .preview iframe { display:block; border:0; border-radius:0; margin:0; max-width:none; transform-origin:0 0; }
 .sources { margin-top:var(--s6); } .source { display:flex; gap:var(--s2); align-items:flex-end; margin-bottom:var(--s4); } .source label { flex:1; min-width:0; margin:0; } .source input { width:100%; font-size:var(--type-meta); }
 @media(max-width:1050px) { .widget-layout { grid-template-columns:1fr; } }
</style>
