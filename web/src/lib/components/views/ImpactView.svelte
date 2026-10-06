<script lang="ts">
	import type { AffectedSymbol } from '../../api/types';
	import { EMPTY, fromImpact } from '../../graph/elements';
	import { callSteps, count } from '../../format';
	import { workspace } from '../../state/workspace.svelte';
	import EmptyState from '../EmptyState.svelte';
	import EvidencePath from '../EvidencePath.svelte';
	import GraphCanvas from '../GraphCanvas.svelte';
	import KindBadge from '../KindBadge.svelte';
	import Segmented from '../Segmented.svelte';

	const LIST_LIMIT = 300;
	let tab = $state<'affected' | 'tests' | 'files' | 'score'>('affected');
	let canvas = $state<GraphCanvas>();

	const report = $derived(workspace.impact);
	const data = $derived(report ? fromImpact(report) : EMPTY);
	const tests = $derived(report?.affected.filter((a) => a.symbol.isTest) ?? []);
	const certainTests = $derived(tests.filter((t) => t.confidence === 'CERTAIN'));
	const directTests = $derived(certainTests.filter((t) => callSteps(t.path) <= 1));
	const transitiveTests = $derived(certainTests.filter((t) => callSteps(t.path) > 1));
	const possibleTests = $derived(tests.filter((t) => t.confidence === 'POSSIBLE'));

	function rerun() {
		void workspace.runImpact();
	}
</script>

{#snippet affectedList(items: AffectedSymbol[])}
	<ul class="affected">
		{#each items.slice(0, LIST_LIMIT) as a (a.symbol.id)}
			<li class:selected={workspace.selected?.id === a.symbol.id}>
				<div class="head">
					<span class="depth mono" title="Depth">{a.depth}</span>
					<KindBadge kind={a.symbol.kind} test={a.symbol.isTest} />
					<button class="sym mono" onclick={() => workspace.select(a.symbol.id)}
						>{a.symbol.qualifiedName}</button
					>
					{#if a.confidence === 'POSSIBLE'}<span class="possible">possible</span>{/if}
					<span class="loc mono">{a.symbol.file}:{a.symbol.line}</span>
				</div>
				<EvidencePath steps={a.path} />
			</li>
		{/each}
	</ul>
	{#if items.length > LIST_LIMIT}
		<p class="faint more">{items.length - LIST_LIMIT} more not listed.</p>
	{/if}
{/snippet}

<div class="view">
	<div class="toolbar">
		{#if workspace.selected}
			<span class="subject mono" title={workspace.selected.id}>
				Changing <strong>{workspace.selected.qualifiedName}</strong>
			</span>
		{/if}
		<label class="field">
			<span class="label">Max depth</span>
			<select
				class="input"
				value={workspace.impactDepth}
				onchange={(e) => {
					workspace.impactDepth = Number(e.currentTarget.value);
					rerun();
				}}
			>
				{#each [1, 2, 3, 4, 6, 8, 10] as d (d)}<option value={d}>{d}</option>{/each}
			</select>
		</label>
		<label class="field">
			<input
				type="checkbox"
				checked={workspace.includeAmbiguous}
				onchange={(e) => {
					workspace.includeAmbiguous = e.currentTarget.checked;
					rerun();
				}}
			/>
			<span>Include ambiguous calls</span>
		</label>
		<div class="right">
			<button class="btn small" onclick={() => canvas?.fit()} disabled={!report}>Fit</button>
		</div>
	</div>

	{#if !report}
		<EmptyState title="No impact analysis yet">
			Select a symbol and choose <strong>Impact</strong> in the inspector to see everything a change to
			it could affect, and why.
		</EmptyState>
	{:else}
		<div class="summary">
			<div class="metric">
				<span class="value">{report.directCount + report.indirectCount}</span>
				<span class="label">affected</span>
				<span class="faint">{report.directCount} direct · {report.indirectCount} indirect</span>
			</div>
			<div class="metric">
				<span class="value">{report.files.length}</span><span class="label">files</span>
			</div>
			<div class="metric">
				<span class="value">{report.modules.length}</span><span class="label">modules</span>
			</div>
			<div class="metric">
				<span class="value">{report.tests.length}</span><span class="label">tests</span>
			</div>
			{#if report.includeAmbiguous}
				<div class="metric">
					<span class="value possible-value">{report.possibleCount}</span><span class="label"
						>possible</span
					>
				</div>
			{/if}
			<button
				class="metric score level-{report.score.level.toLowerCase()}"
				onclick={() => (tab = 'score')}
				title="Show how the score is computed"
			>
				<span class="value">{report.score.total.toFixed(1)}</span>
				<span class="label">blast radius · {report.score.level.toLowerCase()}</span>
			</button>
		</div>
		{#if report.truncated}
			<div class="banner warning">
				The traversal stopped at the node limit; the report is partial.
			</div>
		{/if}

		<div class="split">
			<div class="graph">
				{#if report.affected.length}
					<GraphCanvas
						bind:this={canvas}
						{data}
						selectedId={workspace.selected?.id ?? null}
						onselect={(id) => workspace.select(id)}
						label="Impact graph"
					/>
				{:else}
					<EmptyState title="Nothing depends on this symbol">
						No resolved calls, dispatch or implementations reach it within depth {report.maxDepth}.
						{#if !report.includeAmbiguous}Ambiguous calls are not included; enable them to see
							possible dependents.{/if}
					</EmptyState>
				{/if}
			</div>
			<div class="details">
				<div class="tabs">
					<Segmented
						label="Impact details"
						value={tab}
						onchange={(v) => (tab = v)}
						options={[
							{ value: 'affected', label: `Affected (${report.affected.length})` },
							{ value: 'tests', label: `Tests (${tests.length})` },
							{ value: 'files', label: `Files (${report.files.length})` },
							{ value: 'score', label: 'Score' }
						]}
					/>
				</div>
				<div class="panel">
					{#if tab === 'affected'}
						{@render affectedList(report.affected)}
					{:else if tab === 'tests'}
						{#if tests.length}
							<h4 class="label">Direct ({directTests.length})</h4>
							<p class="faint intro">The test calls the changed code.</p>
							{@render affectedList(directTests)}
							<h4 class="label">Transitive ({transitiveTests.length})</h4>
							<p class="faint intro">The test reaches the changed code through other code.</p>
							{@render affectedList(transitiveTests)}
							{#if possibleTests.length}
								<h4 class="label">Possible ({possibleTests.length})</h4>
								<p class="faint intro">Reached only through ambiguous calls.</p>
								{@render affectedList(possibleTests)}
							{/if}
						{:else}
							<p class="faint intro">
								No test reaches this symbol through resolved calls. Tests may still run it through
								code static analysis does not follow (another process, a function passed as a value,
								a macro, a generic parameter).
							</p>
						{/if}
					{:else if tab === 'files'}
						<table>
							<thead><tr><th>File</th><th>Symbols</th><th>Tests</th></tr></thead>
							<tbody>
								{#each report.files as f (f.name)}
									<tr><td class="mono">{f.name}</td><td>{f.symbols}</td><td>{f.tests}</td></tr>
								{/each}
							</tbody>
						</table>
						<h4 class="label">Modules</h4>
						<table>
							<tbody>
								{#each report.modules as m (m.name)}
									<tr
										><td class="mono">{m.name.replace(/^mod:/, '')}</td><td
											>{count(m.symbols, 'symbol')}</td
										></tr
									>
								{/each}
							</tbody>
						</table>
					{:else}
						<p class="faint intro">
							The score sums six factors. Each is <code>ln(1 + value) / ln(1 + saturation)</code>,
							capped at 1, times its weight. It measures how much of the graph the change reaches,
							not how likely it is to break something.
						</p>
						<table>
							<thead>
								<tr
									><th>Factor</th><th>Value</th><th>Saturation</th><th>Weight</th><th
										>Contribution</th
									></tr
								>
							</thead>
							<tbody>
								{#each report.score.factors as f (f.name)}
									<tr>
										<td class="mono">{f.name}</td>
										<td>{f.value}</td>
										<td>{f.saturation}</td>
										<td>{f.weight.toFixed(2)}</td>
										<td>
											<span class="bar"><span style:width="{f.normalized * 100}%"></span></span>
											{f.contribution.toFixed(1)}
										</td>
									</tr>
								{/each}
								<tr class="total"
									><td>Total</td><td></td><td></td><td></td><td>{report.score.total.toFixed(1)}</td
									></tr
								>
							</tbody>
						</table>
					{/if}
				</div>
			</div>
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
		gap: 14px;
		padding: 6px 10px;
		border-bottom: 1px solid var(--border);
	}
	.subject {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		max-width: 50%;
	}
	.field {
		display: flex;
		align-items: center;
		gap: 6px;
	}
	.field .input {
		height: 24px;
	}
	.right {
		margin-left: auto;
	}
	.summary {
		display: flex;
		gap: 0;
		border-bottom: 1px solid var(--border);
	}
	.metric {
		display: flex;
		flex-direction: column;
		padding: 8px 16px;
		border: 0;
		border-right: 1px solid var(--border);
		background: none;
		text-align: left;
	}
	.metric .value {
		font-size: 18px;
		font-weight: 600;
		font-variant-numeric: tabular-nums;
	}
	.metric .faint {
		font-size: 11px;
	}
	.score {
		margin-left: auto;
		border-left: 1px solid var(--border);
		border-right: 0;
	}
	.score:hover {
		background: var(--bg-hover);
	}
	.level-high .value {
		color: var(--danger);
	}
	.level-medium .value {
		color: var(--warning);
	}
	.possible-value {
		color: var(--edge-candidate);
	}
	.split {
		flex: 1;
		min-height: 0;
		display: grid;
		grid-template-rows: minmax(160px, 45%) 1fr;
	}
	.graph {
		position: relative;
		min-height: 0;
		border-bottom: 1px solid var(--border);
	}
	.details {
		display: flex;
		flex-direction: column;
		min-height: 0;
	}
	.tabs {
		padding: 6px 10px;
		border-bottom: 1px solid var(--border);
	}
	.panel {
		flex: 1;
		overflow: auto;
		padding: 4px 10px 16px;
	}
	.intro {
		margin: 6px 0;
		font-size: 12px;
	}
	.affected {
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.affected li {
		padding: 6px 0;
		border-bottom: 1px solid var(--border);
	}
	.affected li.selected {
		background: var(--accent-soft);
	}
	.head {
		display: flex;
		align-items: center;
		gap: 8px;
		margin-bottom: 2px;
	}
	.depth {
		width: 18px;
		color: var(--text-faint);
		text-align: right;
	}
	.sym {
		padding: 0;
		border: 0;
		background: none;
		text-align: left;
		font-weight: 600;
		word-break: break-all;
	}
	.sym:hover {
		text-decoration: underline;
	}
	.possible {
		color: var(--edge-candidate);
		font-size: 11px;
	}
	.loc {
		margin-left: auto;
		color: var(--text-faint);
		white-space: nowrap;
	}
	.more {
		font-size: 12px;
	}
	table {
		width: 100%;
		border-collapse: collapse;
		font-size: 12px;
	}
	th,
	td {
		padding: 4px 8px 4px 0;
		border-bottom: 1px solid var(--border);
		text-align: left;
		font-variant-numeric: tabular-nums;
	}
	th {
		color: var(--text-muted);
		font-weight: 500;
	}
	.total td {
		font-weight: 600;
		border-bottom: 0;
	}
	h4 {
		margin: 14px 0 4px;
	}
	.bar {
		display: inline-block;
		width: 60px;
		height: 6px;
		margin-right: 6px;
		border-radius: 3px;
		background: var(--bg-hover);
		vertical-align: middle;
		overflow: hidden;
	}
	.bar span {
		display: block;
		height: 100%;
		background: var(--accent);
	}
</style>
