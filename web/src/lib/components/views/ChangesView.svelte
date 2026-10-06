<script lang="ts">
	import type { ChangeKind, DownstreamSymbol, SymbolChange } from '../../api/types';
	import { EMPTY, fromDiff } from '../../graph/elements';
	import { count, shortSha } from '../../format';
	import { workspace } from '../../state/workspace.svelte';
	import EmptyState from '../EmptyState.svelte';
	import EvidencePath from '../EvidencePath.svelte';
	import GraphCanvas from '../GraphCanvas.svelte';
	import KindBadge from '../KindBadge.svelte';
	import Segmented from '../Segmented.svelte';

	const LIST_LIMIT = 300;
	/** Above this many downstream symbols, the canvas shows direct dependents only. */
	const CANVAS_LIMIT = 60;
	const CHANGE_LABEL: Record<ChangeKind, string> = {
		MODIFIED: 'modified',
		ADDED: 'added',
		REMOVED: 'removed',
		MOVED: 'moved'
	};

	let tab = $state<'symbols' | 'downstream' | 'tests' | 'files'>('symbols');
	let canvas = $state<GraphCanvas>();
	/** Shown when a symbol of the diff is not in the stored index. */
	let notice = $state<string | null>(null);

	const report = $derived(workspace.diff);
	const canvasDepth = $derived(
		report && report.downstream.length > CANVAS_LIMIT ? 1 : Number.POSITIVE_INFINITY
	);
	const data = $derived(report ? fromDiff(report, canvasDepth) : EMPTY);
	const drawn = $derived(report?.downstream.filter((d) => d.depth <= canvasDepth).length ?? 0);
	const certain = $derived(report?.downstream.filter((d) => d.confidence === 'CERTAIN') ?? []);
	const possible = $derived(report?.downstream.filter((d) => d.confidence === 'POSSIBLE') ?? []);
	const tests = $derived(certain.filter((d) => d.symbol.isTest));
	const refs = $derived(workspace.gitRefs);
	const indexed = $derived(workspace.repository);
	/** The stored graph and source views show another revision than the head. */
	const otherRevision = $derived(
		report && indexed && (report.head.sha === null || report.head.sha !== indexed.indexedSha)
	);

	async function choose(id: string, line: number | null = null) {
		const found = await workspace.select(id, line, true);
		notice = found
			? null
			: `${id.slice(id.indexOf(':') + 1)} is not in the stored index (indexed at ${shortSha(indexed?.indexedSha ?? null)}). Index the head revision to inspect it.`;
	}

	function compare(event: SubmitEvent) {
		event.preventDefault();
		notice = null;
		void workspace.runDiff();
	}

	function changeLabel(c: SymbolChange): string {
		return c.change === 'MODIFIED' && c.signature ? 'signature' : CHANGE_LABEL[c.change];
	}

	function lineText(c: SymbolChange): string {
		const ranges = c.lines.map((l) => (l.start === l.end ? `${l.start}` : `${l.start}-${l.end}`));
		return `${c.symbol.file}:${ranges.length ? ranges.join(',') : c.symbol.line}`;
	}
</script>

{#snippet downstreamList(items: DownstreamSymbol[])}
	<ul class="list">
		{#each items.slice(0, LIST_LIMIT) as d (d.symbol.id)}
			<li class:selected={workspace.selected?.id === d.symbol.id}>
				<div class="head">
					<span class="depth mono" title="Depth">{d.depth}</span>
					<KindBadge kind={d.symbol.kind} test={d.symbol.isTest} />
					<button class="sym mono" onclick={() => choose(d.symbol.id)}
						>{d.symbol.qualifiedName}</button
					>
					{#if d.confidence === 'POSSIBLE'}<span class="possible">possible</span>{/if}
					{#if d.revision === 'BASE'}
						<span class="via-base" title="Reached through code the diff removed"
							>via removed code</span
						>
					{/if}
					<span class="loc mono">{d.symbol.file}:{d.symbol.line}</span>
				</div>
				<EvidencePath
					steps={d.path}
					onselect={choose}
					otherRevision={d.revision === 'BASE' ? `the base revision (${report?.base.label})` : null}
				/>
			</li>
		{/each}
	</ul>
	{#if items.length > LIST_LIMIT}
		<p class="faint more">{items.length - LIST_LIMIT} more not listed.</p>
	{/if}
{/snippet}

<div class="view">
	<form class="toolbar" onsubmit={compare}>
		<label class="field">
			<span class="label">Base</span>
			<input
				class="input mono rev"
				list="git-revisions"
				bind:value={workspace.diffBase}
				placeholder="main"
				spellcheck="false"
				required
			/>
		</label>
		<span class="arrow" aria-hidden="true">→</span>
		<label class="field">
			<span class="label">Head</span>
			<input
				class="input mono rev"
				list="git-revisions"
				bind:value={workspace.diffHead}
				placeholder="working tree"
				spellcheck="false"
			/>
		</label>
		<datalist id="git-revisions">
			{#each refs?.refs ?? [] as r (r.kind + r.name)}
				<option value={r.name}>{r.kind === 'TAG' ? 'tag' : 'branch'} · {r.subject}</option>
			{/each}
			{#each refs?.commits.slice(0, 30) ?? [] as c (c.sha)}
				<option value={shortSha(c.sha)}>{c.subject}</option>
			{/each}
		</datalist>
		<button
			class="btn primary small"
			type="submit"
			disabled={!workspace.diffBase || !!workspace.gitUnavailable}>Compare</button
		>
		<label class="field">
			<span class="label">Max depth</span>
			<select class="input" bind:value={workspace.impactDepth}>
				{#each [1, 2, 3, 4, 6, 8, 10] as d (d)}<option value={d}>{d}</option>{/each}
			</select>
		</label>
		<label class="field">
			<input type="checkbox" bind:checked={workspace.includeAmbiguous} />
			<span>Ambiguous calls</span>
		</label>
		<div class="right">
			{#if report}
				<span class="faint mono resolved" title="Commits compared, and analysis time">
					{shortSha(report.base.sha)} → {report.head.sha
						? shortSha(report.head.sha)
						: 'working tree'}
					· {(report.analysisMs / 1000).toFixed(1)} s
				</span>
			{/if}
			<button class="btn small" type="button" onclick={() => canvas?.fit()} disabled={!report}
				>Fit</button
			>
		</div>
	</form>

	{#if workspace.gitUnavailable}
		<EmptyState title="Git history is not available">
			{workspace.gitUnavailable} Comparing revisions needs the repository's Git history on the server.
		</EmptyState>
	{:else if !report}
		<EmptyState title={workspace.busy ? 'Comparing…' : 'Compare two revisions'}>
			Choose a base and a head (a branch, tag, commit or expression such as <code>HEAD~1</code>;
			leave the head empty for uncommitted changes). CodeAtlas analyses both, lists what changed
			symbol by symbol, and follows the changes to everything that depends on them.
		</EmptyState>
	{:else}
		{@const s = report.summary}
		<div class="summary">
			<div class="metric">
				<span class="value">{s.filesChanged}</span><span class="label">files changed</span>
				<span class="faint"
					>{s.filesAdded} added · {s.filesRemoved} removed · {s.filesModified} modified{s.filesRenamed
						? ` · ${s.filesRenamed} renamed`
						: ''}</span
				>
			</div>
			<div class="metric">
				<span class="value">{s.functionsModified}</span><span class="label">functions modified</span
				>
				<span class="faint">{s.functionsAdded} added · {s.functionsRemoved} removed</span>
			</div>
			<div class="metric">
				<span class="value" class:attention={s.signaturesChanged > 0}>{s.signaturesChanged}</span
				><span class="label">signatures</span>
				<span class="faint">changed</span>
			</div>
			<div class="metric">
				<span class="value">{s.downstreamSymbols}</span><span class="label">downstream</span>
				<span class="faint">in {count(s.affectedModules, 'module')}</span>
			</div>
			<div class="metric">
				<span class="value">{s.affectedTests}</span><span class="label">tests to run</span>
				{#if s.untestedChanges}
					<button class="untested-hint" onclick={() => (tab = 'tests')}
						>{s.untestedChanges} changed {s.untestedChanges === 1 ? 'function' : 'functions'} untested</button
					>
				{/if}
			</div>
			{#if report.includeAmbiguous}
				<div class="metric">
					<span class="value possible-value">{s.possibleSymbols}</span><span class="label"
						>possible</span
					>
				</div>
			{/if}
		</div>
		{#if report.truncated}
			<div class="banner warning">
				The traversal stopped at the node limit; the report is partial.
			</div>
		{/if}
		{#if otherRevision}
			<div class="banner note">
				The inspector and source show the indexed revision ({indexed?.branch ?? 'detached'} @ {shortSha(
					indexed?.indexedSha ?? null
				)}), which may differ from the head compared here.
			</div>
		{/if}
		{#if notice}
			<div class="banner note">{notice}</div>
		{/if}

		<div class="split">
			<div class="graph">
				{#if data.nodes.length}
					<GraphCanvas
						bind:this={canvas}
						{data}
						selectedId={workspace.selected?.id ?? null}
						onselect={(id) => choose(id)}
						label="Change impact graph"
					/>
					{#if drawn < report.downstream.length}
						<p class="canvas-note faint">
							Showing direct dependents only ({drawn} of {report.downstream.length} downstream symbols);
							the Downstream tab lists all of them.
						</p>
					{/if}
				{:else if report.symbols.length}
					<EmptyState title="No modified or removed symbols">
						The diff only adds or moves code, or changes whitespace and comments, so nothing
						existing depends on it.
					</EmptyState>
				{:else}
					<EmptyState title="No changes to Rust symbols">
						{report.files.length
							? 'The changed files contain no Rust symbol changes.'
							: 'The two revisions are identical.'}
					</EmptyState>
				{/if}
			</div>
			<div class="details">
				<div class="tabs">
					<Segmented
						label="Change details"
						value={tab}
						onchange={(v) => (tab = v)}
						options={[
							{ value: 'symbols', label: `Changed symbols (${report.symbols.length})` },
							{ value: 'downstream', label: `Downstream (${report.downstream.length})` },
							{ value: 'tests', label: `Tests (${tests.length})` },
							{ value: 'files', label: `Files (${report.files.length})` }
						]}
					/>
				</div>
				<div class="panel">
					{#if tab === 'symbols'}
						<ul class="list">
							{#each report.symbols.slice(0, LIST_LIMIT) as c (c.change + c.symbol.id)}
								<li class:selected={workspace.selected?.id === c.symbol.id}>
									<div class="head">
										<span class="change change-{changeLabel(c)}">{changeLabel(c)}</span>
										<KindBadge kind={c.symbol.kind} test={c.symbol.isTest} />
										{#if c.change === 'REMOVED'}
											<span class="sym mono gone">{c.symbol.qualifiedName}</span>
										{:else}
											<button class="sym mono" onclick={() => choose(c.symbol.id)}
												>{c.symbol.qualifiedName}</button
											>
										{/if}
										{#if c.change === 'MOVED' && c.previous}
											<span class="faint mono">from {c.previous.qualifiedName}</span>
										{/if}
										{#if c.change === 'REMOVED'}
											<span class="loc mono" title="In the base revision">{lineText(c)} (base)</span
											>
										{:else}
											<button
												class="loc mono"
												title="Show the changed lines"
												onclick={() => choose(c.symbol.id, c.lines[0]?.start ?? null)}
												>{lineText(c)}</button
											>
										{/if}
									</div>
									{#if c.signature}
										<pre class="signature mono"><span class="before">- {c.signature.before}</span>
<span class="after">+ {c.signature.after}</span></pre>
									{/if}
								</li>
							{/each}
						</ul>
						{#if report.cosmetic.length}
							<h4 class="label">Whitespace or comment changes only</h4>
							<ul class="list compact">
								{#each report.cosmetic as c (c.id)}
									<li>
										<div class="head">
											<KindBadge kind={c.kind} test={c.isTest} />
											<button class="sym mono" onclick={() => choose(c.id)}
												>{c.qualifiedName}</button
											>
											<span class="loc mono">{c.file}:{c.line}</span>
										</div>
									</li>
								{/each}
							</ul>
						{/if}
					{:else if tab === 'downstream'}
						{#if report.downstream.length}
							<p class="faint intro">
								Symbols the diff did not change that depend on modified or removed ones, with the
								chain back to the change.
							</p>
							{@render downstreamList(certain)}
							{#if possible.length}
								<h4 class="label">Possibly affected (through ambiguous calls)</h4>
								{@render downstreamList(possible)}
							{/if}
						{:else}
							<p class="faint intro">
								Nothing outside the diff depends on the changed symbols within depth {report.maxDepth}.
							</p>
						{/if}
					{:else if tab === 'tests'}
						{#if tests.length}
							<p class="faint intro">Unchanged tests whose call chain reaches a change:</p>
							{@render downstreamList(tests)}
						{:else}
							<p class="faint intro">
								No unchanged test reaches the changes through resolved calls.
							</p>
						{/if}
						{#if report.untested.length}
							<h4 class="label">Modified, but no test reaches it</h4>
							<p class="faint intro">
								No resolved call chain leads from a test to these functions. Tests may still run
								them through code static analysis does not follow (another process, a function
								passed as a value, a macro, a generic parameter).
							</p>
							<ul class="list compact">
								{#each report.untested as u (u.id)}
									<li>
										<div class="head">
											<KindBadge kind={u.kind} test={u.isTest} />
											<button class="sym mono" onclick={() => choose(u.id)}
												>{u.qualifiedName}</button
											>
											<span class="loc mono">{u.file}:{u.line}</span>
										</div>
									</li>
								{/each}
							</ul>
						{/if}
					{:else}
						<table>
							<thead><tr><th>Status</th><th>File</th><th>Hunks</th></tr></thead>
							<tbody>
								{#each report.files as f (f.path)}
									<tr>
										<td class="status status-{f.status.toLowerCase()}">{f.status.toLowerCase()}</td>
										<td class="mono"
											>{#if f.oldPath}{f.oldPath} →
											{/if}{f.path}{#if !f.rust}<span class="faint"> · not analysed</span>{/if}</td
										>
										<td>{f.rust && f.status === 'MODIFIED' ? f.hunks.length : ''}</td>
									</tr>
								{/each}
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
		flex-wrap: wrap;
		align-items: center;
		gap: 6px 12px;
		padding: 6px 10px;
		border-bottom: 1px solid var(--border);
		white-space: nowrap;
	}
	.field {
		display: flex;
		align-items: center;
		gap: 6px;
	}
	.field .input {
		height: 24px;
	}
	.rev {
		width: 130px;
	}
	.arrow {
		color: var(--text-faint);
	}
	.right {
		display: flex;
		align-items: center;
		gap: 10px;
		margin-left: auto;
	}
	.resolved {
		font-size: 11px;
	}
	.summary {
		display: flex;
		flex-wrap: wrap;
		border-bottom: 1px solid var(--border);
	}
	.metric {
		display: flex;
		flex-direction: column;
		padding: 8px 14px;
		border-right: 1px solid var(--border);
	}
	.metric .value {
		font-size: 18px;
		font-weight: 600;
		font-variant-numeric: tabular-nums;
	}
	.metric .faint,
	.metric .label {
		font-size: 11px;
		white-space: nowrap;
	}
	.attention {
		color: var(--warning);
	}
	.untested-hint {
		padding: 0;
		border: 0;
		background: none;
		color: var(--warning);
		font-size: 11px;
		text-align: left;
		white-space: nowrap;
	}
	.untested-hint:hover {
		text-decoration: underline;
	}
	.possible-value {
		color: var(--edge-candidate);
	}
	.banner.note {
		background: var(--bg-subtle);
		color: var(--text-muted);
	}
	.split {
		flex: 1;
		min-height: 0;
		display: grid;
		grid-template-rows: minmax(160px, 42%) 1fr;
	}
	.canvas-note {
		position: absolute;
		left: 10px;
		bottom: 6px;
		margin: 0;
		font-size: 11px;
		pointer-events: none;
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
	.list {
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.list li {
		padding: 6px 0;
		border-bottom: 1px solid var(--border);
	}
	.list.compact li {
		padding: 3px 0;
	}
	.list li.selected {
		background: var(--accent-soft);
	}
	.head {
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.depth {
		width: 18px;
		color: var(--text-faint);
		text-align: right;
	}
	.change {
		width: 62px;
		font-size: 11px;
		text-align: right;
		color: var(--text-muted);
	}
	.change-signature {
		color: var(--warning);
		font-weight: 600;
	}
	.change-removed {
		color: var(--danger);
	}
	.change-added {
		color: var(--success);
	}
	.sym {
		padding: 0;
		border: 0;
		background: none;
		text-align: left;
		font-weight: 600;
		word-break: break-all;
	}
	button.sym:hover {
		text-decoration: underline;
	}
	.sym.gone {
		color: var(--text-muted);
		text-decoration: line-through;
	}
	.possible {
		color: var(--edge-candidate);
		font-size: 11px;
	}
	.via-base {
		color: var(--text-faint);
		font-size: 11px;
	}
	.loc {
		margin-left: auto;
		padding: 0;
		border: 0;
		background: none;
		color: var(--text-faint);
		white-space: nowrap;
	}
	button.loc {
		color: var(--accent);
	}
	button.loc:hover {
		text-decoration: underline;
	}
	.signature {
		margin: 4px 0 0 70px;
		padding: 4px 8px;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--bg-subtle);
		white-space: pre-wrap;
		word-break: break-all;
	}
	.before {
		color: var(--danger);
	}
	.after {
		color: var(--success);
	}
	.more {
		font-size: 12px;
	}
	h4 {
		margin: 14px 0 4px;
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
	}
	th {
		color: var(--text-muted);
		font-weight: 500;
	}
	.status {
		width: 70px;
		color: var(--text-muted);
	}
	.status-added {
		color: var(--success);
	}
	.status-removed {
		color: var(--danger);
	}
</style>
