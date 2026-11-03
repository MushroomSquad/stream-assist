<script lang="ts">
import * as api from '$lib/ttsApi';
import { ru } from '$lib/i18n/ru';
let { runningPort, run, view }: { view: api.TtsView; runningPort: number; run: (f: () => Promise<unknown>) => Promise<void> } = $props();
let text = $state('');
let testing = $state(false);
async function test() {
  testing = true;
  try { await run(() => api.tts_test(text.trim())); } finally { testing = false; }
}
</script>
<section>
<p class="muted">{ru.ttsBusyNote}</p>
<div class="card">
<label>{ru.url}<input readonly value={`http://127.0.0.1:${runningPort}/widget/tts`} /></label>
<div class="actions"><button onclick={() => void run(() => navigator.clipboard.writeText(`http://127.0.0.1:${runningPort}/widget/tts`))}>{ru.copy}</button>
<button onclick={() => void run(api.obs_add_tts)}>{ru.addObs}</button></div>
</div>
<h3>{ru.queue}</h3>
<p class="muted">{ru.ttsLimits}</p>
{#if view.current}
<div class="card"><span class="tally live" aria-hidden="true"></span> <strong>{ru.now}</strong> · {view.current.user || ru.ttsTest} · {view.current.reward ? ru.ttsReward : ru.ttsChat}
{#if view.current.synthesizing}<span role="status"> · {ru.ttsSynthesizing}</span>{/if}
<p>{view.current.text}</p></div>
{/if}
{#if view.queue.length}<ol>{#each view.queue as item (item.id)}<li><div><strong>{item.user || ru.ttsTest}</strong> · {item.reward ? ru.ttsReward : ru.ttsChat}<p>{item.text}</p></div></li>{/each}</ol>
{:else if !view.current}<p class="muted">{ru.ttsEmpty}</p>{/if}
<div class="actions">
<button disabled={!view.current} onclick={() => void run(api.tts_skip)}>{ru.skip}</button>
<button disabled={!view.queue.length} onclick={() => void run(api.tts_clear)}>{ru.ttsClear}</button>
</div>
<form onsubmit={e => { e.preventDefault(); void test(); }}>
<label>{ru.ttsTestText}<textarea required rows="3" bind:value={text}></textarea></label>
<button disabled={testing || !text.trim() || [...text.trim()].length > 300}>{ru.ttsTest}</button>
</form>
</section>
<style>
section { padding-top: var(--s4); }
label { max-width: none; }
.card { background: var(--raised); border-radius: var(--r-row); }
</style>
