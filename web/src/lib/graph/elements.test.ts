import { describe, expect, it } from 'vitest';
import type { ArchitectureGraph, ImpactReport } from '../api/types';
import { fromArchitecture, fromCycle, fromImpact } from './elements';

const report: ImpactReport = {
	changed: [
		{
			id: 'fn:app::validate',
			kind: 'FUNCTION',
			qualifiedName: 'app::validate',
			file: 'src/a.rs',
			line: 3,
			isTest: false
		}
	],
	maxDepth: 8,
	includeAmbiguous: true,
	directCount: 1,
	indirectCount: 1,
	possibleCount: 1,
	truncated: false,
	tests: ['fn:t::checks'],
	files: [],
	modules: [],
	score: { total: 10, level: 'LOW', factors: [] },
	affected: [
		{
			depth: 1,
			confidence: 'CERTAIN',
			symbol: {
				id: 'method:app::S::run',
				kind: 'METHOD',
				qualifiedName: 'app::S::run',
				file: 'src/a.rs',
				line: 9,
				isTest: false
			},
			path: [
				{
					source: 'method:app::S::run',
					target: 'fn:app::validate',
					kind: 'CALLS',
					file: 'src/a.rs',
					lines: [10],
					resolution: 'scope'
				}
			]
		},
		{
			depth: 2,
			confidence: 'CERTAIN',
			symbol: {
				id: 'fn:t::checks',
				kind: 'FUNCTION',
				qualifiedName: 't::checks',
				file: 'tests/t.rs',
				line: 4,
				isTest: true
			},
			path: [
				{
					source: 'fn:t::checks',
					target: 'method:app::S::run',
					kind: 'CALLS',
					file: 'tests/t.rs',
					lines: [5],
					resolution: 'import'
				},
				{
					source: 'method:app::S::run',
					target: 'fn:app::validate',
					kind: 'CALLS',
					file: 'src/a.rs',
					lines: [10],
					resolution: 'scope'
				}
			]
		},
		{
			depth: 1,
			confidence: 'POSSIBLE',
			symbol: {
				id: 'fn:app::maybe',
				kind: 'FUNCTION',
				qualifiedName: 'app::maybe',
				file: 'src/b.rs',
				line: 1,
				isTest: false
			},
			path: [
				{
					source: 'fn:app::maybe',
					target: 'fn:app::validate',
					kind: 'MAY_CALL',
					file: 'src/b.rs',
					lines: [2],
					resolution: null
				}
			]
		}
	]
};

describe('fromImpact', () => {
	it('connects affected symbols through their evidence steps', () => {
		const data = fromImpact(report);
		expect(data.roots).toEqual(['fn:app::validate']);
		expect(data.nodes.map((n) => n.id).sort()).toEqual([
			'fn:app::maybe',
			'fn:app::validate',
			'fn:t::checks',
			'method:app::S::run'
		]);
		// Shared steps are drawn once.
		expect(data.edges).toHaveLength(3);
		const classes = Object.fromEntries(data.nodes.map((n) => [n.id, n.classes]));
		expect(classes['fn:t::checks']).toContain('test');
		expect(classes['fn:app::maybe']).toContain('possible');
		expect(classes['fn:app::validate']).toContain('changed');
		expect(data.edges.find((e) => e.source === 'fn:app::maybe')?.classes).toEqual([
			'step-may-call'
		]);
	});
});

describe('fromArchitecture', () => {
	const graph: ArchitectureGraph = {
		level: 'MODULE',
		nodes: [
			{ id: 'mod:app', fanIn: 0, fanOut: 1, inCycle: false },
			{ id: 'mod:app::db', fanIn: 2, fanOut: 1, inCycle: true },
			{ id: 'mod:app::api', fanIn: 1, fanOut: 1, inCycle: true },
			{ id: 'mod:tool', fanIn: 0, fanOut: 1, inCycle: false },
			{ id: 'mod:other', fanIn: 0, fanOut: 0, inCycle: false }
		],
		edges: [
			{ from: 'mod:app', to: 'mod:app::db', weight: 1, via: ['IMPORTS'] },
			{ from: 'mod:app::db', to: 'mod:app::api', weight: 3, via: ['CALLS'] },
			{ from: 'mod:app::api', to: 'mod:app::db', weight: 1, via: ['CALLS'] },
			{ from: 'mod:tool', to: 'mod:app::api', weight: 2, via: ['CALLS'] }
		]
	};

	it('marks cycles and labels weights', () => {
		const data = fromArchitecture(graph, null);
		expect(data.nodes).toHaveLength(5);
		expect(data.nodes.find((n) => n.id === 'mod:app::db')?.classes).toContain('cycle');
		expect(data.edges.find((e) => e.id === 'mod:app::db->mod:app::api')?.label).toBe('3');
		expect(data.edges.find((e) => e.id === 'mod:app->mod:app::db')?.label).toBeUndefined();
	});

	it('scopes to one crate and keeps outside neighbours as context', () => {
		const data = fromArchitecture(graph, 'tool');
		const byId = Object.fromEntries(data.nodes.map((n) => [n.id, n.classes]));
		expect(Object.keys(byId).sort()).toEqual(['mod:app::api', 'mod:tool']);
		expect(byId['mod:app::api']).toContain('context');
		expect(byId['mod:tool']).not.toContain('context');
	});
});

describe('fromCycle', () => {
	it('orders ring nodes along the hops', () => {
		const data = fromCycle({
			members: ['mod:a', 'mod:b', 'mod:c'],
			hops: [
				{ from: 'mod:a', to: 'mod:c', weight: 1, via: [], evidence: [] },
				{ from: 'mod:c', to: 'mod:b', weight: 2, via: [], evidence: [] },
				{ from: 'mod:b', to: 'mod:a', weight: 1, via: [], evidence: [] }
			]
		});
		expect(data.nodes.map((n) => n.id)).toEqual(['mod:a', 'mod:c', 'mod:b']);
		expect(data.edges.map((e) => e.label)).toEqual([undefined, '2', undefined]);
		expect(data.layout).toBe('ring');
	});
});
