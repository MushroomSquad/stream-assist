<script lang="ts">
import { onMount } from 'svelte';
import * as api from '$lib/voiceApi';
import { openExternal } from '$lib/api';
import { ru, errorText } from '$lib/i18n/ru';
let { status, installed }: { status: api.ModelStatus; installed: () => Promise<void> } = $props();
let withEncoder = $state(true), busy = $state(false), message = $state('');
let progress = $state({ done: 0, total: 1 });
let listening = $state(false);
onMount(() => {
  let disposed = false, stop = () => {};
  void api.onInstall(p => progress = p).then(fn => { if (disposed) fn(); else { stop = fn; listening = true; } }).catch(e => message = errorText(e));
  return () => { disposed = true; stop(); };
});
async function install() { busy = true; message = ''; try { await api.vl_install(withEncoder); await installed(); } catch (e) { message = errorText(e); } finally { busy = false; } }
</script>
<div class="voice-screen">
<div class="card"><h3>{ru.vlSetup}</h3><p>{ru.vlSize}</p>
<button onclick={() => { void openExternal('https://huggingface.co/Supertone/supertonic-3/blob/3cadd1ee6394adea1bd021217a0e650ede09a323/LICENSE').catch(e => message = errorText(e)); }}>{ru.vlLicense}</button>
<p class="muted">{ru.vlLicenseNotice}</p>
<label class="check"><input type="checkbox" bind:checked={withEncoder} disabled={busy} />{ru.vlEncoder}</label>
{#if !status.ffmpeg}<p class="notice">{ru.vlWavOnly}</p>{/if}
{#if busy}<progress aria-label={ru.vlDownload} value={progress.done} max={progress.total}></progress><p class="muted">{Math.round(progress.done / progress.total * 100)}%</p>{/if}
{#if message}<p class="notice" role="alert">{message}</p>{/if}
<button class="primary" disabled={busy || !listening} onclick={install}>{ru.vlDownload}</button></div>

</div>
