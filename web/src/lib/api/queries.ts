// Typed API operations used by the UI.

import { request } from './client';
import type {
	ArchitectureGraph,
	ArchitectureLevel,
	Crate,
	Cycle,
	CycleLevel,
	Direction,
	ImpactReport,
	IndexResult,
	Neighborhood,
	Relation,
	Repository,
	SearchPage,
	SourceSnippet,
	Symbol,
	SymbolKind,
	SymbolRef
} from './types';

const SYMBOL = `id kind name qualifiedName file crateName startLine endLine visibility signature isTest
	unresolvedCalls { line callee reason }`;
const EDGE = `from to relation resolution lines`;
const REPOSITORY = `id name root originUrl branch headSha indexedSha indexedAt sourceFiles loc languages`;
const SYMBOL_REF = `id kind qualifiedName file line isTest`;

export async function repositories(): Promise<Repository[]> {
	const data = await request<{ repositories: Repository[] }>(`{ repositories { ${REPOSITORY} } }`);
	return data.repositories;
}

export async function symbol(repoId: string, id: string): Promise<Symbol | null> {
	const data = await request<{ symbol: Symbol | null }>(
		`query($repo: ID!, $id: ID!) { symbol(repoId: $repo, id: $id) { ${SYMBOL} } }`,
		{ repo: repoId, id }
	);
	return data.symbol;
}

export async function search(
	repoId: string,
	text: string,
	options: { kinds?: SymbolKind[]; first?: number; after?: string | null } = {}
): Promise<SearchPage> {
	const data = await request<{ searchSymbols: SearchPage }>(
		`query($repo: ID!, $text: String!, $kinds: [SymbolKind!], $first: Int, $after: String) {
			searchSymbols(repoId: $repo, query: $text, kinds: $kinds, first: $first, after: $after) {
				edges { cursor node { ${SYMBOL} } }
				pageInfo { hasNextPage endCursor }
			}
		}`,
		{
			repo: repoId,
			text,
			kinds: options.kinds?.length ? options.kinds : null,
			first: options.first ?? 20,
			after: options.after ?? null
		}
	);
	return data.searchSymbols;
}

export async function crates(repoId: string): Promise<Crate[]> {
	const data = await request<{ crates: Crate[] }>(
		`query($repo: ID!) { crates(repoId: $repo) { id name package kind rootFile rootModule } }`,
		{ repo: repoId }
	);
	return data.crates;
}

export async function children(repoId: string, id: string): Promise<Symbol[]> {
	const data = await request<{ children: Symbol[] }>(
		`query($repo: ID!, $id: ID!) { children(repoId: $repo, id: $id) { ${SYMBOL} } }`,
		{ repo: repoId, id }
	);
	return data.children;
}

export async function neighborhood(
	repoId: string,
	symbolId: string,
	direction: Direction,
	depth: number,
	relations: Relation[]
): Promise<Neighborhood> {
	const field = direction === 'DEPENDENTS' ? 'dependents' : 'dependencies';
	const data = await request<Record<string, Neighborhood>>(
		`query($repo: ID!, $id: ID!, $depth: Int!, $relations: [Relation!]!) {
			result: ${field}(repoId: $repo, symbolId: $id, depth: $depth, relations: $relations) {
				root { ${SYMBOL} } direction maxDepth truncated
				nodes { depth symbol { ${SYMBOL} } via { ${EDGE} } }
				edges { ${EDGE} }
			}
		}`,
		{ repo: repoId, id: symbolId, depth, relations }
	);
	return data.result;
}

export async function impact(
	repoId: string,
	symbolId: string,
	maxDepth: number,
	includeAmbiguous: boolean
): Promise<ImpactReport> {
	const data = await request<{ impact: ImpactReport }>(
		`query($repo: ID!, $id: ID!, $depth: Int!, $ambiguous: Boolean!) {
			impact(repoId: $repo, symbolId: $id, maxDepth: $depth, includeAmbiguous: $ambiguous) {
				changed { ${SYMBOL_REF} }
				maxDepth includeAmbiguous directCount indirectCount possibleCount truncated tests
				affected {
					depth confidence symbol { ${SYMBOL_REF} }
					path { source target kind file lines resolution }
				}
				files { name symbols tests }
				modules { name symbols tests }
				score { total level factors { name value saturation weight normalized contribution } }
			}
		}`,
		{ repo: repoId, id: symbolId, depth: maxDepth, ambiguous: includeAmbiguous }
	);
	return data.impact;
}

export async function cycles(repoId: string, level: CycleLevel): Promise<Cycle[]> {
	const data = await request<{ circularDependencies: Cycle[] }>(
		`query($repo: ID!, $level: Level!) {
			circularDependencies(repoId: $repo, level: $level) {
				members hops { from to weight via evidence { from to relation file line } }
			}
		}`,
		{ repo: repoId, level }
	);
	return data.circularDependencies;
}

export async function architecture(
	repoId: string,
	level: ArchitectureLevel
): Promise<ArchitectureGraph> {
	const data = await request<{ architectureGraph: ArchitectureGraph }>(
		`query($repo: ID!, $level: ArchitectureLevel!) {
			architectureGraph(repoId: $repo, level: $level) {
				level nodes { id fanIn fanOut inCycle } edges { from to weight via }
			}
		}`,
		{ repo: repoId, level }
	);
	return data.architectureGraph;
}

export async function source(
	repoId: string,
	file: string,
	startLine: number,
	endLine: number
): Promise<SourceSnippet> {
	const data = await request<{ source: SourceSnippet }>(
		`query($repo: ID!, $file: String!, $start: Int!, $end: Int!) {
			source(repoId: $repo, file: $file, startLine: $start, endLine: $end) {
				file startLine endLine totalLines lines
			}
		}`,
		{ repo: repoId, file, start: startLine, end: endLine }
	);
	return data.source;
}

export async function indexRepository(source: string): Promise<IndexResult> {
	const data = await request<{ indexRepository: IndexResult }>(
		`mutation($source: String!) {
			indexRepository(source: $source) {
				repository { ${REPOSITORY} }
				filesAnalyzed nodes relationships resolutionRate analysisMs writeMs
			}
		}`,
		{ source }
	);
	return data.indexRepository;
}

export async function affectedTests(repoId: string, symbolId: string): Promise<SymbolRef[]> {
	const data = await request<{ affectedTests: { test: SymbolRef }[] }>(
		`query($repo: ID!, $id: ID!) { affectedTests(repoId: $repo, symbolId: $id) { test { ${SYMBOL_REF} } } }`,
		{ repo: repoId, id: symbolId }
	);
	return data.affectedTests.map((t) => t.test);
}
