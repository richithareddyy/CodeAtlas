<script lang="ts">
	import { onMount } from 'svelte';
	import * as api from '../api/queries';
	import type { Symbol, SymbolKind } from '../api/types';
	import { workspace } from '../state/workspace.svelte';
	import KindBadge from './KindBadge.svelte';
	import Self from './ExplorerNode.svelte';

	interface Props {
		id: string;
		label: string;
		kind: SymbolKind;
		test?: boolean;
		level: number;
		initiallyOpen?: boolean;
		/** Extra text after the label (e.g. the crate kind). */
		note?: string;
	}
	let { id, label, kind, test = false, level, initiallyOpen = false, note }: Props = $props();

	const CONTAINERS: SymbolKind[] = ['MODULE', 'STRUCT', 'ENUM', 'TRAIT'];
	const expandable = $derived(CONTAINERS.includes(kind));
	let open = $state(false);
	let children = $state<Symbol[] | null>(null);
	let failed = $state(false);

	onMount(() => {
		if (initiallyOpen) void toggle();
	});

	async function toggle() {
		open = !open;
		if (!open || children || !workspace.repoId) return;
		try {
			children = await api.children(workspace.repoId, id);
		} catch {
			failed = true;
		}
	}

	function activate() {
		if (kind === 'MODULE') {
			void workspace.select(id);
			if (!open) void toggle();
		} else {
			void workspace.focus(id);
		}
	}
</script>

<li
	role="treeitem"
	aria-expanded={expandable ? open : undefined}
	aria-selected={workspace.selected?.id === id}
>
	<div
		class="row"
		class:selected={workspace.selected?.id === id}
		style:padding-left="{6 + level * 14}px"
	>
		{#if expandable}
			<button class="twisty" aria-label={open ? 'Collapse' : 'Expand'} onclick={toggle}>
				{open ? '▾' : '▸'}
			</button>
		{:else}
			<span class="twisty"></span>
		{/if}
		<button class="name" onclick={activate} title={id}>
			<KindBadge {kind} {test} />
			<span class="text">{label}</span>
			{#if note}<span class="note">{note}</span>{/if}
		</button>
	</div>
	{#if open}
		{#if failed}
			<p class="status" style:padding-left="{26 + level * 14}px">Could not load items.</p>
		{:else if !children}
			<p class="status" style:padding-left="{26 + level * 14}px">Loading…</p>
		{:else if children.length === 0}
			<p class="status" style:padding-left="{26 + level * 14}px">No items</p>
		{:else}
			<ul role="group">
				{#each children as child (child.id)}
					<Self
						id={child.id}
						label={child.name}
						kind={child.kind}
						test={child.isTest}
						level={level + 1}
					/>
				{/each}
			</ul>
		{/if}
	{/if}
</li>

<style>
	ul {
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.row {
		display: flex;
		align-items: center;
		height: 24px;
		padding-right: 6px;
	}
	.row:hover {
		background: var(--bg-hover);
	}
	.row.selected {
		background: var(--accent-soft);
	}
	.twisty {
		flex: 0 0 16px;
		padding: 0;
		border: 0;
		background: none;
		color: var(--text-faint);
		font-size: 10px;
	}
	.name {
		display: flex;
		align-items: center;
		gap: 6px;
		min-width: 0;
		padding: 0;
		border: 0;
		background: none;
		text-align: left;
	}
	.text {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-family: var(--font-mono);
		font-size: 12px;
	}
	.note {
		color: var(--text-faint);
		font-size: 11px;
	}
	.status {
		margin: 0;
		height: 22px;
		color: var(--text-faint);
		font-size: 12px;
		line-height: 22px;
	}
</style>
