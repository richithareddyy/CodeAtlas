<script lang="ts">
	import type { Snippet } from 'svelte';
	let { title, children }: { title: string; children?: Snippet } = $props();
</script>

<div class="empty">
	<p class="title">{title}</p>
	{#if children}<div class="body">{@render children()}</div>{/if}
</div>

<style>
	.empty {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		height: 100%;
		padding: 24px;
		text-align: center;
		color: var(--text-muted);
	}
	.title {
		margin: 0 0 6px;
		color: var(--text);
		font-weight: 600;
	}
	.body {
		max-width: 420px;
		font-size: 12px;
	}
</style>
