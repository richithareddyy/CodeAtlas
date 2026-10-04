<script lang="ts">
	import { onMount } from 'svelte';
	import CommandPalette from '#lib/components/CommandPalette.svelte';
	import Explorer from '#lib/components/Explorer.svelte';
	import IndexDialog from '#lib/components/IndexDialog.svelte';
	import Inspector from '#lib/components/Inspector.svelte';
	import StatusBar from '#lib/components/StatusBar.svelte';
	import TopBar from '#lib/components/TopBar.svelte';
	import ArchitectureView from '#lib/components/views/ArchitectureView.svelte';
	import ChangesView from '#lib/components/views/ChangesView.svelte';
	import CyclesView from '#lib/components/views/CyclesView.svelte';
	import GraphView from '#lib/components/views/GraphView.svelte';
	import ImpactView from '#lib/components/views/ImpactView.svelte';
	import { workspace, type View } from '#lib/state/workspace.svelte.js';

	const TABS: { view: View; label: string }[] = [
		{ view: 'graph', label: 'Graph' },
		{ view: 'impact', label: 'Impact' },
		{ view: 'changes', label: 'Changes' },
		{ view: 'architecture', label: 'Architecture' },
		{ view: 'cycles', label: 'Cycles' }
	];

	onMount(() => {
		void workspace.init(new URL(window.location.href));
	});

	function onkeydown(event: KeyboardEvent) {
		const target = event.target as HTMLElement | null;
		const typing = target?.closest('input, textarea, select') !== null;
		if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
			event.preventDefault();
			if (workspace.repository) workspace.paletteOpen = !workspace.paletteOpen;
		} else if (event.key === '/' && !typing && workspace.repository) {
			event.preventDefault();
			workspace.paletteOpen = true;
		} else if (event.key === 'Escape') {
			workspace.paletteOpen = false;
		}
	}
</script>

<svelte:window {onkeydown} />

<div class="app">
	<TopBar />
	<div class="workspace">
		<Explorer />
		<main class="center">
			<div class="tabs" role="tablist" aria-label="Views">
				{#each TABS as tab (tab.view)}
					<button
						role="tab"
						aria-selected={workspace.view === tab.view}
						class:active={workspace.view === tab.view}
						onclick={() => workspace.setView(tab.view)}
					>
						{tab.label}
					</button>
				{/each}
			</div>
			{#if workspace.error}
				<div class="banner error" role="alert">
					<span>{workspace.error}</span>
					<button class="dismiss" aria-label="Dismiss" onclick={() => (workspace.error = null)}
						>×</button
					>
				</div>
			{/if}
			{#if workspace.apiReachable && workspace.repositories.length === 0}
				<div class="first-run">
					<h1>No repositories indexed</h1>
					<p>
						Index a Rust repository to start: use <strong>Index…</strong> above, or run
						<code>codeatlas index &lt;path&gt;</code>.
					</p>
					<button class="btn primary" onclick={() => (workspace.indexOpen = true)}
						>Index a repository</button
					>
				</div>
			{:else}
				<div class="view" role="tabpanel">
					{#if workspace.view === 'graph'}
						<GraphView />
					{:else if workspace.view === 'impact'}
						<ImpactView />
					{:else if workspace.view === 'changes'}
						<ChangesView />
					{:else if workspace.view === 'architecture'}
						<ArchitectureView />
					{:else}
						<CyclesView />
					{/if}
				</div>
			{/if}
		</main>
		<Inspector />
	</div>
	<StatusBar />
</div>

<CommandPalette />
<IndexDialog />

<style>
	.app {
		display: grid;
		grid-template-rows: auto 1fr auto;
		height: 100vh;
	}
	.workspace {
		display: grid;
		grid-template-columns: minmax(220px, 280px) 1fr minmax(300px, 380px);
		min-height: 0;
	}
	.center {
		display: flex;
		flex-direction: column;
		min-width: 0;
		min-height: 0;
	}
	.tabs {
		display: flex;
		gap: 2px;
		height: 32px;
		padding: 0 8px;
		border-bottom: 1px solid var(--border);
		background: var(--bg);
	}
	.tabs button {
		padding: 0 12px;
		border: 0;
		border-bottom: 2px solid transparent;
		background: none;
		color: var(--text-muted);
	}
	.tabs button:hover {
		color: var(--text);
	}
	.tabs button.active {
		border-bottom-color: var(--accent);
		color: var(--text);
		font-weight: 600;
	}
	.view {
		flex: 1;
		min-height: 0;
	}
	.dismiss {
		margin-left: auto;
		padding: 0 4px;
		border: 0;
		background: none;
		color: inherit;
		font-size: 15px;
		line-height: 1;
	}
	.first-run {
		margin: 12vh auto 0;
		max-width: 460px;
		text-align: center;
	}
	.first-run h1 {
		font-size: 17px;
	}
	.first-run p {
		color: var(--text-muted);
	}
</style>
