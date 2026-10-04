// Client-side state of the explorable symbol graph.
//
// The canvas never shows the whole repository: it starts from one symbol's
// neighbourhood, and the user expands or collapses individual nodes. The
// model records which expansion added each node, so collapsing removes
// exactly what an expansion brought in, without touching nodes that other
// expansions (or the root) also need.

import type { GraphEdge, Neighborhood, Relation, Symbol, SymbolKind } from '../api/types';

export interface ModelNode {
	id: string;
	symbol: Symbol;
	/** Distance from the focused symbol when first added. */
	depth: number;
}

export interface ModelEdge extends GraphEdge {
	id: string;
}

export function edgeId(edge: GraphEdge): string {
	return `${edge.from}->${edge.to}:${edge.relation}`;
}

export class GraphModel {
	readonly nodes = new Map<string, ModelNode>();
	readonly edges = new Map<string, ModelEdge>();
	/** Node → expansions (node IDs) that added it; the root has none. */
	private readonly addedBy = new Map<string, Set<string>>();
	/** Expansion origin → nodes it added. */
	private readonly expansions = new Map<string, Set<string>>();
	rootId: string | null = null;
	truncated = false;

	/** A model holding one neighbourhood, focused on its root. */
	static focus(neighborhood: Neighborhood): GraphModel {
		const model = new GraphModel();
		model.rootId = neighborhood.root.id;
		model.nodes.set(neighborhood.root.id, {
			id: neighborhood.root.id,
			symbol: neighborhood.root,
			depth: 0
		});
		model.merge(neighborhood.root.id, neighborhood, 0);
		return model;
	}

	/** Adds a neighbourhood fetched for `origin`; returns the IDs it added. */
	merge(origin: string, neighborhood: Neighborhood, baseDepth?: number): string[] {
		const base = baseDepth ?? this.nodes.get(origin)?.depth ?? 0;
		const added: string[] = [];
		const additions = this.expansions.get(origin) ?? new Set<string>();
		for (const node of neighborhood.nodes) {
			if (node.symbol.id === this.rootId) continue;
			if (!this.nodes.has(node.symbol.id)) {
				this.nodes.set(node.symbol.id, {
					id: node.symbol.id,
					symbol: node.symbol,
					depth: base + node.depth
				});
				added.push(node.symbol.id);
			}
			if (node.symbol.id !== origin) {
				additions.add(node.symbol.id);
				const by = this.addedBy.get(node.symbol.id) ?? new Set<string>();
				by.add(origin);
				this.addedBy.set(node.symbol.id, by);
			}
		}
		this.expansions.set(origin, additions);
		for (const edge of neighborhood.edges) {
			if (this.nodes.has(edge.from) && this.nodes.has(edge.to)) {
				this.edges.set(edgeId(edge), { ...edge, id: edgeId(edge) });
			}
		}
		this.truncated ||= neighborhood.truncated;
		return added;
	}

	isExpanded(id: string): boolean {
		return this.expansions.has(id);
	}

	/**
	 * Removes what expanding `id` added, recursively collapsing those nodes'
	 * own expansions. Nodes still added by another expansion stay.
	 */
	collapse(id: string): string[] {
		const removed: string[] = [];
		const additions = this.expansions.get(id);
		this.expansions.delete(id);
		if (!additions) return removed;
		for (const child of additions) {
			const by = this.addedBy.get(child);
			by?.delete(id);
			if (child === this.rootId || (by && by.size > 0)) continue;
			removed.push(...this.collapse(child));
			this.addedBy.delete(child);
			this.nodes.delete(child);
			removed.push(child);
		}
		for (const [key, edge] of this.edges) {
			if (!this.nodes.has(edge.from) || !this.nodes.has(edge.to)) this.edges.delete(key);
		}
		return removed;
	}

	/** Nodes and edges passing the kind and relation filters. The root is always shown. */
	visible(hiddenKinds: Set<SymbolKind>, relations: Set<Relation>) {
		const nodes = [...this.nodes.values()].filter(
			(n) => n.id === this.rootId || !hiddenKinds.has(n.symbol.kind)
		);
		const ids = new Set(nodes.map((n) => n.id));
		const edges = [...this.edges.values()].filter(
			(e) => relations.has(e.relation) && ids.has(e.from) && ids.has(e.to)
		);
		return { nodes, edges };
	}
}
