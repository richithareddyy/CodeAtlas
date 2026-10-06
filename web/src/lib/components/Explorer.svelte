<script lang="ts">
	import { SUPPORT_CRATE_KINDS } from '../format';
	import { workspace } from '../state/workspace.svelte';
	import ExplorerNode from './ExplorerNode.svelte';

	const crates = $derived(workspace.crates.filter((c) => c.rootModule));
	/** Library and binary crates first; tests, benches and examples folded. */
	const product = $derived(crates.filter((c) => !SUPPORT_CRATE_KINDS.has(c.kind)));
	const support = $derived(crates.filter((c) => SUPPORT_CRATE_KINDS.has(c.kind)));
	let showSupport = $state(false);
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
				{#each product as c, i (c.id)}
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
			{#if support.length}
				<button
					class="group"
					aria-expanded={showSupport}
					onclick={() => (showSupport = !showSupport)}
				>
					<span class="caret">{showSupport ? '▾' : '▸'}</span>
					Tests, benches and examples ({support.length})
				</button>
				{#if showSupport}
					<ul role="tree" aria-label="Test, bench and example crates">
						{#each support as c (c.id)}
							<ExplorerNode
								id={c.rootModule ?? c.id}
								label={c.name}
								kind="MODULE"
								level={0}
								note={c.kind}
								initiallyOpen={false}
							/>
						{/each}
					</ul>
				{/if}
			{/if}
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
	.group {
		display: flex;
		align-items: center;
		gap: 6px;
		width: 100%;
		padding: 6px 10px;
		border: 0;
		border-top: 1px solid var(--border);
		background: none;
		color: var(--text-muted);
		font-size: 12px;
		text-align: left;
	}
	.group:hover {
		background: var(--bg-hover);
	}
	.caret {
		width: 10px;
		color: var(--text-faint);
	}
</style>
