import { describe, expect, it } from 'vitest';
import {
	callSteps,
	kindOf,
	qualifiedName,
	shortLabel,
	splitPath,
	stepSentence,
	supportCrateNames,
	timestamp
} from './format';

describe('symbol labels', () => {
	it('strips the kind prefix', () => {
		expect(qualifiedName('fn:shop::payments::authorize')).toBe('shop::payments::authorize');
		expect(qualifiedName('mod:shop')).toBe('shop');
	});

	it('keeps qualified trait paths together', () => {
		expect(splitPath('a::<Money as std::fmt::Display>::fmt')).toEqual([
			'a',
			'<Money as std::fmt::Display>',
			'fmt'
		]);
	});

	it('labels methods with their owner', () => {
		expect(shortLabel('method:shop::payments::PaymentService::authorize')).toBe(
			'PaymentService::authorize'
		);
		expect(shortLabel('method:shop::gw::<Stripe as Gateway>::charge')).toBe(
			'<Stripe as Gateway>::charge'
		);
		expect(shortLabel('fn:shop::payments::fee#2')).toBe('fee#2');
		expect(shortLabel('src/lib.rs')).toBe('src/lib.rs');
		expect(shortLabel('mod:shop::payments')).toBe('payments');
	});

	it('derives kinds from ID prefixes', () => {
		expect(kindOf('method:a::B::c')).toBe('METHOD');
		expect(kindOf('src/lib.rs')).toBeNull();
	});
});

describe('evidence', () => {
	it('phrases dispatch steps from the caller side', () => {
		const sentence = stepSentence({
			source: 'method:g::Gateway::charge',
			target: 'method:g::<Stripe as Gateway>::charge',
			kind: 'DISPATCHES_TO',
			file: 'src/g.rs',
			lines: [9],
			resolution: 'impl_block'
		});
		expect(sentence).toEqual({
			source: 'calls to Gateway::charge',
			verb: 'may dispatch to',
			target: '<Stripe as Gateway>::charge'
		});
	});

	it('formats timestamps compactly', () => {
		const value = '2026-10-03T17:15:50.653706+00:00';
		expect(timestamp(value, 'UTC')).toBe('2026-10-03 17:15');
		expect(timestamp(value, 'America/Phoenix')).toBe('2026-10-03 10:15');
		expect(timestamp('not a date')).toBe('not a date');
	});
});

describe('callSteps', () => {
	it('counts calls but not dispatch or implementation steps', () => {
		expect(callSteps([{ kind: 'CALLS' }, { kind: 'DISPATCHES_TO' }])).toBe(1);
		expect(callSteps([{ kind: 'CALLS' }, { kind: 'MAY_CALL' }, { kind: 'IMPLEMENTS' }])).toBe(2);
		expect(callSteps([])).toBe(0);
	});
});

describe('supportCrateNames', () => {
	it('names test, bench and example crates unless a product crate shares the name', () => {
		const names = supportCrateNames([
			{ name: 'tokio', kind: 'lib' },
			{ name: 'sync_mpsc', kind: 'bench' },
			{ name: 'chat', kind: 'example' },
			{ name: 'tokio', kind: 'test' }
		]);
		expect([...names].sort()).toEqual(['chat', 'sync_mpsc']);
	});
});
