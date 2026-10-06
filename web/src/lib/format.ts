// Display helpers for symbol IDs, kinds and evidence.

import type { DependencyKind, EvidenceStep, Relation, SymbolKind } from './api/types';

/** `fn:shop::payments::authorize` → `shop::payments::authorize`. */
export function qualifiedName(id: string): string {
	const colon = id.indexOf(':');
	// `mod:a::b` has its first `:` before any `::`.
	return colon >= 0 && id[colon + 1] !== ':' ? id.slice(colon + 1) : id;
}

/**
 * Splits a qualified name on `::`, keeping `<Type as path::Trait>` segments
 * whole: `a::<X as std::fmt::Display>::fmt` → `['a', '<X as std::fmt::Display>', 'fmt']`.
 */
export function splitPath(path: string): string[] {
	const segments: string[] = [];
	let depth = 0;
	let current = '';
	for (let i = 0; i < path.length; i++) {
		const ch = path[i];
		if (ch === '<') depth++;
		if (ch === '>') depth = Math.max(0, depth - 1);
		if (depth === 0 && ch === ':' && path[i + 1] === ':') {
			segments.push(current);
			current = '';
			i++;
			continue;
		}
		current += ch;
	}
	segments.push(current);
	return segments;
}

/** Strips a trailing `#N` duplicate suffix for display. */
function withoutSuffix(segment: string): string {
	return segment.replace(/#\d+$/, '');
}

/**
 * Short label for a symbol ID: `Type::method` for methods, the last segment
 * otherwise, and the path for files.
 */
export function shortLabel(id: string): string {
	if (!id.includes(':')) return id; // file paths and crate names
	const kind = id.slice(0, id.indexOf(':'));
	const segments = splitPath(qualifiedName(id));
	const last = segments[segments.length - 1] ?? id;
	if (kind === 'method' && segments.length >= 2) {
		return `${withoutSuffix(segments[segments.length - 2])}::${last}`;
	}
	return last;
}

export const KIND_LABEL: Record<SymbolKind, string> = {
	MODULE: 'mod',
	STRUCT: 'struct',
	ENUM: 'enum',
	TRAIT: 'trait',
	FUNCTION: 'fn',
	METHOD: 'method'
};

/** Kind from an ID prefix (`fn:` → FUNCTION). */
export function kindOf(id: string): SymbolKind | null {
	const prefix = id.slice(0, id.indexOf(':'));
	const map: Record<string, SymbolKind> = {
		mod: 'MODULE',
		struct: 'STRUCT',
		enum: 'ENUM',
		trait: 'TRAIT',
		fn: 'FUNCTION',
		method: 'METHOD'
	};
	return map[prefix] ?? null;
}

export const RELATION_LABEL: Record<Relation, string> = {
	CALLS: 'calls',
	CALLS_CANDIDATE: 'ambiguous call',
	IMPORTS: 'imports',
	IMPLEMENTS: 'implements'
};

export function lines(lines: number[]): string {
	if (lines.length === 0) return '';
	return lines.length === 1 ? `line ${lines[0]}` : `lines ${lines.join(', ')}`;
}

/** Sentence for one evidence step: "A calls B". */
export function stepSentence(step: EvidenceStep): { source: string; verb: string; target: string } {
	const verbs: Record<DependencyKind, string> = {
		CALLS: 'calls',
		DISPATCHES_TO: 'may dispatch to',
		IMPLEMENTS: 'implements',
		MAY_CALL: 'may call (ambiguous)'
	};
	const source = shortLabel(step.source);
	const target = shortLabel(step.target);
	if (step.kind === 'DISPATCHES_TO') {
		return { source: `calls to ${source}`, verb: verbs[step.kind], target };
	}
	return { source, verb: verbs[step.kind], target };
}

export function shortSha(sha: string | null): string {
	return sha ? sha.slice(0, 10) : '';
}

/** `2026-10-03T17:15:50.65+00:00` → `2026-10-03 17:15`. */
/** `YYYY-MM-DD HH:MM` in the viewer's time zone (or the one given). */
export function timestamp(value: string, timeZone?: string): string {
	const date = new Date(value);
	if (Number.isNaN(date.getTime())) return value;
	const parts = Object.fromEntries(
		new Intl.DateTimeFormat('en-CA', {
			timeZone,
			year: 'numeric',
			month: '2-digit',
			day: '2-digit',
			hour: '2-digit',
			minute: '2-digit',
			hourCycle: 'h23'
		})
			.formatToParts(date)
			.map((part) => [part.type, part.value])
	);
	return `${parts.year}-${parts.month}-${parts.day} ${parts.hour}:${parts.minute}`;
}

export function count(n: number, noun: string, plural = `${noun}s`): string {
	return `${n.toLocaleString()} ${n === 1 ? noun : plural}`;
}

/**
 * Call steps in an evidence chain. A test is *direct* when its chain has
 * one call: dispatch and implementation steps do not count (as in the
 * server's test selection).
 */
export function callSteps(path: { kind: string }[]): number {
	return path.filter((step) => step.kind === 'CALLS' || step.kind === 'MAY_CALL').length;
}

/** Crate kinds that hold tests or examples rather than the product. */
export const SUPPORT_CRATE_KINDS = new Set(['test', 'bench', 'example']);

/**
 * Names of test, bench and example crates, unless a library or binary
 * crate has the same name.
 */
export function supportCrateNames(crates: { name: string; kind: string }[]): Set<string> {
	const product = new Set(
		crates.filter((c) => !SUPPORT_CRATE_KINDS.has(c.kind)).map((c) => c.name)
	);
	return new Set(
		crates.filter((c) => SUPPORT_CRATE_KINDS.has(c.kind) && !product.has(c.name)).map((c) => c.name)
	);
}

export type TextPart =
	| { kind: 'text'; value: string }
	| { kind: 'code'; value: string }
	| { kind: 'cite'; ids: string[] };

/**
 * Splits explanation text into plain text, `code` and citations such as
 * `[E1]` or `[E1, E2]`.
 */
export function explanationParts(text: string): TextPart[] {
	const parts: TextPart[] = [];
	const pattern = /`([^`]+)`|\[(E\d+(?:\s*,\s*E\d+)*)\]/g;
	let last = 0;
	for (const match of text.matchAll(pattern)) {
		const at = match.index ?? 0;
		if (at > last) parts.push({ kind: 'text', value: text.slice(last, at) });
		if (match[1] !== undefined) parts.push({ kind: 'code', value: match[1] });
		else parts.push({ kind: 'cite', ids: match[2].split(',').map((id) => id.trim()) });
		last = at + match[0].length;
	}
	if (last < text.length) parts.push({ kind: 'text', value: text.slice(last) });
	return parts;
}
