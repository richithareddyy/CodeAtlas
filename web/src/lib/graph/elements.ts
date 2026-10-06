// Builders from API results to the generic element lists the canvas draws.

import type {
	ArchitectureGraph,
	Cycle,
	GitImpactReport,
	ImpactReport,
	Relation,
	SymbolKind
} from '../api/types';
import { kindOf, shortLabel } from '../format';
import type { GraphModel } from './model';
import { zoomModules, type ZoomGraph } from './zoom';

export interface CanvasNode {
	id: string;
	label: string;
	/** Style classes, e.g. `kind-function`, `root`, `test`, `cycle`. */
	classes: string[];
	/** Width in px, derived from the label (Cytoscape cannot size to text). */
	width: number;
	tooltip: string;
}

export interface CanvasEdge {
	id: string;
	source: string;
	target: string;
	classes: string[];
	label?: string;
}

export interface CanvasData {
	nodes: CanvasNode[];
	edges: CanvasEdge[];
	/** Nodes the tree layout starts from. */
	roots: string[];
	layout: 'tree' | 'dependency' | 'ring';
}

export const EMPTY: CanvasData = { nodes: [], edges: [], roots: [], layout: 'tree' };

function width(label: string): number {
	return Math.min(320, Math.max(48, label.length * 6.6 + 18));
}

function kindClass(kind: SymbolKind | null): string {
	return kind ? `kind-${kind.toLowerCase()}` : 'kind-file';
}

function node(id: string, label: string, classes: string[], tooltip = id): CanvasNode {
	return { id, label, classes, width: width(label), tooltip };
}

const RELATION_CLASS: Record<Relation, string> = {
	CALLS: 'rel-calls',
	CALLS_CANDIDATE: 'rel-candidate',
	IMPORTS: 'rel-imports',
	IMPLEMENTS: 'rel-implements'
};

/** The explorable neighbourhood graph. */
export function fromModel(
	model: GraphModel,
	hiddenKinds: Set<SymbolKind>,
	relations: Set<Relation>
): CanvasData {
	const { nodes, edges } = model.visible(hiddenKinds, relations);
	return {
		nodes: nodes.map((n) => {
			const classes = [kindClass(n.symbol.kind)];
			if (n.id === model.rootId) classes.push('root');
			if (n.symbol.isTest) classes.push('test');
			if (model.isExpanded(n.id) && n.id !== model.rootId) classes.push('expanded');
			return node(
				n.id,
				shortLabel(n.id),
				classes,
				`${n.symbol.qualifiedName}\n${n.symbol.file}:${n.symbol.startLine}`
			);
		}),
		edges: edges.map((e) => ({
			id: e.id,
			source: e.from,
			target: e.to,
			classes: [RELATION_CLASS[e.relation]]
		})),
		roots: model.rootId ? [model.rootId] : [],
		layout: 'tree'
	};
}

/**
 * The impact graph: changed symbols at the root, every affected symbol
 * connected through the steps of its evidence chain.
 */
export function fromImpact(report: ImpactReport): CanvasData {
	const nodes = new Map<string, CanvasNode>();
	const edges = new Map<string, CanvasEdge>();
	for (const changed of report.changed) {
		nodes.set(
			changed.id,
			node(changed.id, shortLabel(changed.id), [kindClass(changed.kind), 'root', 'changed'])
		);
	}
	for (const a of report.affected) {
		const classes = [kindClass(a.symbol.kind), `depth-${Math.min(a.depth, 4)}`];
		if (a.symbol.isTest) classes.push('test');
		if (a.confidence === 'POSSIBLE') classes.push('possible');
		nodes.set(
			a.symbol.id,
			node(
				a.symbol.id,
				shortLabel(a.symbol.id),
				classes,
				`${a.symbol.qualifiedName}\n${a.symbol.file}:${a.symbol.line}`
			)
		);
	}
	for (const a of report.affected) {
		for (const step of a.path) {
			if (!nodes.has(step.source) || !nodes.has(step.target)) continue;
			const id = `${step.source}->${step.target}:${step.kind}`;
			edges.set(id, {
				id,
				source: step.source,
				target: step.target,
				classes: [`step-${step.kind.toLowerCase().replace('_', '-')}`]
			});
		}
	}
	return {
		nodes: [...nodes.values()],
		edges: [...edges.values()],
		roots: report.changed.map((c) => c.id),
		layout: 'tree'
	};
}

/**
 * The diff impact graph: modified and removed symbols at the root, and
 * every downstream symbol connected through its evidence chain. Added and
 * moved symbols have no pre-existing dependents and are left out. With
 * `maxDepth`, deeper downstream symbols are left out too.
 */
export function fromDiff(report: GitImpactReport, maxDepth = Infinity): CanvasData {
	const downstream = report.downstream.filter((d) => d.depth <= maxDepth);
	const nodes = new Map<string, CanvasNode>();
	const edges = new Map<string, CanvasEdge>();
	const roots: string[] = [];
	for (const c of report.symbols) {
		if (c.change !== 'MODIFIED' && c.change !== 'REMOVED') continue;
		if (c.symbol.kind === 'MODULE') continue;
		const classes = [kindClass(c.symbol.kind), 'root', 'changed'];
		if (c.change === 'REMOVED') classes.push('removed');
		if (c.symbol.isTest) classes.push('test');
		const what =
			c.change === 'REMOVED' ? 'removed' : c.signature ? 'signature changed' : 'modified';
		nodes.set(
			c.symbol.id,
			node(
				c.symbol.id,
				shortLabel(c.symbol.id),
				classes,
				`${c.symbol.qualifiedName} (${what})\n${c.symbol.file}:${c.symbol.line}`
			)
		);
		roots.push(c.symbol.id);
	}
	for (const d of downstream) {
		const classes = [kindClass(d.symbol.kind), `depth-${Math.min(d.depth, 4)}`];
		if (d.symbol.isTest) classes.push('test');
		if (d.confidence === 'POSSIBLE') classes.push('possible');
		nodes.set(
			d.symbol.id,
			node(
				d.symbol.id,
				shortLabel(d.symbol.id),
				classes,
				`${d.symbol.qualifiedName}\n${d.symbol.file}:${d.symbol.line}`
			)
		);
	}
	for (const d of downstream) {
		for (const step of d.path) {
			// Chains may pass through changed symbols that are not roots
			// (e.g. methods of a changed type); show them too.
			for (const id of [step.source, step.target]) {
				if (!nodes.has(id)) {
					nodes.set(id, node(id, shortLabel(id), [kindClass(kindOf(id)), 'changed']));
				}
			}
			const id = `${step.source}->${step.target}:${step.kind}`;
			edges.set(id, {
				id,
				source: step.source,
				target: step.target,
				classes: [`step-${step.kind.toLowerCase().replace('_', '-')}`]
			});
		}
	}
	return { nodes: [...nodes.values()], edges: [...edges.values()], roots, layout: 'tree' };
}

/** Label for an architecture node ID (crate name, module ID or file path). */
export function architectureLabel(id: string): string {
	return id.startsWith('mod:') ? id.slice(4) : id;
}

/**
 * The architecture graph, optionally limited to nodes whose label starts
 * with `scope` (e.g. one crate's modules) and their direct neighbours.
 */
export function fromArchitecture(
	graph: ArchitectureGraph,
	scope: string | null,
	hiddenCrates: Set<string> = new Set()
): CanvasData {
	const inScope = (id: string) => {
		if (!scope) return true;
		const label = architectureLabel(id);
		return label === scope || label.startsWith(`${scope}::`) || label.startsWith(`${scope}/`);
	};
	// Crate-level IDs are crate names (`#N` for duplicates); module IDs
	// start with their crate. File-level nodes are not filtered.
	const crateOf = (id: string): string | null =>
		graph.level === 'FILE' ? null : architectureLabel(id).split('::')[0].replace(/#\d+$/, '');
	const visible = (id: string) => {
		const name = crateOf(id);
		return name === null || !hiddenCrates.has(name);
	};
	if (graph.level === 'MODULE') return fromZoom(zoomModules(graph, scope, visible));
	const edges = graph.edges.filter(
		(e) => (inScope(e.from) || inScope(e.to)) && visible(e.from) && visible(e.to)
	);
	const ids = new Set(edges.flatMap((e) => [e.from, e.to]));
	for (const n of graph.nodes) if (inScope(n.id) && visible(n.id)) ids.add(n.id);
	const kind = graph.level === 'CRATE' ? 'kind-crate' : 'kind-file';
	return {
		nodes: graph.nodes
			.filter((n) => ids.has(n.id))
			.map((n) => {
				const classes = [kind];
				if (n.inCycle) classes.push('cycle');
				if (!inScope(n.id)) classes.push('context');
				return node(
					n.id,
					architectureLabel(n.id),
					classes,
					`${architectureLabel(n.id)}\n${n.fanIn} dependents, ${n.fanOut} dependencies`
				);
			}),
		edges: edges.map((e) => ({
			id: `${e.from}->${e.to}`,
			source: e.from,
			target: e.to,
			classes: ['rel-depends'],
			label: e.weight > 1 ? String(e.weight) : undefined
		})),
		roots: [],
		layout: 'dependency'
	};
}

/** A zoomed module graph: groups carry their module count. */
function fromZoom(z: ZoomGraph): CanvasData {
	return {
		nodes: z.nodes.map((n) => {
			const classes = ['kind-module'];
			if (n.inCycle) classes.push('cycle');
			if (n.context) classes.push('context');
			if (n.members > 1) classes.push('group');
			const label = n.members > 1 ? `${n.label} · ${n.members}` : n.label;
			const what = n.members > 1 ? `${n.members} modules` : 'module';
			return node(
				n.id,
				label,
				classes,
				`${n.path} (${what})\n${n.fanIn} dependents, ${n.fanOut} dependencies`
			);
		}),
		edges: z.edges.map((e) => ({
			id: `${e.from}->${e.to}`,
			source: e.from,
			target: e.to,
			classes: ['rel-depends'],
			label: e.weight > 1 ? String(e.weight) : undefined
		})),
		roots: [],
		layout: 'dependency'
	};
}

/** One cycle, drawn as its hops. */
export function fromCycle(cycle: Cycle): CanvasData {
	return {
		// Hop order, so the ring reads in the direction of the cycle.
		nodes: (cycle.hops.length ? cycle.hops.map((h) => h.from) : cycle.members).map((m) =>
			node(m, kindOf(m) ? shortLabel(m) : architectureLabel(m), [kindClass(kindOf(m)), 'cycle'])
		),
		edges: cycle.hops.map((h) => ({
			id: `${h.from}->${h.to}`,
			source: h.from,
			target: h.to,
			classes: ['rel-cycle'],
			label: h.weight > 1 ? String(h.weight) : undefined
		})),
		roots: cycle.members.slice(0, 1),
		layout: 'ring'
	};
}
