// Layered layouts. Trees rooted at the focused symbol put each node in the
// layer of its shortest (undirected) distance from a root; dependency graphs
// put dependents above their dependencies (longest-path layering, with
// cycle edges ignored). Within a layer, nodes are ordered by the mean
// position of their neighbours in the previous layer to reduce crossings.

export interface LayoutNode {
	id: string;
	width: number;
}

export interface LayoutEdge {
	source: string;
	target: string;
}

export type Orientation = 'horizontal' | 'vertical';

export type Positions = Record<string, { x: number; y: number }>;

const NODE_HEIGHT = 24;
const ROW_GAP = 16;
const LAYER_GAP = 56;

function adjacency(nodes: LayoutNode[], edges: LayoutEdge[]) {
	const ids = new Set(nodes.map((n) => n.id));
	const out = new Map<string, string[]>();
	const both = new Map<string, string[]>();
	for (const id of ids) {
		out.set(id, []);
		both.set(id, []);
	}
	for (const e of edges) {
		if (!ids.has(e.source) || !ids.has(e.target) || e.source === e.target) continue;
		out.get(e.source)!.push(e.target);
		both.get(e.source)!.push(e.target);
		both.get(e.target)!.push(e.source);
	}
	return { ids, out, both };
}

/** Assigns every node to a layer by breadth-first distance from the roots. */
export function layers(nodes: LayoutNode[], edges: LayoutEdge[], roots: string[]): string[][] {
	const { ids, both } = adjacency(nodes, edges);
	const seen = new Set<string>();
	const result: string[][] = [];
	// Nodes unreachable from the roots start their own component.
	const starts = [...roots.filter((r) => ids.has(r)), ...nodes.map((n) => n.id)];
	for (const start of starts) {
		if (seen.has(start)) continue;
		seen.add(start);
		let frontier = [start];
		let depth = 0;
		while (frontier.length) {
			(result[depth] ??= []).push(...frontier);
			const next: string[] = [];
			for (const id of frontier) {
				for (const other of both.get(id)!) {
					if (seen.has(other)) continue;
					seen.add(other);
					next.push(other);
				}
			}
			frontier = next;
			depth++;
		}
	}
	return orderByNeighbours(result, both);
}

/**
 * Longest-path layering of a directed graph: sources (nothing depends on
 * them) on layer 0, and every edge pointing to a later layer. Edges that
 * close a cycle are found by depth-first search and ignored.
 */
export function dependencyLayers(nodes: LayoutNode[], edges: LayoutEdge[]): string[][] {
	const { out, both } = adjacency(nodes, edges);
	const state = new Map<string, 'open' | 'done'>();
	const order: string[] = [];
	const forward = new Map<string, string[]>();

	for (const n of nodes) {
		if (state.has(n.id)) continue;
		// Iterative DFS keeping an explicit iterator per frame.
		const stack: [string, number][] = [[n.id, 0]];
		state.set(n.id, 'open');
		forward.set(n.id, []);
		while (stack.length) {
			const frame = stack[stack.length - 1];
			const [id, index] = frame;
			const targets = out.get(id)!;
			if (index < targets.length) {
				frame[1]++;
				const target = targets[index];
				const seen = state.get(target);
				if (seen === 'open') continue; // back edge: part of a cycle
				forward.get(id)!.push(target);
				if (!seen) {
					state.set(target, 'open');
					forward.set(target, []);
					stack.push([target, 0]);
				}
			} else {
				state.set(id, 'done');
				order.push(id);
				stack.pop();
			}
		}
	}

	// Reverse post-order is a topological order of the acyclic edges.
	const layerOf = new Map<string, number>();
	for (const id of order.reverse()) {
		const layer = layerOf.get(id) ?? 0;
		layerOf.set(id, layer);
		for (const target of forward.get(id)!) {
			layerOf.set(target, Math.max(layerOf.get(target) ?? 0, layer + 1));
		}
	}

	const result: string[][] = [];
	for (const n of nodes) (result[layerOf.get(n.id)!] ??= []).push(n.id);
	return orderByNeighbours(
		result.filter((layer) => layer?.length),
		both
	);
}

function orderByNeighbours(result: string[][], neighbours: Map<string, string[]>): string[][] {
	for (let depth = 1; depth < result.length; depth++) {
		const previous = new Map(result[depth - 1].map((id, index) => [id, index]));
		const score = new Map<string, number>();
		for (const id of result[depth]) {
			const positions = neighbours
				.get(id)!
				.map((other) => previous.get(other))
				.filter((p): p is number => p !== undefined);
			score.set(
				id,
				positions.length ? positions.reduce((a, b) => a + b, 0) / positions.length : Infinity
			);
		}
		result[depth].sort((a, b) => score.get(a)! - score.get(b)!);
	}
	return result;
}

/**
 * Positions layers as columns (horizontal) or rows (vertical). A layer
 * longer than `wrap` (layout units) continues in further columns or rows,
 * so that wide layers do not shrink the whole drawing.
 */
export function place(
	nodes: LayoutNode[],
	layered: string[][],
	orientation: Orientation,
	wrap = Number.POSITIVE_INFINITY
): Positions {
	const width = new Map(nodes.map((n) => [n.id, n.width]));
	const positions: Positions = {};
	let offset = 0;
	for (const layer of layered) {
		if (orientation === 'horizontal') {
			const step = NODE_HEIGHT + ROW_GAP;
			const perColumn = Math.max(1, Math.floor((wrap + ROW_GAP) / step));
			for (let i = 0; i < layer.length; i += perColumn) {
				const column = layer.slice(i, i + perColumn);
				const columnWidth = Math.max(...column.map((id) => width.get(id)!));
				const top = (-(column.length - 1) * step) / 2;
				column.forEach((id, index) => {
					positions[id] = { x: offset + columnWidth / 2, y: top + index * step };
				});
				const last = i + perColumn >= layer.length;
				offset += columnWidth + (last ? LAYER_GAP : ROW_GAP);
			}
		} else {
			for (const row of wrapRow(layer, width, wrap)) {
				const widths = row.map((id) => width.get(id)!);
				const total = widths.reduce((a, b) => a + b, 0) + ROW_GAP * (row.length - 1);
				let x = -total / 2;
				row.forEach((id, index) => {
					positions[id] = { x: x + widths[index] / 2, y: offset };
					x += widths[index] + ROW_GAP;
				});
				offset += NODE_HEIGHT + ROW_GAP;
			}
			offset += LAYER_GAP - ROW_GAP;
		}
	}
	return positions;
}

/** Splits a row of nodes so that no part is wider than `wrap`. */
function wrapRow(layer: string[], width: Map<string, number>, wrap: number): string[][] {
	const rows: string[][] = [];
	let current: string[] = [];
	let used = 0;
	for (const id of layer) {
		const w = width.get(id)!;
		if (current.length && used + ROW_GAP + w > wrap) {
			rows.push(current);
			current = [];
			used = 0;
		}
		used += (current.length ? ROW_GAP : 0) + w;
		current.push(id);
	}
	if (current.length) rows.push(current);
	return rows;
}

/** Tree layout from the given roots. */
export function layered(
	nodes: LayoutNode[],
	edges: LayoutEdge[],
	roots: string[],
	orientation: Orientation
): Positions {
	return place(nodes, layers(nodes, edges, roots), orientation);
}
