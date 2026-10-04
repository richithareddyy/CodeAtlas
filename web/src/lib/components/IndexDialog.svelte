<script lang="ts">
	import { tick } from 'svelte';
	import * as api from '../api/queries';
	import { describeError } from '../api/client';
	import type { IndexResult } from '../api/types';
	import { workspace } from '../state/workspace.svelte';

	let source = $state('');
	let running = $state(false);
	let error = $state<string | null>(null);
	let result = $state<IndexResult | null>(null);
	let input = $state<HTMLInputElement>();

	$effect(() => {
		if (workspace.indexOpen) {
			error = null;
			result = null;
			if (!source && workspace.repository) source = workspace.repository.root;
			void tick().then(() => input?.select());
		}
	});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (!source.trim() || running) return;
		running = true;
		error = null;
		result = null;
		try {
			result = await api.indexRepository(source.trim());
			await workspace.reloadRepositories();
			await workspace.selectRepository(result.repository.id);
		} catch (e) {
			error = describeError(e);
		} finally {
			running = false;
		}
	}

	function close() {
		if (!running) workspace.indexOpen = false;
	}
</script>

{#if workspace.indexOpen}
	<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
	<div class="backdrop" onclick={close} onkeydown={(e) => e.key === 'Escape' && close()}>
		<div
			class="dialog"
			role="dialog"
			aria-modal="true"
			aria-labelledby="index-title"
			tabindex="-1"
			onclick={(e) => e.stopPropagation()}
		>
			<form onsubmit={submit}>
				<h2 id="index-title">Index a repository</h2>
				<p class="muted">
					A local path or a Git URL, as seen by the CodeAtlas server. Indexing replaces any earlier
					index of the same repository.
				</p>
				<input
					bind:this={input}
					bind:value={source}
					class="input"
					placeholder="/path/to/repository or https://github.com/org/repo"
					aria-label="Repository path or URL"
					disabled={running}
				/>
				{#if running}
					<p class="status"><span class="spinner"></span> Analysing and writing the graph…</p>
				{/if}
				{#if error}
					<p class="status error">{error}</p>
				{/if}
				{#if result}
					<p class="status success">
						Indexed {result.repository.name}: {result.filesAnalyzed} files, {result.nodes.toLocaleString()}
						nodes, {result.relationships.toLocaleString()} relationships{#if result.resolutionRate !== null},
							{(result.resolutionRate * 100).toFixed(1)}% of calls resolved{/if} in {(
							(result.analysisMs + result.writeMs) /
							1000
						).toFixed(1)} s.
					</p>
				{/if}
				<div class="buttons">
					<button type="button" class="btn" onclick={close} disabled={running}>
						{result ? 'Close' : 'Cancel'}
					</button>
					<button type="submit" class="btn primary" disabled={running || !source.trim()}>
						Index
					</button>
				</div>
			</form>
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
		padding-top: 14vh;
		background: rgb(0 0 0 / 0.25);
		z-index: 20;
	}
	.dialog {
		width: min(560px, 92vw);
		padding: 16px 18px;
		border: 1px solid var(--border-strong);
		border-radius: 6px;
		background: var(--bg);
		box-shadow: 0 12px 32px rgb(0 0 0 / 0.2);
	}
	h2 {
		margin: 0 0 4px;
		font-size: 15px;
	}
	p {
		margin: 0 0 12px;
		font-size: 12px;
	}
	.input {
		width: 100%;
		height: 30px;
		font-family: var(--font-mono);
		font-size: 12px;
	}
	.status {
		display: flex;
		gap: 8px;
		align-items: center;
		margin: 10px 0 0;
	}
	.error {
		color: var(--danger);
	}
	.success {
		color: var(--success);
	}
	.buttons {
		display: flex;
		justify-content: flex-end;
		gap: 8px;
		margin-top: 14px;
	}
</style>
