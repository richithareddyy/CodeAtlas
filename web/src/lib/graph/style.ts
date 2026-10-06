// Cytoscape stylesheet built from the CSS custom properties in app.css, so
// the graph follows the light / dark theme.

import type { StylesheetJson } from 'cytoscape';

function token(name: string): string {
	return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

export function graphStyle(): StylesheetJson {
	const t = {
		text: token('--text'),
		muted: token('--text-muted'),
		surface: token('--bg'),
		subtle: token('--bg-subtle'),
		accent: token('--accent'),
		accentSoft: token('--accent-soft'),
		edge: token('--edge'),
		implements: token('--kind-trait'),
		candidate: token('--edge-candidate'),
		cycle: token('--danger'),
		test: token('--kind-test'),
		mono: token('--font-mono')
	};
	const kinds: [string, string][] = [
		['function', token('--kind-fn')],
		['method', token('--kind-method')],
		['struct', token('--kind-struct')],
		['enum', token('--kind-enum')],
		['trait', token('--kind-trait')],
		['module', token('--kind-module')],
		['crate', token('--kind-module')],
		['file', token('--kind-module')]
	];

	return [
		{
			selector: 'node',
			style: {
				shape: 'round-rectangle',
				width: 'data(width)',
				height: 24,
				label: 'data(label)',
				'font-family': t.mono,
				'font-size': 11,
				color: t.text,
				'text-valign': 'center',
				'text-halign': 'center',
				'background-color': t.surface,
				'border-width': 1.5,
				'border-color': t.edge
			}
		},
		...kinds.map(([kind, color]) => ({
			selector: `node.kind-${kind}`,
			style: { 'border-color': color }
		})),
		{ selector: 'node.test', style: { 'border-style': 'dashed', 'border-color': t.test } },
		{ selector: 'node.root', style: { 'background-color': t.accentSoft, 'border-width': 2.5 } },
		{ selector: 'node.changed', style: { 'border-color': t.accent } },
		{ selector: 'node.expanded', style: { 'background-color': t.subtle } },
		{ selector: 'node.possible', style: { opacity: 0.55, 'border-style': 'dotted' } },
		{
			selector: 'node.removed',
			style: { 'border-color': t.cycle, 'border-style': 'dashed', color: t.muted }
		},
		{ selector: 'node.cycle', style: { 'border-color': t.cycle, 'border-width': 2 } },
		{ selector: 'node.context', style: { opacity: 0.45 } },
		{ selector: 'node.group', style: { 'border-width': 2.5, 'background-color': t.subtle } },
		{
			selector: 'node:selected',
			style: {
				'border-color': t.accent,
				'border-width': 3,
				'overlay-color': t.accent,
				'overlay-opacity': 0.08,
				'overlay-padding': 4
			}
		},
		{
			selector: 'edge',
			style: {
				width: 1.3,
				'line-color': t.edge,
				'target-arrow-color': t.edge,
				'target-arrow-shape': 'triangle',
				'arrow-scale': 0.8,
				'curve-style': 'bezier',
				'font-size': 10,
				'font-family': t.mono,
				color: t.muted,
				'text-background-color': t.surface,
				'text-background-opacity': 1,
				'text-background-padding': '2px'
			}
		},
		{ selector: 'edge[label]', style: { label: 'data(label)' } },
		{ selector: 'edge.rel-imports', style: { 'line-style': 'dashed' } },
		{
			selector: 'edge.rel-implements, edge.step-implements, edge.step-dispatches-to',
			style: {
				'line-color': t.implements,
				'target-arrow-color': t.implements,
				'line-style': 'dotted'
			}
		},
		{
			selector: 'edge.rel-candidate, edge.step-may-call',
			style: {
				'line-color': t.candidate,
				'target-arrow-color': t.candidate,
				'line-style': 'dashed'
			}
		},
		{
			selector: 'edge.rel-cycle',
			style: { 'line-color': t.cycle, 'target-arrow-color': t.cycle, width: 2 }
		}
	];
}
