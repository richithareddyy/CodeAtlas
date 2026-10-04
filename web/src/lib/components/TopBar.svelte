<script lang="ts">
	import { shortSha, timestamp } from '../format';
	import { workspace } from '../state/workspace.svelte';

	const repo = $derived(workspace.repository);
	const isMac = typeof navigator !== 'undefined' && /Mac/.test(navigator.platform);
</script>

<header class="topbar">
	<div class="brand">
		<svg viewBox="0 0 32 32" width="18" height="18" aria-hidden="true">
			<circle cx="9" cy="10" r="3" fill="var(--kind-fn)" />
			<circle cx="23" cy="9" r="3" fill="var(--kind-fn)" />
			<circle cx="16" cy="23" r="3.5" fill="var(--kind-struct)" />
			<path d="M11 12l4 8M21 11l-4 9M12 10h8" stroke="var(--edge)" stroke-width="1.6" fill="none" />
		</svg>
		<span>CodeAtlas</span>
	</div>

	<span class="sep" aria-hidden="true">/</span>
	<label class="repo">
		<span class="visually-hidden">Repository</span>
		<select
			class="input"
			value={workspace.repoId ?? ''}
			disabled={workspace.repositories.length === 0}
			onchange={(e) => workspace.selectRepository(e.currentTarget.value)}
		>
			{#if workspace.repositories.length === 0}
				<option value="">No repositories</option>
			{/if}
			{#each workspace.repositories as r (r.id)}
				<option value={r.id}>{r.name}</option>
			{/each}
		</select>
	</label>
	{#if repo}
		<span class="revision mono" title="Indexed {timestamp(repo.indexedAt)}">
			{repo.branch ?? 'detached'}{repo.indexedSha ? ` @ ${shortSha(repo.indexedSha)}` : ''}
		</span>
	{/if}
	<button class="btn small" onclick={() => (workspace.indexOpen = true)}>Index…</button>

	<button
		class="search"
		onclick={() => (workspace.paletteOpen = true)}
		disabled={!repo}
		aria-keyshortcuts={isMac ? 'Meta+K' : 'Control+K'}
	>
		<span>Search symbols</span>
		<kbd>{isMac ? '⌘' : 'Ctrl+'}K</kbd>
	</button>
</header>

<style>
	.topbar {
		display: flex;
		align-items: center;
		gap: 10px;
		height: 44px;
		padding: 0 12px;
		border-bottom: 1px solid var(--border);
		background: var(--bg);
	}
	.brand {
		display: flex;
		align-items: center;
		gap: 7px;
		font-weight: 650;
		letter-spacing: -0.01em;
	}
	.sep {
		color: var(--text-faint);
	}
	.repo select {
		min-width: 170px;
		font-weight: 600;
	}
	.revision {
		color: var(--text-muted);
	}
	.search {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 24px;
		width: min(380px, 34vw);
		height: 28px;
		margin-left: auto;
		padding: 0 6px 0 10px;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--bg-subtle);
		color: var(--text-muted);
	}
	.search:hover:not(:disabled) {
		border-color: var(--border-strong);
	}
	kbd {
		padding: 0 5px;
		border: 1px solid var(--border);
		border-radius: 3px;
		background: var(--bg);
		font-family: var(--font-mono);
		font-size: 11px;
	}
	.visually-hidden {
		position: absolute;
		width: 1px;
		height: 1px;
		overflow: hidden;
		clip: rect(0 0 0 0);
	}
</style>
