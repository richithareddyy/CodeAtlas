<script lang="ts">
	import { workspace } from '../state/workspace.svelte';
</script>

<footer class="status" aria-live="polite">
	<span class="api">
		<span
			class="dot"
			class:ok={workspace.apiReachable === true}
			class:down={workspace.apiReachable === false}
		></span>
		{workspace.apiReachable === false
			? 'API unreachable'
			: workspace.apiReachable
				? 'API connected'
				: 'Connecting…'}
	</span>
	{#if workspace.busy}<span class="busy"><span class="spinner"></span> Loading</span>{/if}
	{#if workspace.repository}
		<span class="right mono">{workspace.repository.root}</span>
	{/if}
</footer>

<style>
	.status {
		display: flex;
		align-items: center;
		gap: 14px;
		height: 24px;
		padding: 0 10px;
		border-top: 1px solid var(--border);
		background: var(--bg-panel);
		color: var(--text-muted);
		font-size: 11.5px;
	}
	.api,
	.busy {
		display: flex;
		align-items: center;
		gap: 6px;
	}
	.dot {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: var(--text-faint);
	}
	.dot.ok {
		background: var(--success);
	}
	.dot.down {
		background: var(--danger);
	}
	.spinner {
		width: 10px;
		height: 10px;
	}
	.right {
		margin-left: auto;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: 11px;
	}
</style>
