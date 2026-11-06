<script lang="ts">
import { onMount } from 'svelte';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { check } from '@tauri-apps/plugin-updater';
import { openUrl } from '@tauri-apps/plugin-opener';
import { createUpdater, type UpdateState } from './updateController';
import { ru } from './i18n/ru';

let state = $state<UpdateState>({ offer: null, phase: 'idle', downloaded: 0, total: undefined, error: false, automatic: true });
let dialog: HTMLDialogElement;
const updater = createUpdater({
  check: () => check({ timeout: 12000 }),
  supported: () => invoke<boolean>('update_install_supported'),
  prepare: () => invoke('prepare_update'),
  recover: () => invoke('update_install_failed'),
  restart: () => invoke('restart_after_update'),
  openRelease: version => openUrl(`https://github.com/MushroomSquad/stream-assist/releases/tag/v${encodeURIComponent(version)}`)
}, next => { state = next; });
const busy = $derived(state.phase === 'downloading' || state.phase === 'installing');
const percent = $derived(state.total ? Math.min(100, Math.round(state.downloaded / state.total * 100)) : undefined);
$effect(() => {
  if (!dialog) return;
  if (state.offer && !dialog.open) dialog.showModal();
  else if (!state.offer && dialog.open) dialog.close();
});
onMount(() => {
  if (isTauri() && !import.meta.env.DEV) void updater.check();
  return () => updater.dispose();
});
function cancel(event: Event) { event.preventDefault(); updater.dismiss(); }
</script>

<dialog bind:this={dialog} oncancel={cancel} aria-labelledby="update-title" aria-describedby="update-description">
  <div class="update-content" aria-busy={busy}>
    <span class="eyebrow">STREAM ASSIST</span>
    <h2 id="update-title">{ru.updateTitle} {state.offer?.version}</h2>
    <p id="update-description" class="muted">{state.automatic ? ru.updateDescription : ru.updateManualDescription}</p>
    {#if state.offer?.body}<pre class="notes">{state.offer.body}</pre>{/if}
    <div role="status" aria-live="polite">
      {#if state.phase === 'downloading'}
        <p>{ru.updateDownloading}{percent === undefined ? '' : ` ${percent}%`}</p>
        <progress max="100" value={percent} aria-label={ru.updateDownloading}></progress>
      {:else if state.phase === 'installing'}<p>{ru.updateInstalling}</p>
      {:else if state.phase === 'restart'}<p>{ru.updateReady}</p>{/if}
    </div>
    {#if state.error}<p role="alert">{ru.updateFailed}</p>{/if}
    <div class="buttons">
      <button disabled={busy} onclick={() => updater.dismiss()}>{ru.updateSkip}</button>
      <button class="primary" disabled={busy} onclick={() => updater.update()}>
        {state.phase === 'restart' ? ru.updateRestart : state.error ? ru.updateRetry : ru.updateInstall}
      </button>
    </div>
  </div>
</dialog>

<style>
dialog { width: min(480px, calc(100vw - 32px)); max-height: calc(100vh - 48px); padding: 0; color: var(--t1); background: var(--card); border: 1px solid var(--line-strong); border-radius: var(--r-card); box-shadow: 0 24px 80px #0008; }
dialog::backdrop { background: #0009; }
.update-content { padding: var(--s6); display: flex; flex-direction: column; gap: var(--s4); }
.eyebrow { font-family: var(--font-mono); font-size: 11px; letter-spacing: .12em; color: var(--t3); }
h2 { margin: 0; }
.notes { margin: 0; max-height: 180px; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; font: inherit; font-size: var(--type-secondary); color: var(--t2); }
.buttons { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: var(--s2); }
</style>
