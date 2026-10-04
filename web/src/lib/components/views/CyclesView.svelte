<script lang="ts">
	import type { Cycle, CycleLevel } from '../../api/types';
	import { EMPTY, architectureLabel, fromCycle } from '../../graph/elements';
	import { kindOf, shortLabel } from '../../format';
	import { workspace } from '../../state/workspace.svelte';
	import EmptyState from '../EmptyState.svelte';
	import GraphCanvas from '../GraphCanvas.svelte';
	import Segmented from '../Segmented.svelte';

	const cycles = $derived(workspace.cycles);
	const cycle = $derived(cycles?.[workspace.cycleIndex] ?? null);
	const data = $derived(cycle ? fromCycle(cycle) : EMPTY);

	function label(id: string): string {
		return kindOf(id) && workspace.cycleLevel === 'FUNCTION'
			? shortLabel(id)
			: architectureLabel(id);
	}

	/** Members in hop order, returning to the first one. */
	function path(c: Cycle): string[] {
		const order = c.hops.length ? c.hops.map((h) => h.from) : c.members;
		return [...order, order[0]];
	}

	/** Last path segment, for the compact cycle list. */
	function compact(id: string): string {
		if (kindOf(id)) return shortLabel(id);
		const full = architectureLabel(id);
		return full.split(/::|\//).pop() ?? full;
	}

	function selectMember(id: string) {
		if (kindOf(id)) void workspace.select(id);
	}
</script>

<div class="view">
	<div class="toolbar">
		<Segmented
			label="Level"
			value={workspace.cycleLevel}
			onchange={(level: CycleLevel) => workspace.loadCycles(level)}
			options={[
				{ value: 'MODULE', label: 'Modules' },
				{ value: 'FILE', label: 'Files' },
				{ value: 'FUNCTION', label: 'Functions', title: 'Recursion and mutual recursion' }
			]}
		/>
		{#if cycles}<span class="faint">{cycles.length} cycles</span>{/if}
	</div>
	{#if !cycles}
		<EmptyState title={workspace.busy ? 'Finding cycles…' : 'No data'} />
	{:else if cycles.length === 0}
		<EmptyState title="No circular dependencies">
			Nothing at the {workspace.cycleLevel.toLowerCase()} level depends on itself, directly or indirectly.
		</EmptyState>
	{:else}
		<div class="split">
			<ul class="list" aria-label="Cycles">
				{#each cycles as c, i (i)}
					<li>
						<button
							class:active={i === workspace.cycleIndex}
							onclick={() => (workspace.cycleIndex = i)}
						>
							<span class="count">{c.members.length}</span>
							<span class="members mono" title={path(c).map(label).join(' → ')}
								>{path(c).map(compact).join(' → ')}</span
							>
						</button>
					</li>
				{/each}
			</ul>
			{#if cycle}
				<div class="detail">
					<div class="graph">
						<GraphCanvas {data} onselect={selectMember} label="Cycle graph" />
					</div>
					<div class="hops">
						<p class="faint intro">
							A shortest cycle through these {cycle.members.length} members. Each hop lists the source
							lines that create it.
						</p>
						<ol>
							{#each cycle.hops as hop, i (i)}
								<li>
									<div class="hop mono">
										<button class="sym" onclick={() => selectMember(hop.from)}
											>{label(hop.from)}</button
										>
										<span class="arrow">→</span>
										<button class="sym" onclick={() => selectMember(hop.to)}>{label(hop.to)}</button
										>
										<span class="faint"
											>{hop.weight} reference{hop.weight === 1 ? '' : 's'} · {hop.via
												.join(', ')
												.toLowerCase()}</span
										>
									</div>
									<ul class="evidence">
										{#each hop.evidence as e, j (j)}
											<li>
												<span class="relation">{e.relation.toLowerCase()}</span>
												<button class="sym mono" onclick={() => workspace.select(e.from)}
													>{shortLabel(e.from)}</button
												>
												<span class="faint">→</span>
												<button class="sym mono" onclick={() => workspace.select(e.to)}
													>{shortLabel(e.to)}</button
												>
												<button class="loc mono" onclick={() => workspace.select(e.from, e.line)}
													>{e.file}:{e.line}</button
												>
											</li>
										{/each}
										{#if hop.weight > hop.evidence.length}
											<li class="faint">and {hop.weight - hop.evidence.length} more</li>
										{/if}
									</ul>
								</li>
							{/each}
						</ol>
					</div>
				</div>
			{/if}
		</div>
	{/if}
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
	.split {
		flex: 1;
		min-height: 0;
		display: grid;
		grid-template-columns: 280px 1fr;
	}
	.list {
		margin: 0;
		padding: 0;
		overflow: auto;
		list-style: none;
		border-right: 1px solid var(--border);
	}
	.list button {
		display: flex;
		gap: 8px;
		width: 100%;
		padding: 7px 10px;
		border: 0;
		border-bottom: 1px solid var(--border);
		background: none;
		text-align: left;
	}
	.list button:hover {
		background: var(--bg-hover);
	}
	.list button.active {
		background: var(--accent-soft);
	}
	.count {
		flex: 0 0 22px;
		color: var(--danger);
		font-weight: 600;
		text-align: right;
	}
	.members {
		overflow: hidden;
		display: -webkit-box;
		-webkit-line-clamp: 2;
		line-clamp: 2;
		-webkit-box-orient: vertical;
		font-size: 11.5px;
		overflow-wrap: anywhere;
	}
	.detail {
		display: grid;
		grid-template-rows: minmax(180px, 50%) 1fr;
		min-height: 0;
	}
	.graph {
		position: relative;
		min-height: 0;
		border-bottom: 1px solid var(--border);
	}
	.hops {
		overflow: auto;
		padding: 6px 12px 16px;
	}
	.intro {
		margin: 4px 0 8px;
		font-size: 12px;
	}
	ol {
		margin: 0;
		padding-left: 18px;
	}
	ol > li {
		margin-bottom: 8px;
	}
	.hop {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		align-items: baseline;
	}
	.arrow {
		color: var(--danger);
	}
	.evidence {
		margin: 2px 0 0;
		padding-left: 12px;
		list-style: none;
		border-left: 1px solid var(--border);
		font-size: 12px;
	}
	.evidence li {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		align-items: baseline;
		line-height: 20px;
	}
	.relation {
		color: var(--text-muted);
		min-width: 64px;
	}
	button.sym,
	button.loc {
		padding: 0;
		border: 0;
		background: none;
		text-align: left;
	}
	button.sym:hover,
	button.loc:hover {
		text-decoration: underline;
	}
	.loc {
		color: var(--accent);
	}
</style>
