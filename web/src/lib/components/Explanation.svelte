<script lang="ts">
	// An explanation of an impact result: the text, with each citation linked
	// to the fact it rests on, the facts themselves (with their source
	// locations), and who wrote the text. Fetched on demand.
	import { explainImpact } from '../api/queries';
	import type { Explanation, Fact } from '../api/types';
	import { explanationParts } from '../format';
	import { workspace } from '../state/workspace.svelte';

	interface Props {
		changedId: string;
		/** Explains why this symbol is affected; without it, the whole impact. */
		affectedId?: string | null;
		/** Fetch immediately instead of waiting for the button. */
		auto?: boolean;
	}

	let { changedId, affectedId = null, auto = false }: Props = $props();

	let result = $state.raw<Explanation | null>(null);
	let error = $state<string | null>(null);
	let loading = $state(false);
	let highlighted = $state<string | null>(null);

	async function load() {
		const repo = workspace.repoId;
		if (!repo || loading) return;
		loading = true;
		error = null;
		try {
			result = await explainImpact(repo, changedId, affectedId, workspace.impactDepth);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		if (auto) void load();
	});

	/** The symbol whose file holds the evidence for a fact. */
	function owner(fact: Fact): string | null {
		return fact.kind === 'DISPATCHES_TO' ? fact.targetId : fact.sourceId;
	}

	function openFact(fact: Fact) {
		const id = owner(fact);
		if (id) void workspace.select(id, fact.lines[0] ?? null);
	}

	function cite(id: string) {
		highlighted = id;
		const fact = result?.facts.find((f) => f.id === id);
		if (fact) openFact(fact);
	}
</script>

{#snippet rich(text: string)}
	{#each explanationParts(text) as part, i (i)}
		{#if part.kind === 'code'}<code>{part.value}</code>{:else if part.kind === 'cite'}<span
				class="cites"
				>{#each part.ids as id (id)}<button
						class="cite"
						class:missing={!result?.facts.some((f) => f.id === id)}
						title="Show fact {id} and its source"
						onmouseenter={() => (highlighted = id)}
						onmouseleave={() => (highlighted = null)}
						onclick={() => cite(id)}>{id}</button
					>{/each}</span
			>{:else}{part.value}{/if}
	{/each}
{/snippet}

<div class="explanation">
	{#if !result}
		<div class="ask">
			<button class="btn small" onclick={load} disabled={loading}>
				{loading ? 'Explaining…' : affectedId ? 'Why?' : 'Explain this impact'}
			</button>
			{#if loading}<span class="faint">Gathering facts from the graph…</span>{/if}
		</div>
		{#if error}<p class="error">{error}</p>{/if}
	{:else}
		<p class="text">{@render rich(result.text)}</p>
		<div class="meta">
			{#if result.source === 'MODEL'}
				<span class="badge model" title="Checked against the facts below"
					>Worded by {result.model}, checked against the evidence</span
				>
			{:else}
				<span class="badge">Built from the evidence</span>
			{/if}
			{#if result.status === 'POSSIBLE'}<span class="badge possible">Possible, not certain</span
				>{/if}
			{#if result.status === 'INSUFFICIENT'}<span class="badge insufficient">No evidence</span>{/if}
		</div>
		{#if result.facts.length}
			<ol class="facts">
				{#each result.facts as fact (fact.id)}
					<li
						class:highlighted={highlighted === fact.id}
						class:uncertain={fact.kind === 'MAY_CALL'}
					>
						<span class="id mono">{fact.id}</span>
						<span class="fact">{@render rich(fact.text)}</span>
						{#if fact.file && owner(fact)}
							<button class="loc mono" title="Show the source" onclick={() => openFact(fact)}
								>{fact.file}{fact.lines.length ? `:${fact.lines[0]}` : ''}</button
							>
						{/if}
					</li>
				{/each}
			</ol>
		{/if}
		{#each result.notes as note, i (i)}<p class="note faint">{note}</p>{/each}
		{#if result.rejectedText}
			<details class="rejected">
				<summary class="faint">Rejected model answer</summary>
				<p>{result.rejectedText}</p>
				{#each result.verification?.problems ?? [] as problem, i (i)}
					<p class="faint">· {problem}</p>
				{/each}
			</details>
		{/if}
	{/if}
</div>

<style>
	.explanation {
		margin: 4px 0 2px 26px;
		font-size: 12px;
	}
	.ask {
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.text {
		margin: 4px 0;
		line-height: 1.5;
		/* The evidence-built summary lists examples on separate lines. */
		white-space: pre-line;
	}
	.cites {
		white-space: nowrap;
	}
	.cite {
		margin: 0 1px;
		padding: 0 4px;
		border: 1px solid var(--border);
		border-radius: 3px;
		background: var(--bg-hover);
		font: inherit;
		font-size: 10px;
		vertical-align: 1px;
	}
	.cite:hover {
		border-color: var(--accent);
	}
	.cite.missing {
		border-color: var(--danger);
		color: var(--danger);
	}
	.meta {
		display: flex;
		gap: 6px;
		margin: 4px 0;
	}
	.badge {
		padding: 1px 6px;
		border: 1px solid var(--border);
		border-radius: 3px;
		color: var(--text-muted);
		font-size: 11px;
	}
	.badge.model {
		border-color: var(--accent);
	}
	.badge.possible {
		color: var(--edge-candidate);
	}
	.badge.insufficient {
		color: var(--warning);
	}
	.facts {
		margin: 4px 0;
		padding: 0;
		list-style: none;
		border-left: 1px solid var(--border);
	}
	.facts li {
		display: flex;
		align-items: baseline;
		gap: 8px;
		padding: 2px 0 2px 8px;
	}
	.facts li.highlighted {
		background: var(--accent-soft);
	}
	.facts li.uncertain .fact {
		color: var(--edge-candidate);
	}
	.id {
		color: var(--text-faint);
		min-width: 22px;
	}
	.loc {
		margin-left: auto;
		padding: 0;
		border: 0;
		background: none;
		color: var(--text-faint);
		white-space: nowrap;
	}
	.loc:hover {
		color: var(--accent);
		text-decoration: underline;
	}
	.note {
		margin: 2px 0;
		font-size: 11px;
	}
	.error {
		color: var(--danger);
	}
	.rejected p {
		margin: 2px 0;
	}
</style>
