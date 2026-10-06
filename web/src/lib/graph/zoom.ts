// Conceptual zoom for the module graph: inside a scope (a crate or a
// module), each submodule tree is drawn as one node, so a crate with
// hundreds of modules shows its top-level structure first. Double-clicking
// a group zooms into it.

import type { ArchitectureGraph } from '../api/types';

export interface ZoomNode {
	/** `group:<path>` for a module tree, the module ID for a single module. */
	id: string;
	/** Full module path of the group (`tokio::runtime`). */
	path: string;
	/** Label relative to the scope (`runtime`), or the full path for context. */
	label: string;
	/** Modules the node stands for. */
	members: number;
	/** Some member is part of a dependency cycle. */
	inCycle: boolean;
	/** Outside the scope (shown because something inside depends on it). */
	context: boolean;
	fanIn: number;
	fanOut: number;
}

export interface ZoomGraph {
	nodes: ZoomNode[];
	edges: { from: string; to: string; weight: number }[];
}

export const GROUP = 'group:';

function moduleLabel(id: string): string {
	return id.startsWith('mod:') ? id.slice(4) : id;
}

/** The group a module path belongs to at this zoom level. */
export function groupOf(path: string, scope: string | null): string {
	const segments = path.split('::');
	if (!scope) return segments.slice(0, 2).join('::');
	if (path === scope) return scope;
	if (path.startsWith(`${scope}::`)) {
		return `${scope}::${path.slice(scope.length + 2).split('::')[0]}`;
	}
	// Outside the scope: one level below the common ancestor.
	const scoped = scope.split('::');
	let common = 0;
	while (common < scoped.length && segments[common] === scoped[common]) common++;
	return segments.slice(0, common + 1).join('::');
}

/**
 * Aggregates a module-level graph for `scope`. Without a scope, modules are
 * grouped by their crate's top-level modules.
 */
export function zoomModules(
	graph: ArchitectureGraph,
	scope: string | null,
	visible: (id: string) => boolean = () => true
): ZoomGraph {
	const inScope = (path: string) => !scope || path === scope || path.startsWith(`${scope}::`);
	const members = new Map<string, string[]>();
	const cycles = new Set<string>();
	const keyOf = new Map<string, string>();
	for (const n of graph.nodes) {
		if (!visible(n.id)) continue;
		const key = groupOf(moduleLabel(n.id), scope);
		keyOf.set(n.id, key);
		members.set(key, [...(members.get(key) ?? []), n.id]);
		if (n.inCycle) cycles.add(key);
	}

	const weights = new Map<string, number>();
	for (const e of graph.edges) {
		const from = keyOf.get(e.from);
		const to = keyOf.get(e.to);
		if (!from || !to || from === to) continue;
		if (!inScope(from) && !inScope(to)) continue;
		const key = `${from}\u0000${to}`;
		weights.set(key, (weights.get(key) ?? 0) + e.weight);
	}
	const edges = [...weights].map(([key, weight]) => {
		const [from, to] = key.split('\u0000');
		return { from, to, weight };
	});

	const used = new Set(edges.flatMap((e) => [e.from, e.to]));
	const nodes: ZoomNode[] = [];
	for (const [path, ids] of members) {
		const context = !inScope(path);
		// Context groups appear only through their edges.
		if (context && !used.has(path)) continue;
		// A group that is exactly one module without submodules is that module.
		const single = ids.length === 1 && moduleLabel(ids[0]) === path;
		nodes.push({
			id: single ? ids[0] : `${GROUP}${path}`,
			path,
			label: context
				? path
				: path === scope
					? `${path.split('::').pop()} (root)`
					: scope
						? path.slice(scope.length + 2)
						: path,
			members: ids.length,
			inCycle: cycles.has(path),
			context,
			fanIn: edges.filter((e) => e.to === path).length,
			fanOut: edges.filter((e) => e.from === path).length
		});
	}
	const idOf = new Map(nodes.map((n) => [n.path, n.id]));
	return {
		nodes,
		edges: edges
			.filter((e) => idOf.has(e.from) && idOf.has(e.to))
			.map((e) => ({ from: idOf.get(e.from)!, to: idOf.get(e.to)!, weight: e.weight }))
	};
}
