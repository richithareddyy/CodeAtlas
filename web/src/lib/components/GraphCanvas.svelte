<script lang="ts">
	import { onMount } from 'svelte';
	import cytoscape from 'cytoscape';
	import type { Core, ElementDefinition, LayoutOptions } from 'cytoscape';
	import type { CanvasData } from '../graph/elements';
	import { graphStyle } from '../graph/style';
	import {
		dependencyLayers,
		layers,
		place,
		type LayoutNode,
		type Positions
	} from '../graph/layout';

	interface Props {
		data: CanvasData;
		selectedId?: string | null;
		onselect?: (id: string) => void;
		/** Double-click on a node. */
		onactivate?: (id: string) => void;
		label?: string;
	}

	let {
		data,
		selectedId = null,
		onselect,
		onactivate,
		label = 'Dependency graph'
	}: Props = $props();

	const MAX_FIT_ZOOM = 1.15;
	/** Layer lengths (layout units) tried when wide layers are wrapped. */
	const WRAPS = [Number.POSITIVE_INFINITY, 2400, 1600, 1200, 800, 500];

	let container: HTMLDivElement;
	let cy: Core | null = null;
	let structure = '';
	let tooltip = $state<{ text: string; x: number; y: number } | null>(null);

	onMount(() => {
		const instance = cytoscape({
			container,
			style: graphStyle(),
			wheelSensitivity: 0.25,
			minZoom: 0.15,
			maxZoom: 3,
			boxSelectionEnabled: false,
			selectionType: 'single'
		});
		instance.on('tap', 'node', (event) => onselect?.(event.target.id()));
		instance.on('dbltap', 'node', (event) => onactivate?.(event.target.id()));
		instance.on('mouseover', 'node', (event) => {
			const position = event.target.renderedPosition();
			tooltip = {
				text: event.target.data('tooltip'),
				x: position.x,
				y: position.y + 18
			};
		});
		instance.on('mouseout', 'node', () => (tooltip = null));
		instance.on('viewport', () => (tooltip = null));

		const media = matchMedia('(prefers-color-scheme: dark)');
		const restyle = () => instance.style(graphStyle());
		media.addEventListener('change', restyle);

		cy = instance;
		sync(data);
		return () => {
			media.removeEventListener('change', restyle);
			instance.destroy();
			cy = null;
		};
	});

	$effect(() => {
		sync(data);
	});

	$effect(() => {
		highlight(selectedId);
	});

	function sync(next: CanvasData) {
		const instance = cy;
		if (!instance) return;
		const wanted = new Set<string>();
		for (const n of next.nodes) wanted.add(n.id);
		for (const e of next.edges) wanted.add(e.id);

		instance.batch(() => {
			instance.elements().forEach((element) => {
				if (!wanted.has(element.id())) element.remove();
			});
			const definitions: ElementDefinition[] = [];
			for (const n of next.nodes) {
				const nodeData = { id: n.id, label: n.label, width: n.width, tooltip: n.tooltip };
				const existing = instance.getElementById(n.id);
				if (existing.nonempty()) {
					existing.data(nodeData);
					existing.classes(n.classes.join(' '));
				} else {
					definitions.push({ group: 'nodes', data: nodeData, classes: n.classes.join(' ') });
				}
			}
			for (const e of next.edges) {
				const edgeData = { id: e.id, source: e.source, target: e.target, label: e.label };
				const existing = instance.getElementById(e.id);
				if (existing.nonempty()) {
					existing.classes(e.classes.join(' '));
				} else {
					definitions.push({ group: 'edges', data: edgeData, classes: e.classes.join(' ') });
				}
			}
			instance.add(definitions);
		});

		const key = `${next.layout}|${[...wanted].sort().join('|')}`;
		if (key !== structure) {
			structure = key;
			layout(next);
		}
		highlight(selectedId);
	}

	function layout(next: CanvasData) {
		const instance = cy;
		if (!instance || instance.nodes().empty()) return;
		const roots = next.roots.filter((r) => instance.getElementById(r).nonempty());
		const options: LayoutOptions =
			next.layout === 'ring'
				? {
						name: 'circle',
						fit: true,
						padding: 32,
						animate: false,
						avoidOverlap: true,
						nodeDimensionsIncludeLabels: true,
						// Clockwise from the top, following the cycle's hop order.
						startAngle: -Math.PI / 2
					}
				: {
						name: 'preset',
						positions: layeredPositions(instance, next.layout === 'tree' ? roots : null),
						fit: true,
						padding: 24,
						animate: false
					};
		instance.layout(options).run();
		// Small graphs would otherwise be fitted at a huge zoom level.
		if (instance.zoom() > MAX_FIT_ZOOM) {
			instance.zoom(MAX_FIT_ZOOM);
			instance.center();
		}
	}

	/**
	 * Layers run in whichever direction lets the graph be drawn largest.
	 * `roots` null lays the graph out by dependency order instead of from roots.
	 */
	function layeredPositions(instance: Core, roots: string[] | null) {
		const nodes = instance
			.nodes()
			.map((n) => ({ id: n.id(), width: Number(n.data('width')) || 80 }));
		const edges = instance
			.edges()
			.map((e) => ({ source: e.data('source'), target: e.data('target') }));
		const rows = roots ? layers(nodes, edges, roots) : dependencyLayers(nodes, edges);
		let best: { positions: Positions; zoom: number } | null = null;
		for (const orientation of ['horizontal', 'vertical'] as const) {
			for (const wrap of WRAPS) {
				const positions = place(nodes, rows, orientation, wrap);
				const zoom = Math.min(MAX_FIT_ZOOM, fitZoom(positions, nodes, instance));
				// Strictly better only, so unwrapped layouts win ties.
				if (!best || zoom > best.zoom * 1.05) best = { positions, zoom };
			}
		}
		return best!.positions;
	}

	function fitZoom(positions: Positions, nodes: LayoutNode[], instance: Core) {
		let [x1, y1, x2, y2] = [Infinity, Infinity, -Infinity, -Infinity];
		for (const n of nodes) {
			const p = positions[n.id];
			x1 = Math.min(x1, p.x - n.width / 2);
			x2 = Math.max(x2, p.x + n.width / 2);
			y1 = Math.min(y1, p.y - 12);
			y2 = Math.max(y2, p.y + 12);
		}
		return Math.min((instance.width() - 48) / (x2 - x1), (instance.height() - 48) / (y2 - y1));
	}

	function highlight(id: string | null) {
		const instance = cy;
		if (!instance) return;
		instance.$(':selected').unselect();
		if (id) instance.getElementById(id).select();
	}

	export function fit() {
		if (!cy) return;
		cy.fit(undefined, 24);
		if (cy.zoom() > MAX_FIT_ZOOM) {
			cy.zoom(MAX_FIT_ZOOM);
			cy.center();
		}
	}

	export function center(id: string) {
		const element = cy?.getElementById(id);
		if (cy && element?.nonempty())
			cy.animate({ center: { eles: element }, zoom: Math.max(cy.zoom(), 1) }, { duration: 200 });
	}

	export function relayout() {
		layout(data);
	}
</script>

<div class="canvas" role="application" aria-label={label}>
	<div class="surface" bind:this={container}></div>
	{#if tooltip}
		<div class="tooltip mono" style:left="{tooltip.x}px" style:top="{tooltip.y}px">
			{tooltip.text}
		</div>
	{/if}
</div>

<style>
	.canvas {
		position: relative;
		width: 100%;
		height: 100%;
		min-height: 0;
		overflow: hidden;
		background:
			radial-gradient(circle, var(--border) 1px, transparent 1px) 0 0 / 22px 22px,
			var(--bg-panel);
	}

	.surface {
		position: absolute;
		inset: 0;
	}

	.tooltip {
		position: absolute;
		transform: translateX(-50%);
		max-width: 420px;
		padding: 4px 8px;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--bg);
		color: var(--text);
		font-size: 11px;
		white-space: pre;
		pointer-events: none;
		box-shadow: 0 2px 8px rgb(0 0 0 / 0.12);
		z-index: 2;
	}
</style>
