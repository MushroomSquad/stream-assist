<script lang="ts">
import { onMount } from 'svelte';
import * as api from '$lib/api';
import { ru, errorText } from '$lib/i18n/ru';
import NowQueue from '$lib/views/NowQueue.svelte';
import Rewards from '$lib/views/Rewards.svelte';
import Limits from '$lib/views/Limits.svelte';
import Widget from '$lib/views/Widget.svelte';
import VoiceLab from '$lib/views/voice/VoiceLab.svelte';
import Tts from '$lib/views/Tts.svelte';
import Connections from '$lib/views/Connections.svelte';
let tab = $state('now');
let snapshot = $state(api.emptyState);
let settings = $state<api.Settings>(structuredClone(api.defaultSettings));
let themes = $state<api.ThemeMeta[]>([]);
let ready = $state(false);
let message = $state('');
let runningPort = $state(47821);
const tabs = ['now', 'rewards', 'limits', 'widget', 'connections', 'voice', 'tts'] as const;
async function run(f: () => Promise<unknown>, clear = true) { if (clear) message = ''; try { await f(); } catch (e) { if (clear || !message) message = errorText(e); } }
async function refreshThemes() { themes = await api.list_themes(); }
async function load() { await run(async () => { settings = await api.get_settings(); runningPort = (await api.running_port()) ?? settings.port; await refreshThemes(); ready = true; }); }
function save() { void run(async () => { await api.save_settings($state.snapshot(settings)); message = ru.saved; if (settings.port !== runningPort) { message = ru.portWarning;  } }); }
onMount(() => {
  let disposed = false;
  let unlisten = () => {};
  void run(async () => { const cleanup = await api.onState(s => { snapshot = s; }); if (disposed) cleanup(); else unlisten = cleanup; snapshot = await api.get_state(); });
  void load();
  return () => { disposed = true; unlisten(); };
});
</script>
<svelte:head><title>{ru.appName}</title></svelte:head>
<main><header><h1>{ru.appName}</h1><p>{ru.subtitle}</p></header>
<nav aria-label={ru.appName}>{#each tabs as id}<button class:active={tab === id} aria-current={tab === id ? 'page' : undefined} onclick={() => tab = id}>{ru[id]}</button>{/each}</nav>
{#if snapshot.warning === 'server_port_busy'}<p class="notice" role="alert">{ru.server_port_busy.replace('N', String(runningPort))}</p>
{:else if snapshot.warning === 'server_start_failed'}<p class="notice" role="alert">{ru.server_start_failed}</p>{/if}
{#if message}<p class="notice" role="status">{message}</p>{/if}
{#if tab === 'now'}<NowQueue {snapshot} {run} />
{:else if tab === 'voice'}<VoiceLab />
{:else if !ready}<p>{ru.settingsLoading}</p><button onclick={load}>{ru.reload}</button>
{:else if tab === 'tts'}<Tts {settings} {runningPort} {save} {run} />
{:else if tab === 'rewards'}<Rewards {settings} {save} />
{:else if tab === 'limits'}<Limits {settings} {save} />
{:else if tab === 'widget'}<Widget {runningPort} {settings} {themes} {save} {run} {refreshThemes} />
{:else}<Connections {runningPort} {settings} {save} {run} />{/if}
</main>
<style>
:global(:root){font-family:system-ui,sans-serif;color:#24312e;background:#f5f6f5;font-synthesis:none;color-scheme:light dark;--surface:#fff;--border:#d8dfdc;--muted:#64716c;--accent:#246b55;--notice:#fff3d7}
:global(*){box-sizing:border-box}main{max-width:960px;margin:auto;padding:28px}header h1{font-size:25px;margin:0}header p{color:var(--muted);margin:6px 0 24px}nav{display:flex;gap:6px;border-bottom:1px solid var(--border);padding-bottom:12px;flex-wrap:wrap}nav button{background:transparent}nav .active{background:var(--accent);color:white;border-color:var(--accent)}:global(h2){font-size:22px;margin:26px 0 18px}:global(h3){font-size:17px}:global(button),:global(input),:global(select),:global(textarea){font:inherit;border:1px solid var(--border);border-radius:6px;background:var(--surface);color:inherit;padding:9px 12px}:global(button){cursor:pointer}:global(button:hover){border-color:var(--accent)}:global(button:disabled){opacity:.45;cursor:default}:global(button:focus-visible),:global(input:focus-visible),:global(select:focus-visible),:global(textarea:focus-visible){outline:2px solid var(--accent);outline-offset:2px}:global(.primary){background:var(--accent);color:white}:global(label){display:flex;flex-direction:column;gap:7px;margin:16px 0;max-width:560px}:global(.check){flex-direction:row;align-items:center}:global(input[type=checkbox]){width:18px;height:18px}:global(.card){background:var(--surface);border:1px solid var(--border);border-radius:9px;padding:20px;margin:16px 0}:global(.actions){display:flex;gap:8px;flex-wrap:wrap;margin:16px 0}:global(.muted){color:var(--muted);font-size:14px;line-height:1.6}:global(.notice){padding:14px 16px;background:var(--notice);border-radius:6px;line-height:1.5}:global(progress){width:100%;height:8px;accent-color:var(--accent)}:global(ol){padding:0;list-style:none}:global(li){display:flex;align-items:center;justify-content:space-between;gap:14px;padding:14px;background:var(--surface);border-bottom:1px solid var(--border)}:global(iframe){width:100%;height:280px;border:1px solid var(--border);border-radius:8px;margin-top:20px}:global(textarea){resize:vertical}
@media(prefers-color-scheme:dark){:global(:root){color:#e4eae6;background:#171d1b;--surface:#222b27;--border:#3d4b44;--muted:#a8b5ad;--accent:#357c63;--notice:#453c23}}@media(max-width:600px){main{padding:18px}nav{gap:4px}nav button{padding:8px}}
</style>
