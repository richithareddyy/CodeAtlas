<script lang="ts">
	import * as api from '../api/queries';
	import type { SymbolRef } from '../api/types';
	import { count, shortSha, timestamp } from '../format';
	import { workspace } from '../state/workspace.svelte';
	import KindBadge from './KindBadge.svelte';
	import SourceView from './SourceView.svelte';

	interface Counts {
		id: string;
		callers: number;
		callees: number;
		tests: SymbolRef[];
	}

	let counts = $state<Counts | null>(null);
	const symbol = $derived(workspace.selected);
	const repository = $derived(workspace.repository);

	$effect(() => {
		const current = symbol;
		const repo = workspace.repoId;
		counts = null;
		if (!current || !repo) return;
		const callable = current.kind === 'FUNCTION' || current.kind === 'METHOD';
		Promise.all([
			callable ? api.neighborhood(repo, current.id, 'DEPENDENTS', 1, ['CALLS']) : null,
			callable ? api.neighborhood(repo, current.id, 'DEPENDENCIES', 1, ['CALLS']) : null,
			api.affectedTests(repo, current.id)
		])
			.then(([callers, callees, tests]) => {
				if (workspace.selected?.id !== current.id) return;
				counts = {
					id: current.id,
					callers: callers?.nodes.length ?? 0,
					callees: callees?.nodes.length ?? 0,
					tests
				};
			})
			.catch(() => {
				// Counts are secondary; the main error banner reports API failures.
			});
	});

	const editorLink = $derived(
		symbol && repository
			? `vscode://file/${repository.root}/${symbol.file}:${workspace.focusLine ?? symbol.startLine}`
			: null
	);
</script>

<aside class="inspector" aria-label="Inspector">
	{#if symbol}
		<header>
			<div class="title">
				<KindBadge kind={symbol.kind} test={symbol.isTest} />
				<h2>{symbol.name}</h2>
			</div>
			<p class="qualified mono">{symbol.qualifiedName}</p>
		</header>

		<section class="actions">
			<button class="btn small" onclick={() => workspace.focus(symbol.id, { view: 'graph' })}>
				Graph
			</button>
			<button class="btn small" onclick={() => workspace.focus(symbol.id, { view: 'impact' })}>
				Impact
			</button>
			{#if editorLink}
				<a class="btn small" href={editorLink} title="Open in VS Code">Open in editor</a>
			{/if}
		</section>

		<dl class="facts">
			<dt>Location</dt>
			<dd class="mono">{symbol.file}:{symbol.startLine}–{symbol.endLine}</dd>
			<dt>Crate</dt>
			<dd class="mono">{symbol.crateName}</dd>
			<dt>Visibility</dt>
			<dd class="mono">{symbol.visibility}</dd>
			{#if counts && counts.id === symbol.id}
				{#if symbol.kind === 'FUNCTION' || symbol.kind === 'METHOD'}
					<dt>Callers</dt>
					<dd>{counts.callers} direct</dd>
					<dt>Callees</dt>
					<dd>{counts.callees} resolved</dd>
				{/if}
				<dt>Tests</dt>
				<dd>
					{count(counts.tests.length, 'test')}
					{counts.tests.length === 1 ? 'reaches' : 'reach'} this symbol
				</dd>
			{/if}
		</dl>

		{#if symbol.signature}
			<section>
				<h3 class="label">Signature</h3>
				<pre class="signature mono">{symbol.signature}</pre>
			</section>
		{/if}

		{#if symbol.unresolvedCalls.length}
			<section>
				<h3 class="label">Unresolved calls ({symbol.unresolvedCalls.length})</h3>
				<ul class="unresolved">
					{#each symbol.unresolvedCalls as call, i (i)}
						<li>
							<button class="link mono" onclick={() => workspace.select(symbol.id, call.line)}>
								line {call.line}
							</button>
							<code>{call.callee}</code>
							<span class="faint">{call.reason.replaceAll('_', ' ')}</span>
						</li>
					{/each}
				</ul>
			</section>
		{/if}

		{#if counts && counts.tests.length}
			<section>
				<h3 class="label">Tests</h3>
				<ul class="tests">
					{#each counts.tests.slice(0, 12) as test (test.id)}
						<li>
							<button class="link mono" onclick={() => workspace.select(test.id)}
								>{test.qualifiedName}</button
							>
						</li>
					{/each}
				</ul>
				{#if counts.tests.length > 12}
					<p class="faint more">and {counts.tests.length - 12} more in the Impact view</p>
				{/if}
			</section>
		{/if}

		{#if workspace.repoId}
			<section>
				<h3 class="label">Source</h3>
				<SourceView
					repoId={workspace.repoId}
					file={symbol.file}
					startLine={symbol.startLine}
					endLine={symbol.endLine}
					focusLine={workspace.focusLine}
				/>
			</section>
		{/if}
	{:else if repository}
		<header>
			<h2>{repository.name}</h2>
			<p class="qualified mono">{repository.root}</p>
		</header>
		<dl class="facts">
			<dt>Revision</dt>
			<dd class="mono">
				{repository.branch ?? 'detached'}{repository.indexedSha
					? ` @ ${shortSha(repository.indexedSha)}`
					: ''}
			</dd>
			<dt>Indexed</dt>
			<dd>{timestamp(repository.indexedAt)}</dd>
			<dt>Files</dt>
			<dd>{repository.sourceFiles.toLocaleString()}</dd>
			<dt>Lines</dt>
			<dd>{repository.loc.toLocaleString()} non-blank</dd>
			<dt>Languages</dt>
			<dd>{repository.languages.join(', ')}</dd>
		</dl>
		<p class="hint muted">
			Select a symbol in the explorer or search with <kbd>⌘K</kbd> to see its callers, its impact and
			its source.
		</p>
	{:else}
		<p class="hint muted">No repository selected.</p>
	{/if}
</aside>

<style>
	.inspector {
		height: 100%;
		overflow: auto;
		padding: 12px 14px 24px;
		border-left: 1px solid var(--border);
		background: var(--bg);
	}
	header {
		margin-bottom: 10px;
	}
	.title {
		display: flex;
		align-items: center;
		gap: 8px;
	}
	h2 {
		margin: 0;
		font-size: 15px;
		font-weight: 600;
		word-break: break-all;
	}
	h3 {
		margin: 14px 0 6px;
		font-weight: 600;
	}
	.qualified {
		margin: 4px 0 0;
		color: var(--text-muted);
		word-break: break-all;
	}
	.actions {
		display: flex;
		gap: 6px;
		margin-bottom: 10px;
	}
	.facts {
		display: grid;
		grid-template-columns: 82px 1fr;
		gap: 3px 10px;
		margin: 0;
	}
	dt {
		color: var(--text-muted);
	}
	dd {
		margin: 0;
		word-break: break-all;
	}
	.signature {
		margin: 0;
		padding: 6px 8px;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--bg-panel);
		white-space: pre-wrap;
		word-break: break-word;
	}
	ul {
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.unresolved li,
	.tests li {
		display: flex;
		gap: 8px;
		align-items: baseline;
		line-height: 20px;
	}
	.link {
		padding: 0;
		border: 0;
		background: none;
		color: var(--accent);
		text-align: left;
		word-break: break-all;
	}
	.link:hover {
		text-decoration: underline;
	}
	.more {
		margin: 4px 0 0;
		font-size: 12px;
	}
	.hint {
		margin-top: 14px;
		font-size: 12px;
	}
	kbd {
		padding: 0 4px;
		border: 1px solid var(--border);
		border-radius: 3px;
		font-family: var(--font-mono);
		font-size: 11px;
	}
</style>
