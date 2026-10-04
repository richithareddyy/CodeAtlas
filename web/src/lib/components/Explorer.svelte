<script lang="ts">
	import { workspace } from '../state/workspace.svelte';
	import ExplorerNode from './ExplorerNode.svelte';

	const crates = $derived(workspace.crates.filter((c) => c.rootModule));
</script>

<nav class="explorer" aria-label="Repository explorer">
	<div class="heading">
		<span class="label">Explorer</span>
		{#if workspace.repository}<span class="faint">{crates.length} crates</span>{/if}
	</div>
	{#if !workspace.repository}
		<p class="hint muted">No repository indexed yet.</p>
	{:else if crates.length === 0}
		<p class="hint muted">This repository has no analysed crates.</p>
	{:else}
		{#key workspace.repoId}
			<ul role="tree" aria-label="Crates and modules">
				{#each crates as c, i (c.id)}
					<ExplorerNode
						id={c.rootModule ?? c.id}
						label={c.name}
						kind="MODULE"
						level={0}
						note={c.kind === 'lib' ? undefined : c.kind}
						initiallyOpen={i === 0}
					/>
				{/each}
			</ul>
		{/key}
	{/if}
</nav>

<style>
	.explorer {
		height: 100%;
		overflow: auto;
		border-right: 1px solid var(--border);
		background: var(--bg-panel);
	}
	.heading {
		position: sticky;
		top: 0;
		display: flex;
		justify-content: space-between;
		align-items: center;
		height: 30px;
		padding: 0 10px;
		border-bottom: 1px solid var(--border);
		background: var(--bg-panel);
		z-index: 1;
	}
	ul {
		margin: 0;
		padding: 4px 0;
		list-style: none;
	}
	.hint {
		padding: 10px;
		font-size: 12px;
	}
</style>
