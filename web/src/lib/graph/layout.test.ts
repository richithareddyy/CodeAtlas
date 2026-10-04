import { describe, expect, it } from 'vitest';
import { dependencyLayers, layered, layers } from './layout';

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
