import { describe, expect, it } from 'vitest';
import type { GraphEdge, Neighborhood, Symbol } from '../api/types';
import { GraphModel } from './model';

function sym(id: string, kind: Symbol['kind'] = 'FUNCTION'): Symbol {
	return {
		id,
		kind,
		name: id,
		qualifiedName: id,
		file: 'src/lib.rs',
		crateName: 'app',
		startLine: 1,
		endLine: 1,
		visibility: 'pub',
		signature: null,
		isTest: false,
		unresolvedCalls: []
	};
}

function calls(from: string, to: string): GraphEdge {
	return { from, to, relation: 'CALLS', resolution: 'scope', lines: [1] };
}

/** Callers of `root`: each `[caller, callee, depth]`. */
function dependents(root: string, links: [string, string, number][]): Neighborhood {
	return {
		root: sym(root),
		direction: 'DEPENDENTS',
		maxDepth: 3,
		truncated: false,
		nodes: links.map(([from, to, depth]) => ({ symbol: sym(from), depth, via: calls(from, to) })),
		edges: links.map(([from, to]) => calls(from, to))
	};
}

describe('GraphModel', () => {
	it('focuses on a neighbourhood with depths', () => {
		const model = GraphModel.focus(
			dependents('a', [
				['b', 'a', 1],
				['c', 'b', 2]
			])
		);
		expect([...model.nodes.keys()].sort()).toEqual(['a', 'b', 'c']);
		expect(model.nodes.get('c')?.depth).toBe(2);
		expect(model.edges.size).toBe(2);
	});

	it('expands and collapses without removing shared nodes', () => {
		const model = GraphModel.focus(dependents('a', [['b', 'a', 1]]));
		// Expanding b adds c and d; d is also a caller of a.
		const added = model.merge(
			'b',
			dependents('b', [
				['c', 'b', 1],
				['d', 'b', 1]
			])
		);
		expect(added.sort()).toEqual(['c', 'd']);
		expect(model.nodes.get('c')?.depth).toBe(2);
		model.merge('a', dependents('a', [['d', 'a', 1]]));

		const removed = model.collapse('b');
		expect(removed).toEqual(['c']);
		expect(model.nodes.has('d')).toBe(true);
		expect(model.nodes.has('b')).toBe(true);
		expect([...model.edges.keys()].some((k) => k.startsWith('c->'))).toBe(false);
	});

	it('collapses nested expansions recursively and never removes the root', () => {
		const model = GraphModel.focus(dependents('a', [['b', 'a', 1]]));
		model.merge('b', dependents('b', [['c', 'b', 1]]));
		model.merge('c', dependents('c', [['a', 'c', 1]])); // cycle back to the root
		expect(model.collapse('a').sort()).toEqual(['b', 'c']);
		expect([...model.nodes.keys()]).toEqual(['a']);
	});

	it('filters by kind and relation but always shows the root', () => {
		const hood = dependents('a', [['T', 'a', 1]]);
		hood.root = sym('a', 'TRAIT');
		hood.nodes[0].symbol = sym('T', 'STRUCT');
		const model = GraphModel.focus(hood);
		const hidden = model.visible(new Set(['TRAIT', 'STRUCT']), new Set(['CALLS']));
		expect(hidden.nodes.map((n) => n.id)).toEqual(['a']);
		expect(hidden.edges).toEqual([]);
		const noCalls = model.visible(new Set(), new Set(['IMPORTS']));
		expect(noCalls.nodes).toHaveLength(2);
		expect(noCalls.edges).toEqual([]);
	});
});
