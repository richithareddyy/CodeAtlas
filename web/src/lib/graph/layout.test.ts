import { describe, expect, it } from 'vitest';
import { dependencyLayers, layered, layers, place } from './layout';

const node = (id: string, width = 80) => ({ id, width });

describe('layered layout', () => {
	it('places nodes by distance from the root and orders by parent position', () => {
		const nodes = ['root', 'a', 'b', 'b1', 'a1'].map((id) => node(id));
		const edges = [
			{ source: 'a', target: 'root' },
			{ source: 'b', target: 'root' },
			{ source: 'a1', target: 'a' },
			{ source: 'b1', target: 'b' }
		];
		expect(layers(nodes, edges, ['root'])).toEqual([['root'], ['a', 'b'], ['a1', 'b1']]);
	});

	it('keeps disconnected nodes and ignores edges to unknown nodes', () => {
		const nodes = [node('root'), node('lonely')];
		const edges = [{ source: 'root', target: 'missing' }];
		expect(layers(nodes, edges, ['root'])).toEqual([['root', 'lonely']]);
	});

	it('spaces horizontal columns by the widest node and centres rows', () => {
		const positions = layered(
			[node('root', 100), node('a', 60), node('b', 200)],
			[
				{ source: 'a', target: 'root' },
				{ source: 'b', target: 'root' }
			],
			['root'],
			'horizontal'
		);
		expect(positions.root).toEqual({ x: 50, y: 0 });
		expect(positions.a.x).toBe(positions.b.x);
		expect(positions.a.x - 100).toBe(56 + 100);
		expect(positions.a.y).toBe(-positions.b.y);
	});

	it('puts dependents above dependencies and tolerates cycles', () => {
		const nodes = ['tests', 'checkout', 'payments', 'gateway', 'util'].map((id) => node(id));
		const edges = [
			{ source: 'tests', target: 'checkout' },
			{ source: 'checkout', target: 'payments' },
			{ source: 'tests', target: 'payments' },
			{ source: 'payments', target: 'gateway' },
			{ source: 'gateway', target: 'payments' }, // cycle
			{ source: 'gateway', target: 'util' }
		];
		const result = dependencyLayers(nodes, edges);
		const layerOf = (id: string) => result.findIndex((layer) => layer.includes(id));
		expect(result.flat().sort()).toEqual(nodes.map((n) => n.id).sort());
		expect(layerOf('tests')).toBe(0);
		expect(layerOf('checkout')).toBe(1);
		expect(layerOf('payments')).toBe(2);
		expect(layerOf('gateway')).toBe(3);
		expect(layerOf('util')).toBe(4);
	});

	it('lays vertical layers out side by side without overlap', () => {
		const positions = layered(
			[node('root'), node('a', 120), node('b', 40)],
			[
				{ source: 'a', target: 'root' },
				{ source: 'b', target: 'root' }
			],
			['root'],
			'vertical'
		);
		expect(positions.a.y).toBeGreaterThan(positions.root.y);
		expect(positions.b.x - 20 - (positions.a.x + 60)).toBe(16);
	});
});

describe('wrapping', () => {
	const nodes = ['a', 'b', 'c', 'd', 'e'].map((id) => node(id, 100));

	it('wraps a wide row into several rows', () => {
		const positions = place(nodes, [['a', 'b', 'c', 'd', 'e']], 'vertical', 250);
		// Two nodes (100 + 16 + 100) fit in 250; three do not.
		const rows = new Set(Object.values(positions).map((p) => p.y));
		expect(rows.size).toBe(3);
		expect(positions.a.y).toBe(positions.b.y);
		expect(positions.c.y).toBeGreaterThan(positions.a.y);
	});

	it('wraps a tall column into several columns', () => {
		const positions = place(nodes, [['a', 'b', 'c', 'd', 'e']], 'horizontal', 100);
		// A column holds floor((100 + 16) / 40) = 2 nodes.
		const columns = new Set(Object.values(positions).map((p) => p.x));
		expect(columns.size).toBe(3);
	});

	it('keeps one row or column without a limit', () => {
		const positions = place(nodes, [['a', 'b', 'c', 'd', 'e']], 'vertical');
		expect(new Set(Object.values(positions).map((p) => p.y)).size).toBe(1);
	});
});
