<script lang="ts">
	import type { ArchitectureLevel } from '../../api/types';
	import { EMPTY, architectureLabel, fromArchitecture } from '../../graph/elements';
	import { workspace } from '../../state/workspace.svelte';
	import EmptyState from '../EmptyState.svelte';
	import GraphCanvas from '../GraphCanvas.svelte';
	import Segmented from '../Segmented.svelte';

	let canvas = $state<GraphCanvas>();
	let selected = $state<string | null>(null);

	const graph = $derived(workspace.arch);
	const data = $derived(graph ? fromArchitecture(graph, workspace.archScope) : EMPTY);
	const node = $derived(graph?.nodes.find((n) => n.id === selected) ?? null);
	const cycleCount = $derived(data.nodes.filter((n) => n.classes.includes('cycle')).length);

	function setLevel(level: ArchitectureLevel) {
		selected = null;
		if (level === 'CRATE') workspace.archScope = null;
		void workspace.loadArchitecture(level);
	}

	function choose(id: string) {
		selected = id;
		if (id.startsWith('mod:')) void workspace.select(id);
	}
</script>

<div class="view">
	<div class="toolbar">
		<Segmented
			label="Level"
			value={workspace.archLevel}
			onchange={setLevel}
			options={[
				{ value: 'CRATE', label: 'Crates' },
				{ value: 'MODULE', label: 'Modules' },
				{ value: 'FILE', label: 'Files' }
			]}
		/>
		{#if workspace.archScope}
			<span class="scope">
				Within <code>{workspace.archScope}</code>
				<button class="btn small" onclick={() => (workspace.archScope = null)}>Show all</button>
			</span>
		{/if}
		<div class="right">
			<span class="legend"><span class="swatch cycle"></span>in a cycle ({cycleCount})</span>
			<span class="faint">{data.nodes.length} nodes · {data.edges.length} dependencies</span>
			<button class="btn small" onclick={() => canvas?.fit()} disabled={!graph}>Fit</button>
			<button class="btn small" onclick={() => canvas?.relayout()} disabled={!graph}
				>Re-layout</button
			>
		</div>
	</div>
	<div class="body">
		{#if !graph}
			<EmptyState title={workspace.busy ? 'Loading architecture…' : 'No architecture data'} />
		{:else if data.nodes.length === 0}
			<EmptyState title="No dependencies at this level">
				Nothing here depends on anything else at the {workspace.archLevel.toLowerCase()} level.
			</EmptyState>
		{:else}
			<GraphCanvas
				bind:this={canvas}
				{data}
				selectedId={selected}
				onselect={choose}
				onactivate={(id) => workspace.drillDown(id)}
				label="Architecture dependency graph"
			/>
			<p class="hint faint">
				Edges point from dependent to dependency; numbers count the underlying symbol references.
				{#if workspace.archLevel === 'CRATE'}Double-click a crate to see its modules.{/if}
				{#if workspace.archLevel === 'MODULE'}Double-click a module to explore its symbols.{/if}
			</p>
			{#if node}
				<div class="info">
					<p class="mono name">{architectureLabel(node.id)}</p>
					<p>{node.fanIn} dependents · {node.fanOut} dependencies</p>
					{#if node.inCycle}<p class="cycle-text">Part of a dependency cycle (see Cycles).</p>{/if}
					{#if workspace.archLevel !== 'FILE'}
						<button class="btn small" onclick={() => workspace.drillDown(node.id)}>
							{workspace.archLevel === 'CRATE' ? 'Show modules' : 'Explore symbols'}
						</button>
					{/if}
				</div>
			{/if}
		{/if}
	</div>
</div>

<style>
	.view {
		display: flex;
		flex-direction: column;
		height: 100%;
		min-height: 0;
	}
	.toolbar {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 6px 10px;
		border-bottom: 1px solid var(--border);
	}
	.scope {
		display: flex;
		align-items: center;
		gap: 6px;
		min-width: 0;
		white-space: nowrap;
	}
	.right {
		display: flex;
		align-items: center;
		gap: 10px;
		margin-left: auto;
		font-size: 12px;
		white-space: nowrap;
	}
	.legend {
		display: flex;
		align-items: center;
		gap: 5px;
		color: var(--text-muted);
	}
	.swatch {
		width: 12px;
		height: 10px;
		border: 2px solid var(--danger);
		border-radius: 2px;
	}
	.body {
		position: relative;
		flex: 1;
		min-height: 0;
	}
	.hint {
		position: absolute;
		left: 10px;
		bottom: 6px;
		margin: 0;
		font-size: 11px;
		pointer-events: none;
	}
	.info {
		position: absolute;
		right: 10px;
		top: 10px;
		max-width: 320px;
		padding: 8px 10px;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--bg);
		font-size: 12px;
	}
	.info p {
		margin: 0 0 4px;
	}
	.name {
		font-weight: 600;
		word-break: break-all;
	}
	.cycle-text {
		color: var(--danger);
	}
</style>
