<script lang="ts">
import Icon from '$lib/Icon.svelte';
import { onMount } from 'svelte';
import * as api from '$lib/api';
import { ru, errorText } from '$lib/i18n/ru';
import NowQueue from '$lib/views/NowQueue.svelte';
import Rules from '$lib/views/rules/Rules.svelte';
import Widget from '$lib/views/Widget.svelte';
import VoiceLab from '$lib/views/voice/VoiceLab.svelte';
import Connections from '$lib/views/Connections.svelte';
let tab = $state('now');
let snapshot = $state(api.emptyState);
let settings = $state<api.Settings>(structuredClone(api.defaultSettings));
let themes = $state<api.ThemeMeta[]>([]);
let ready = $state(false);
let message = $state('');
let runningPort = $state(47821);
const tabs = ['now', 'rules', 'widget', 'connections', 'voice'] as const;
const serviceIds = ['twitch', 'spotify', 'obs'] as const;
let services = $state({ twitch: false, spotify: false, obs: false });
const activeRules = $derived([settings.rules.music, settings.rules.tts].some(action => action.enabled && action.triggers.some(trigger => trigger.enabled)));
let updateServices: () => Promise<void> = async () => {};
function selectTab(next: string) {
  const leavingConnections = tab === 'connections' && next !== tab;
  tab = next;
  if (leavingConnections) void updateServices();
}
function serviceStatus(service: typeof serviceIds[number]) {
  return service === 'twitch' && services.twitch && ready && activeRules
    ? ru.live : services[service] ? ru.connected : ru.disconnected;
}
async function refreshServices() {
  const results = await Promise.allSettled([api.twitch_status(), api.spotify_status(), api.obs_check()]);
  return {
    twitch: results[0].status === 'fulfilled' && results[0].value,
    spotify: results[1].status === 'fulfilled' && results[1].value.connected,
    obs: results[2].status === 'fulfilled' && Boolean(results[2].value)
  };
}
async function run(f: () => Promise<unknown>, clear = true) { if (clear) message = ''; try { await f(); } catch (e) { if (clear || !message) message = errorText(e); } }
async function refreshThemes() { themes = await api.list_themes(); }
async function load() { await run(async () => { settings = await api.get_settings(); runningPort = (await api.running_port()) ?? settings.port; await refreshThemes(); ready = true; }); }
function save() { void run(async () => { settings = await api.save_settings($state.snapshot(settings)); message = ru.saved; if (settings.port !== runningPort) { message = ru.portWarning;  } }); }
onMount(() => {
  let disposed = false;
  let unlisten = () => {};
  void run(async () => { const cleanup = await api.onState(s => { snapshot = s; }); if (disposed) cleanup(); else unlisten = cleanup; snapshot = await api.get_state(); });
  void load();
  let polling = false;
  updateServices = async () => {
    if (disposed || polling) return;
    polling = true;
    try {
      const next = await refreshServices();
      if (!disposed) services = next;
    } finally {
      polling = false;
    }
  };
  void updateServices();
  const servicePoll = setInterval(() => { void updateServices(); }, 30_000);
  return () => { disposed = true; unlisten(); clearInterval(servicePoll); };
});
</script>
<svelte:head><title>{ru.appName}</title></svelte:head>
<div class="shell">
<aside class="sidebar">
  <div class="brand" title={ru.appName}><span class="brand-mark" aria-hidden="true"></span><span class="nav-label">{ru.appName}</span></div>
  <nav aria-label={ru.navigation}>
    {#each tabs as id}<button class:active={tab === id} aria-current={tab === id ? 'page' : undefined} aria-label={id === 'voice' ? ru.voiceLab : ru[id]} title={id === 'voice' ? ru.voiceLab : ru[id]} onclick={() => selectTab(id)}><Icon name={id} /><span class="nav-label">{id === 'voice' ? ru.voiceLab : ru[id]}</span></button>{/each}
  </nav>
  <div class="services" role="group" aria-label={ru.serviceStatus}>
    {#each serviceIds as service}
      <button onclick={() => selectTab('connections')} aria-label={`${ru[service]}: ${serviceStatus(service)}`} title={`${ru[service]}: ${serviceStatus(service)}`}>
        <span class="tally" class:connected={services[service]} class:live={service === 'twitch' && services.twitch && ready && activeRules}></span><span class="nav-label">{ru[service]}</span>
      </button>
    {/each}
  </div>
</aside>
<main>
{#if tab === 'rules'}<h1>{ru.rules}</h1>{/if}
{#if snapshot.warning === 'server_port_busy'}<p class="notice" role="alert">{ru.server_port_busy.replace('N', String(runningPort))}</p>
{:else if snapshot.warning === 'server_start_failed'}<p class="notice" role="alert">{ru.server_start_failed}</p>{/if}
{#if message}<p class="notice" role="status">{message}</p>{/if}
{#if tab === 'now'}<NowQueue {snapshot} {run} />
{:else if tab === 'voice'}<VoiceLab />
{:else if !ready}<p>{ru.settingsLoading}</p><button onclick={load}>{ru.reload}</button>
{:else if tab === 'rules'}
  <Rules {settings} {runningPort} {save} {run} />
{:else if tab === 'widget'}<Widget {runningPort} {settings} {themes} {save} {run} {refreshThemes} />
{:else}<Connections {runningPort} bind:settings {save} {run} />{/if}
</main>
</div>
<style>
.shell { display: grid; grid-template-columns: var(--sidebar) minmax(0, 1fr); min-height: 100dvh; }
.sidebar { position: sticky; top: 0; height: 100dvh; display: flex; flex-direction: column; padding: var(--s4) var(--s3); background: var(--bg-deep); border-right: 1px solid var(--line); }
.brand { display: flex; align-items: center; gap: var(--s2); padding: var(--s2); margin-bottom: var(--s6); font-size: var(--type-card); font-weight: 600; white-space: nowrap; }
.brand-mark { width: 14px; height: 14px; border: 1.5px solid var(--t2); border-radius: var(--r-round); display: grid; place-items: center; flex-shrink: 0; }
.brand-mark::after { content: ''; width: var(--s1); height: var(--s1); border-radius: var(--r-round); background: var(--t2); }
nav { display: flex; flex-direction: column; gap: var(--nav-gap); }
nav button, .services button { display: flex; align-items: center; gap: var(--s2); width: 100%; padding: 0 var(--s2); min-height: 0; height: var(--nav-height); border: 0; background: transparent; color: var(--t2); text-align: left; }
nav button:hover, .services button:hover { background: var(--raised); color: var(--t1); }
nav button.active { background: var(--raised); color: var(--t1); box-shadow: var(--active-highlight); }
.services { margin-top: auto; padding-bottom: var(--s3); padding-top: var(--s6); border-top: 1px solid var(--line); }
.services button { height: var(--status-height); font-size: var(--type-secondary); }
main { width: 100%; max-width: var(--content-max); min-width: 0; padding: var(--s6) var(--s8); }
main > :global(section:first-of-type > h2:first-child) { margin-top: 0; }
@media (max-width: 899.98px) {
  .shell { grid-template-columns: var(--sidebar-compact) minmax(0, 1fr); }
  .sidebar { padding-inline: var(--s2); }
  .nav-label { display: none; }
  .brand, nav button, .services button { justify-content: center; padding-inline: 0; }
  main { padding-inline: var(--s4); }
}
</style>
