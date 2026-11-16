<script lang="ts">
	import type { SelectionSafety, UnsafeReason } from '../api/types';
	import { shortLabel } from '../format';
	import { workspace } from '../state/workspace.svelte';

	let { safety }: { safety: SelectionSafety } = $props();

	function describe(r: UnsafeReason): string {
		const symbol = r.symbol ? shortLabel(r.symbol) : '';
		switch (r.kind) {
			case 'TEST_MACROS':
				return `Test code has ${r.count} item-level macro invocation${r.count === 1 ? '' : 's'}, which can generate tests CodeAtlas cannot see.`;
			case 'AMBIGUOUS_CALLERS':
				return `${symbol} is a candidate of ambiguous calls from ${r.callers.map(shortLabel).join(', ')}.`;
			case 'UNRESOLVED_CALLS':
				return `Unresolved calls may target ${symbol}.`;
			case 'HIDDEN_MENTIONS':
				return `${symbol} is used where no call is recorded (in macro tokens or as a value).`;
			case 'EXTERNAL_TRAIT':
				return `${symbol} implements a trait from outside the repository; its callers there are not analysed.`;
			case 'TRUNCATED':
				return 'The traversal stopped at the node limit.';
		}
	}
</script>

{#if safety.selective}
	<p class="faint note">
		No known blind spot applies to this selection. This is a heuristic check, not a guarantee.
	</p>
{:else}
	<div class="banner warning">
		<strong>Run the full test suite.</strong> Selective execution is unsafe here: CodeAtlas cannot
		rule out affected tests outside this selection.
		<ul>
			{#each safety.reasons as r, i (i)}
				<li>
					{describe(r)}
					{#if r.symbol}
						<button class="link mono" onclick={() => workspace.select(r.symbol!)}>show</button>
					{/if}
					{#each r.sites.slice(0, 3) as site (site.file + site.line + site.detail)}
						<span class="site mono">{site.detail} at {site.file}:{site.line}</span>
					{/each}
				</li>
			{/each}
		</ul>
	</div>
{/if}

<style>
	.note {
		margin: 6px 0;
		font-size: 12px;
	}
	.banner {
		display: block;
		margin: 6px 0;
		font-size: 12px;
	}
	ul {
		margin: 4px 0 0;
		padding-left: 18px;
	}
	.link {
		padding: 0;
		border: 0;
		background: none;
		color: var(--accent);
		font-size: 11px;
	}
	.site {
		margin-left: 6px;
		color: var(--text-faint);
		font-size: 11px;
	}
</style>
