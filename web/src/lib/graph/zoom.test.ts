import { describe, expect, it } from 'vitest';
import type { ArchitectureGraph } from '../api/types';
import { groupOf, zoomModules } from './zoom';

const node = (path: string, inCycle = false) => ({
	id: `mod:${path}`,
	fanIn: 0,
	fanOut: 0,
	inCycle
});
const edge = (from: string, to: string, weight = 1) => ({
	from: `mod:${from}`,
	to: `mod:${to}`,
	weight,
	via: []
});

const graph = {
	level: 'MODULE',
	nodes: [
		node('app'),
		node('app::runtime'),
		node('app::runtime::scheduler', true),
		node('app::runtime::driver'),
		node('app::sync'),
		node('app::sync::mpsc', true),
		node('app::util'),
		node('other')
	],
	edges: [
		edge('app::runtime::scheduler', 'app::sync::mpsc', 3),
		edge('app::runtime::driver', 'app::sync', 2),
		edge('app::runtime::scheduler', 'app::runtime::driver'),
		edge('app::sync::mpsc', 'app::runtime::scheduler'),
		edge('app::util', 'other')
	]
} as unknown as ArchitectureGraph;

describe('module zoom', () => {
	it('groups by the level below the scope', () => {
		expect(groupOf('app::runtime::scheduler', 'app')).toBe('app::runtime');
		expect(groupOf('app', 'app')).toBe('app');
		expect(groupOf('app::sync::mpsc', 'app::runtime')).toBe('app::sync');
		expect(groupOf('other::x', 'app::runtime')).toBe('other');
		expect(groupOf('app::runtime::scheduler', null)).toBe('app::runtime');
	});

	it('aggregates module trees inside a crate and sums the weights', () => {
		const z = zoomModules(graph, 'app');
		const byPath = Object.fromEntries(z.nodes.map((n) => [n.path, n]));
		expect(byPath['app::runtime']).toMatchObject({
			id: 'group:app::runtime',
			label: 'runtime',
			members: 3,
			inCycle: true
		});
		// A module without submodules stays itself and opens like one.
		expect(byPath['app::util']).toMatchObject({ id: 'mod:app::util', members: 1 });
		expect(byPath['other']).toMatchObject({ context: true });
		const edges = z.edges.map((e) => `${e.from} -> ${e.to} ${e.weight}`).sort();
		expect(edges).toEqual([
			'group:app::runtime -> group:app::sync 5',
			'group:app::sync -> group:app::runtime 1',
			'mod:app::util -> mod:other 1'
		]);
	});

	it('zooms into a group and shows outside neighbours as context', () => {
		const z = zoomModules(graph, 'app::runtime');
		const labels = z.nodes.map((n) => n.label).sort();
		expect(labels).toEqual(['app::sync', 'driver', 'runtime (root)', 'scheduler']);
		expect(z.nodes.find((n) => n.path === 'app::sync')?.context).toBe(true);
		expect(z.edges).toContainEqual({
			from: 'mod:app::runtime::scheduler',
			to: 'group:app::sync',
			weight: 3
		});
	});
});
