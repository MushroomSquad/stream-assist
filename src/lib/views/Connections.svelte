<script lang="ts">
import { onMount } from 'svelte';
import * as api from '$lib/api';
import { ru } from '$lib/i18n/ru';
let { runningPort, settings = $bindable(), save, run }: { runningPort: number; settings: api.Settings; save: () => void; run: (f: () => Promise<unknown>, clear?: boolean) => void } = $props();
let device = $state<api.DeviceStart | null>(null);
let twitchConnected = $state<boolean | null>(null);
let spotify = $state<api.SpotifyStatus | null>(null);
let obs = $state('');
const redirect = $derived(`http://127.0.0.1:${runningPort}/callback/spotify`);
let poll: ReturnType<typeof setInterval> | undefined;
function stopPoll() { clearInterval(poll); poll = undefined; }
async function checkTwitch() { twitchConnected = await api.twitch_login_poll(); if (twitchConnected) { device = null; stopPoll(); } }
async function startTwitch() {
  stopPoll();
  device = await api.twitch_login_start();
  // ponytail: fixed client-side interval; the backend still enforces Twitch's own poll interval
  poll = setInterval(() => run(async () => { try { await checkTwitch(); } catch (e) { stopPoll(); throw e; } }, false), (Math.max(device.interval, 1) + 1) * 1000);
  await api.openExternal(device.verification_uri);
}
onMount(() => { run(async () => { twitchConnected = await api.twitch_status(); }, false); run(async () => { spotify = await api.spotify_status(); }, false); return stopPoll; });
</script>
<section><h2>{ru.connections}</h2><div class="services">
<div class="card"><header><div class="service-title"><span class="tally" class:connected={twitchConnected === true}></span><h3>{ru.twitch}</h3><span class="service-state">{twitchConnected === null ? ru.connectionUnchecked : twitchConnected ? ru.connected : ru.disconnected}</span></div>
<div class="actions">{#if twitchConnected === false}<button onclick={() => run(startTwitch)}>{ru.login}</button>{:else if twitchConnected}<button onclick={() => run(async () => { stopPoll(); await api.twitch_logout(); twitchConnected = false; device = null; })}>{ru.logout}</button>{/if}</div></header>
{#if device}<p>{ru.code}: <strong class="mono">{device.user_code}</strong></p><button class="verification" onclick={() => run(() => api.openExternal(device!.verification_uri))}>{device.verification_uri}</button><p>{ru.waiting}</p><button onclick={() => run(checkTwitch)}>{ru.checkLogin}</button>{/if}
</div>
<div class="card"><header><div class="service-title"><span class="tally" class:connected={spotify?.connected}></span><h3>{ru.spotify}</h3><span class="service-state">{spotify ? (spotify.connected ? ru.connected : ru.disconnected) : ru.connectionUnchecked}</span></div><button onclick={() => run(async () => { spotify = await api.spotify_status(); })}>{ru.status}</button></header><button onclick={() => run(() => api.openExternal('https://developer.spotify.com/dashboard'))}>{ru.dashboard}</button><p>{ru.createApp}</p><div class="redirect"><label>{ru.redirect}<input readonly value={redirect} /></label><button aria-label={`${ru.copy}: Redirect URI`} onclick={() => run(() => navigator.clipboard.writeText(redirect))}>{ru.copy}</button></div>
<label>{ru.clientId}<input bind:value={settings.spotify_client_id} /></label><div class="actions"><button onclick={() => run(async () => { await api.openExternal(await api.spotify_login(settings.spotify_client_id || '')); })}>{ru.spotifyLogin}</button></div>
{#if spotify}{#if spotify.connected && spotify.premium === false}<p class="notice">{ru.premium}</p>{/if}{/if}<p class="muted">{ru.autoplay}</p></div>
<div class="card"><header><div class="service-title"><span class="tally" class:connected={!!obs}></span><h3>{ru.obs}</h3><span class="service-state">{obs ? `${ru.connected} · ${obs}` : ru.connectionUnchecked}</span></div><div class="actions"><button onclick={save}>{ru.save}</button><button onclick={() => run(async () => { settings = await api.save_settings($state.snapshot(settings)); obs = ''; obs = await api.obs_check(); })}>{ru.check}</button></div></header><div class="obs-fields"><label>{ru.host}<input bind:value={settings.obs.host} /></label><label>{ru.obsPort}<input type="number" min="1" max="65535" bind:value={settings.obs.port} /></label><label>{ru.password}<input type="password" autocomplete="off" bind:value={settings.obs.password} /></label></div></div></div><div class="card youtube">
<label>{ru.youtubeKey}<input type="password" autocomplete="off" bind:value={settings.youtube_api_key} /></label><p class="muted">{ru.youtubeHelp}</p><button class="primary" onclick={save}>{ru.save}</button><p class="muted">{ru.keyringHelp}</p>
</div></section>

<style>
 h2 { margin:0 0 var(--s6); } h3 { margin:0; }
 .services { display:grid; gap:var(--s4); } .card { margin:0; min-width:0; }
 header:last-child { margin-bottom:0; }
 header { display:flex; align-items:center; justify-content:space-between; flex-wrap:wrap; gap:var(--s4); margin-bottom:var(--s5); }
 .service-title { display:flex; align-items:center; gap:var(--s3); flex-wrap:wrap; } .service-state { color:var(--t2); font-size:var(--type-secondary); overflow-wrap:anywhere; }
 .actions { margin:0; } label { max-width:none; } .redirect { display:flex; gap:var(--s2); align-items:flex-end; margin-block:var(--s4); } .redirect label { flex:1; min-width:0; margin:0; } .redirect input { width:100%; font-size:var(--type-meta); }
 .obs-fields { display:grid; grid-template-columns:minmax(0,2fr) minmax(0,1fr) minmax(0,2fr); gap:var(--s4); } .obs-fields label { margin:0; }
 .youtube { margin-top:var(--s8); } .youtube label { max-width:560px; } .youtube button { margin-block:var(--s4); }
 .verification { max-width:100%; height:auto; min-height:var(--control-height); overflow-wrap:anywhere; }
 @media(max-width:700px) { .obs-fields { grid-template-columns:1fr; } .service-title { column-gap:var(--s2); } .service-state { flex-basis:100%; padding-left:var(--s4); } .redirect { align-items:stretch; flex-direction:column; } }
</style>
