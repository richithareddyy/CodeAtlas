<script lang="ts" generics="T extends string">
	interface Props {
		options: { value: T; label: string; title?: string }[];
		value: T;
		onchange: (value: T) => void;
		label: string;
	}
	let { options, value, onchange, label }: Props = $props();
</script>

<div class="segmented" role="radiogroup" aria-label={label}>
	{#each options as option (option.value)}
		<button
			type="button"
			role="radio"
			aria-checked={option.value === value}
			class:active={option.value === value}
			title={option.title}
			onclick={() => onchange(option.value)}
		>
			{option.label}
		</button>
	{/each}
</div>

<style>
	.segmented {
		display: inline-flex;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		overflow: hidden;
	}
	button {
		height: 24px;
		padding: 0 9px;
		border: 0;
		border-right: 1px solid var(--border);
		background: var(--bg);
		font-size: 12px;
	}
	button:last-child {
		border-right: 0;
	}
	button:hover {
		background: var(--bg-hover);
	}
	button.active {
		background: var(--accent-soft);
		color: var(--accent);
		font-weight: 600;
	}
</style>
