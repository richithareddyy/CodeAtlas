<script lang="ts">
	import type { SymbolKind } from '../api/types';
	import { KIND_LABEL } from '../format';

	let { kind, test = false }: { kind: SymbolKind; test?: boolean } = $props();
</script>

<span
	class="badge kind-{kind.toLowerCase()}"
	class:test
	title={test ? `${kind.toLowerCase()} (test)` : kind.toLowerCase()}
>
	{test ? 'test' : KIND_LABEL[kind]}
</span>

<style>
	.badge {
		display: inline-block;
		min-width: 34px;
		padding: 0 4px;
		border-radius: 3px;
		font-family: var(--font-mono);
		font-size: 10.5px;
		line-height: 16px;
		text-align: center;
		border: 1px solid currentColor;
		flex-shrink: 0;
	}
	.kind-function {
		color: var(--kind-fn);
	}
	.kind-method {
		color: var(--kind-method);
	}
	.kind-struct {
		color: var(--kind-struct);
	}
	.kind-enum {
		color: var(--kind-enum);
	}
	.kind-trait {
		color: var(--kind-trait);
	}
	.kind-module {
		color: var(--kind-module);
	}
	.test {
		color: var(--kind-test);
		border-style: dashed;
	}
</style>
