<script lang="ts">
import '../app.css';
import type { Snippet } from 'svelte';
import UpdateDialog from '$lib/UpdateDialog.svelte';
let { children }: { children: Snippet } = $props();
</script>

{@render children()}
<UpdateDialog />
