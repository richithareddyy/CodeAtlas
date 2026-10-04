<script lang="ts">
	import { tick } from 'svelte';
	import * as api from '../api/queries';
	import { describeError } from '../api/client';
	import type { Symbol, SymbolKind } from '../api/types';
	import { workspace } from '../state/workspace.svelte';
	import KindBadge from './KindBadge.svelte';

	const FILTERS: { kind: SymbolKind | null; label: string }[] = [
		{ kind: null, label: 'All' },
		{ kind: 'FUNCTION', label: 'Functions' },
		{ kind: 'METHOD', label: 'Methods' },
		{ kind: 'STRUCT', label: 'Structs' },
		{ kind: 'ENUM', label: 'Enums' },
		{ kind: 'TRAIT', label: 'Traits' },
		{ kind: 'MODULE', label: 'Modules' }
	];

	let query = $state('');
	let filter = $state<SymbolKind | null>(null);
	let results = $state<Symbol[]>([]);
	let cursor = $state<string | null>(null);
	let hasMore = $state(false);
	let active = $state(0);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let input = $state<HTMLInputElement>();
	let list = $state<HTMLUListElement>();
	let generation = 0;

	$effect(() => {
		if (workspace.paletteOpen) void tick().then(() => input?.select());
	});

	$effect(() => {
		const text = query.trim();
		const kinds = filter ? [filter] : undefined;
		const repo = workspace.repoId;
		const current = ++generation;
		if (!text || !repo) {
			results = [];
			hasMore = false;
			error = null;
			return;
		}
		const timer = setTimeout(async () => {
			loading = true;
			try {
				const page = await api.search(repo, text, { kinds, first: 20 });
				if (current !== generation) return;
				results = page.edges.map((e) => e.node);
				cursor = page.pageInfo.endCursor;
				hasMore = page.pageInfo.hasNextPage;
				active = 0;
				error = null;
			} catch (e) {
				if (current === generation) error = describeError(e);
			} finally {
				if (current === generation) loading = false;
			}
		}, 120);
		return () => clearTimeout(timer);
	});

	async function more() {
		const repo = workspace.repoId;
		if (!repo || !cursor) return;
		const page = await api.search(repo, query.trim(), {
			kinds: filter ? [filter] : undefined,
			first: 20,
			after: cursor
		});
		results = [...results, ...page.edges.map((e) => e.node)];
		cursor = page.pageInfo.endCursor;
		hasMore = page.pageInfo.hasNextPage;
	}

	function close() {
		workspace.paletteOpen = false;
	}

	function choose(symbol: Symbol | undefined) {
		if (!symbol) return;
		close();
		void workspace.focus(symbol.id);
	}

	function onkeydown(event: KeyboardEvent) {
		if (event.key === 'ArrowDown') {
			active = Math.min(active + 1, results.length - 1);
		} else if (event.key === 'ArrowUp') {
			active = Math.max(active - 1, 0);
		} else if (event.key === 'Enter') {
			choose(results[active]);
		} else if (event.key === 'Escape') {
			close();
		} else {
			return;
		}
		event.preventDefault();
		void tick().then(() => list?.querySelector('.active')?.scrollIntoView({ block: 'nearest' }));
	}
</script>

{#if workspace.paletteOpen}
	<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
	<div class="backdrop" onclick={close}>
		<div
			class="palette"
			role="dialog"
			aria-modal="true"
			aria-label="Search symbols"
			tabindex="-1"
			onclick={(e) => e.stopPropagation()}
		>
			<input
				bind:this={input}
				bind:value={query}
				{onkeydown}
				class="query"
				placeholder="Search by name or path, e.g. authorize or PaymentService authorize"
				aria-label="Symbol search"
				aria-controls="palette-results"
				aria-activedescendant={results[active] ? `palette-${active}` : undefined}
				autocomplete="off"
				spellcheck="false"
			/>
			<div class="filters" role="group" aria-label="Kind filter">
				{#each FILTERS as f (f.label)}
					<button class:on={filter === f.kind} onclick={() => (filter = f.kind)}>{f.label}</button>
				{/each}
				{#if loading}<span class="spinner" aria-label="Searching"></span>{/if}
			</div>
			{#if error}
				<p class="message error">{error}</p>
			{:else if query.trim() && !loading && results.length === 0}
				<p class="message">No symbols match “{query.trim()}”.</p>
			{:else if !query.trim()}
				<p class="message">Type to search. ↑↓ to move, Enter to open, Esc to close.</p>
			{/if}
			{#if results.length}
				<ul id="palette-results" role="listbox" bind:this={list}>
					{#each results as symbol, i (symbol.id)}
						<li
							id="palette-{i}"
							role="option"
							aria-selected={i === active}
							class:active={i === active}
							onmousemove={() => (active = i)}
							onclick={() => choose(symbol)}
						>
							<KindBadge kind={symbol.kind} test={symbol.isTest} />
							<span class="name mono">{symbol.qualifiedName}</span>
							<span class="location mono">{symbol.file}:{symbol.startLine}</span>
						</li>
					{/each}
				</ul>
				{#if hasMore}
					<button class="more" onclick={more}>Load more</button>
				{/if}
			{/if}
		</div>
	</div>
{/if}

<style>
	.backdrop {
		position: fixed;
		inset: 0;
		display: flex;
		justify-content: center;
		align-items: flex-start;
		padding-top: 12vh;
		background: rgb(0 0 0 / 0.25);
		z-index: 20;
	}
	.palette {
		width: min(760px, 92vw);
		max-height: 70vh;
		display: flex;
		flex-direction: column;
		border: 1px solid var(--border-strong);
		border-radius: 6px;
		background: var(--bg);
		box-shadow: 0 12px 32px rgb(0 0 0 / 0.2);
		overflow: hidden;
	}
	.query {
		height: 42px;
		padding: 0 14px;
		border: 0;
		border-bottom: 1px solid var(--border);
		background: none;
		font-size: 14px;
		outline: none;
	}
	.filters {
		display: flex;
		align-items: center;
		gap: 4px;
		padding: 6px 10px;
		border-bottom: 1px solid var(--border);
	}
	.filters button {
		height: 22px;
		padding: 0 8px;
		border: 1px solid transparent;
		border-radius: var(--radius);
		background: none;
		color: var(--text-muted);
		font-size: 12px;
	}
	.filters button.on {
		border-color: var(--border);
		background: var(--accent-soft);
		color: var(--accent);
	}
	.message {
		margin: 0;
		padding: 12px 14px;
		color: var(--text-muted);
		font-size: 12px;
	}
	.message.error {
		color: var(--danger);
	}
	ul {
		margin: 0;
		padding: 4px 0;
		overflow: auto;
		list-style: none;
	}
	li {
		display: grid;
		grid-template-columns: auto 1fr auto;
		gap: 10px;
		align-items: center;
		padding: 5px 14px;
		cursor: pointer;
	}
	li.active {
		background: var(--accent-soft);
	}
	.name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.location {
		color: var(--text-faint);
	}
	.more {
		margin: 0 14px 10px;
		padding: 4px;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--bg);
	}
</style>
