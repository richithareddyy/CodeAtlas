<script lang="ts">
	import * as api from '../api/queries';
	import { describeError } from '../api/client';
	import type { SourceSnippet } from '../api/types';

	interface Props {
		repoId: string;
		file: string;
		startLine: number;
		endLine: number;
		/** A line to emphasise, e.g. the call site from an evidence link. */
		focusLine?: number | null;
	}
	let { repoId, file, startLine, endLine, focusLine = null }: Props = $props();

	const CONTEXT = 3;
	let snippet = $state<SourceSnippet | null>(null);
	let error = $state<string | null>(null);
	let loading = $state(false);
	let list = $state<HTMLOListElement>();

	$effect(() => {
		const from = Math.max(1, Math.min(startLine, focusLine ?? startLine) - CONTEXT);
		const to = Math.max(endLine, focusLine ?? endLine) + CONTEXT;
		const request = { repoId, file, from, to };
		loading = true;
		error = null;
		api
			.source(request.repoId, request.file, request.from, request.to)
			.then((result) => {
				if (request.file === file && request.repoId === repoId) snippet = result;
			})
			.catch((e) => {
				snippet = null;
				error = describeError(e);
			})
			.finally(() => (loading = false));
	});

	$effect(() => {
		if (!snippet || !list || focusLine === null) return;
		list.querySelector('.focus')?.scrollIntoView({ block: 'center' });
	});
</script>

<div class="source">
	{#if error}
		<p class="message">{error}</p>
	{:else if !snippet}
		<p class="message">{loading ? 'Loading source…' : ''}</p>
	{:else}
		<ol class="mono" bind:this={list} start={snippet.startLine}>
			{#each snippet.lines as text, i (i)}
				{@const line = snippet.startLine + i}
				<li class:inside={line >= startLine && line <= endLine} class:focus={line === focusLine}>
					<span class="number">{line}</span><span class="code">{text || ' '}</span>
				</li>
			{/each}
		</ol>
		{#if snippet.endLine < Math.min(endLine + CONTEXT, snippet.totalLines)}
			<p class="message">Showing the first {snippet.lines.length} lines.</p>
		{/if}
	{/if}
</div>

<style>
	.source {
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--bg-panel);
		overflow: auto;
		max-height: 420px;
	}
	ol {
		margin: 0;
		padding: 4px 0;
		list-style: none;
		font-size: 11.5px;
		line-height: 18px;
	}
	li {
		display: flex;
		white-space: pre;
	}
	li.inside {
		background: var(--accent-soft);
	}
	li.focus {
		background: var(--warning-soft);
		box-shadow: inset 2px 0 0 var(--warning);
	}
	.number {
		flex: 0 0 44px;
		padding-right: 10px;
		text-align: right;
		color: var(--text-faint);
		user-select: none;
	}
	.code {
		padding-right: 12px;
	}
	.message {
		margin: 0;
		padding: 8px 10px;
		font-size: 12px;
		color: var(--text-muted);
	}
</style>
