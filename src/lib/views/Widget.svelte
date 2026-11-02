<script lang="ts">
import * as api from '$lib/api';
import { ru } from '$lib/i18n/ru';
let { runningPort, settings, themes, save, run, refreshThemes }: { runningPort: number; settings: api.Settings; themes: api.ThemeMeta[]; save: () => void; run: (f: () => Promise<unknown>) => void; refreshThemes: () => Promise<void> } = $props();
const url = $derived(`http://127.0.0.1:${runningPort}/widget/music`);
</script>
<section><h2>{ru.widget}</h2>
<label>{ru.theme}<select bind:value={settings.active_theme}>{#each themes as theme}<option value={theme.id}>{theme.name}</option>{/each}</select></label>
<label>{ru.port}<input type="number" min="1" max="65535" bind:value={settings.port} /></label><button class="primary" onclick={save}>{ru.save}</button>
<label>{ru.url}<input readonly value={url} /></label>
<div class="actions"><button onclick={() => run(() => navigator.clipboard.writeText(url))}>{ru.copy}</button><button onclick={() => run(api.obs_add_widget)}>{ru.addObs}</button><button onclick={() => run(api.widget_test)}>{ru.test}</button><button onclick={() => run(async () => { const path = await api.chooseTheme(); if (typeof path === 'string') { settings.active_theme = await api.import_theme(path); await refreshThemes(); save(); } })}>{ru.import}</button></div>
<iframe title={ru.preview} src={`${url}?preview=1`}></iframe>
</section>
