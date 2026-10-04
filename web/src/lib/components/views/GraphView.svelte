<script lang="ts">
	import type { Relation, SymbolKind } from '../../api/types';
	import { EMPTY, fromModel } from '../../graph/elements';
	import { workspace, type GraphDirection } from '../../state/workspace.svelte';
	import EmptyState from '../EmptyState.svelte';
	import GraphCanvas from '../GraphCanvas.svelte';
	import Segmented from '../Segmented.svelte';

	const RELATIONS: { value: Relation; label: string }[] = [
		{ value: 'CALLS', label: 'Calls' },
		{ value: 'IMPLEMENTS', label: 'Implements' },
		{ value: 'IMPORTS', label: 'Imports' },
		{ value: 'CALLS_CANDIDATE', label: 'Ambiguous' }
	];
	const KINDS: { value: SymbolKind; label: string }[] = [
		{ value: 'FUNCTION', label: 'fn' },
		{ value: 'METHOD', label: 'method' },
		{ value: 'STRUCT', label: 'struct' },
		{ value: 'ENUM', label: 'enum' },
		{ value: 'TRAIT', label: 'trait' },
		{ value: 'MODULE', label: 'mod' }
	];

	let canvas = $state<GraphCanvas>();

	const data = $derived.by(() => {
		void workspace.modelVersion;
		const model = workspace.model;
		return model
			? fromModel(model, new Set(workspace.hiddenKinds), new Set(workspace.relations))
			: EMPTY;
	});

	function setDirection(direction: GraphDirection) {
		workspace.direction = direction;
		void workspace.refreshGraph();
	}

	function toggleRelation(relation: Relation) {
		const on = workspace.relations.includes(relation);
		if (on && workspace.relations.length === 1) return;
		workspace.relations = on
			? workspace.relations.filter((r) => r !== relation)
			: [...workspace.relations, relation];
		void workspace.refreshGraph();
	}

	function toggleKind(kind: SymbolKind) {
		workspace.hiddenKinds = workspace.hiddenKinds.includes(kind)
			? workspace.hiddenKinds.filter((k) => k !== kind)
			: [...workspace.hiddenKinds, kind];
	}
</script>

<div class="view">
	<div class="toolbar">
		<Segmented
			label="Direction"
			value={workspace.direction}
			onchange={setDirection}
			options={[
				{ value: 'DEPENDENTS', label: 'Callers', title: 'What depends on the symbol' },
				{ value: 'DEPENDENCIES', label: 'Callees', title: 'What the symbol depends on' },
				{ value: 'BOTH', label: 'Both' }
			]}
		/>
		<label class="field">
			<span class="label">Depth</span>
			<select
				class="input"
				value={workspace.depth}
				onchange={(e) => {
					workspace.depth = Number(e.currentTarget.value);
					void workspace.refreshGraph();
				}}
			>
				{#each [1, 2, 3, 4, 5] as d (d)}<option value={d}>{d}</option>{/each}
			</select>
		</label>
		<div class="chips" role="group" aria-label="Relationships">
			{#each RELATIONS as r (r.value)}
				<button
					class="chip rel-{r.value.toLowerCase()}"
					aria-pressed={workspace.relations.includes(r.value)}
					onclick={() => toggleRelation(r.value)}>{r.label}</button
				>
			{/each}
		</div>
		<div class="chips" role="group" aria-label="Symbol kinds shown">
			{#each KINDS as k (k.value)}
				<button
					class="chip"
					aria-pressed={!workspace.hiddenKinds.includes(k.value)}
					onclick={() => toggleKind(k.value)}>{k.label}</button
				>
			{/each}
		</div>
		<div class="right">
			{#if workspace.model}
				<span class="faint">{data.nodes.length} nodes · {data.edges.length} edges</span>
			{/if}
			<button class="btn small" onclick={() => canvas?.fit()} disabled={!workspace.model}
				>Fit</button
			>
			<button
				class="btn small"
				onclick={() => workspace.selected && canvas?.center(workspace.selected.id)}
				disabled={!workspace.selected}>Center</button
			>
			<button class="btn small" onclick={() => canvas?.relayout()} disabled={!workspace.model}
				>Re-layout</button
			>
		</div>
	</div>

	{#if workspace.model?.truncated}
		<div class="banner warning">
			The traversal hit the server's node limit; some dependents are not shown. Reduce the depth or
			expand nodes individually.
		</div>
	{/if}

	<div class="body">
		{#if workspace.model}
			<GraphCanvas
				bind:this={canvas}
				{data}
				selectedId={workspace.selected?.id ?? null}
				onselect={(id) => workspace.select(id)}
				onactivate={(id) => workspace.toggleExpand(id)}
				label="Symbol dependency graph"
			/>
			<p class="hint faint">
				Click a node to inspect it. Double-click to expand its {workspace.direction ===
				'DEPENDENCIES'
					? 'callees'
					: workspace.direction === 'BOTH'
						? 'neighbours'
						: 'callers'}, double-click again to collapse.
			</p>
			{#if data.nodes.length === 1}
				<div class="overlay">
					<EmptyState title="Nothing found in this direction">
						No resolved {workspace.relations
							.map((r) => r.toLowerCase().replace('_', ' '))
							.join(' or ')}
						relationships for this symbol at depth {workspace.depth}. Try another direction, more
						relationship types, or the Impact view.
					</EmptyState>
				</div>
			{/if}
		{:else}
			<EmptyState title="Pick a symbol to explore">
				Search with <kbd>⌘K</kbd> or choose a function, method or type in the explorer. Its callers and
				callees appear here, with the source lines behind every edge in the inspector.
			</EmptyState>
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
		flex-wrap: wrap;
		align-items: center;
		gap: 10px;
		padding: 6px 10px;
		border-bottom: 1px solid var(--border);
	}
	.field {
		display: flex;
		align-items: center;
		gap: 6px;
	}
	.field .input {
		height: 24px;
	}
	.chips {
		display: flex;
		gap: 3px;
	}
	.chip {
		height: 22px;
		padding: 0 7px;
		border: 1px solid var(--border);
		border-radius: 11px;
		background: var(--bg);
		color: var(--text-faint);
		font-size: 11.5px;
	}
	.chip[aria-pressed='true'] {
		border-color: var(--border-strong);
		background: var(--bg-subtle);
		color: var(--text);
	}
	.right {
		display: flex;
		align-items: center;
		gap: 6px;
		margin-left: auto;
		font-size: 12px;
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
	.overlay {
		position: absolute;
		left: 50%;
		bottom: 40px;
		transform: translateX(-50%);
		width: min(460px, 90%);
		height: auto;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--bg);
	}
	kbd {
		font-family: var(--font-mono);
		font-size: 11px;
	}
</style>
