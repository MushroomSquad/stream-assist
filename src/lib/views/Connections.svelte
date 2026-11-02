<script lang="ts">
import { onMount } from 'svelte';
import * as api from '$lib/api';
import { ru } from '$lib/i18n/ru';
let { runningPort, settings, save, run }: { runningPort: number; settings: api.Settings; save: () => void; run: (f: () => Promise<unknown>) => void } = $props();
let device = $state<api.DeviceStart | null>(null);
let twitchConnected = $state(false);
let spotify = $state<api.SpotifyStatus | null>(null);
let obs = $state('');
const redirect = $derived(`http://127.0.0.1:${runningPort}/callback/spotify`);
onMount(() => { run(async () => { twitchConnected = await api.twitch_status(); }); run(async () => { spotify = await api.spotify_status(); }); });
</script>
<section><h2>{ru.connections}</h2>
<div class="card"><h3>{ru.twitch}</h3>
{#if twitchConnected}<p>{ru.connected}</p>{/if}
<div class="actions"><button onclick={() => run(async () => { device = await api.twitch_login_start(); })}>{ru.login}</button><button onclick={() => run(async () => { await api.twitch_logout(); twitchConnected = false; device = null; })}>{ru.logout}</button></div>
{#if device}<p>{ru.code}: <strong>{device.user_code}</strong></p><button onclick={() => run(() => api.openExternal(device!.verification_uri))}>{device.verification_uri}</button><p>{ru.waiting}</p><button onclick={() => run(async () => { twitchConnected = await api.twitch_login_poll(); if (twitchConnected) device = null; })}>{ru.checkLogin}</button>{/if}
</div>
<div class="card"><h3>{ru.spotify}</h3><button onclick={() => run(() => api.openExternal('https://developer.spotify.com/dashboard'))}>{ru.dashboard}</button><p>{ru.createApp}</p><label>{ru.redirect}<input readonly value={redirect} /></label><button onclick={() => run(() => navigator.clipboard.writeText(redirect))}>{ru.copy}</button>
<label>{ru.clientId}<input bind:value={settings.spotify_client_id} /></label><div class="actions"><button onclick={() => run(async () => { await api.openExternal(await api.spotify_login(settings.spotify_client_id || '')); })}>{ru.spotifyLogin}</button><button onclick={() => run(async () => { spotify = await api.spotify_status(); })}>{ru.status}</button></div>
{#if spotify}<p>{spotify.connected ? ru.connected : ru.disconnected}</p>{#if spotify.connected && spotify.premium === false}<p class="notice">{ru.premium}</p>{/if}{/if}<p class="muted">{ru.autoplay}</p></div>
<div class="card"><h3>{ru.obs}</h3><label>{ru.host}<input bind:value={settings.obs.host} /></label><label>{ru.obsPort}<input type="number" min="1" max="65535" bind:value={settings.obs.port} /></label><label>{ru.password}<input type="password" autocomplete="off" bind:value={settings.obs.password} /></label><div class="actions"><button onclick={save}>{ru.save}</button><button onclick={() => run(async () => { await api.save_settings($state.snapshot(settings)); obs = await api.obs_check(); })}>{ru.check}</button></div>{#if obs}<p>{obs}</p>{/if}</div>
<label>{ru.youtubeKey}<input type="password" autocomplete="off" bind:value={settings.youtube_api_key} /></label><p class="muted">{ru.youtubeHelp}</p><button class="primary" onclick={save}>{ru.save}</button><p class="muted">{ru.keyringHelp}</p>
</section>
