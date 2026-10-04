<script lang="ts">
	import type { EvidenceStep } from '../api/types';
	import { lines, stepSentence } from '../format';
	import { workspace } from '../state/workspace.svelte';

	let { steps }: { steps: EvidenceStep[] } = $props();

	/** The symbol whose file holds the evidence for a step. */
	function evidenceOwner(step: EvidenceStep): string {
		return step.kind === 'DISPATCHES_TO' ? step.target : step.source;
	}
</script>

<ol class="path">
	{#each steps as step, i (i)}
		{@const sentence = stepSentence(step)}
		<li class:uncertain={step.kind === 'MAY_CALL'}>
			<button class="sym" onclick={() => workspace.select(step.source)}>{sentence.source}</button>
			<span class="verb">{sentence.verb}</span>
			<button class="sym" onclick={() => workspace.select(step.target)}>{sentence.target}</button>
			<button
				class="loc"
				title="Show the source of this evidence"
				onclick={() => workspace.select(evidenceOwner(step), step.lines[0] ?? null)}
			>
				{step.file}{step.lines.length ? `:${step.lines[0]}` : ''}
			</button>
			{#if step.lines.length > 1}<span class="faint">({lines(step.lines)})</span>{/if}
		</li>
	{/each}
</ol>

<style>
	.path {
		margin: 2px 0 0;
		padding: 0 0 0 14px;
		list-style: none;
		border-left: 1px solid var(--border);
	}
	li {
		display: flex;
		flex-wrap: wrap;
		gap: 0 5px;
		align-items: baseline;
		font-size: 12px;
		line-height: 20px;
	}
	li.uncertain .verb {
		color: var(--edge-candidate);
	}
	button {
		padding: 0;
		border: 0;
		background: none;
		font-family: var(--font-mono);
		font-size: 11.5px;
	}
	.sym {
		color: var(--text);
	}
	.sym:hover,
	.loc:hover {
		text-decoration: underline;
	}
	.verb {
		color: var(--text-muted);
	}
	.loc {
		color: var(--accent);
	}
</style>
