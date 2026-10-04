// TypeScript shapes of the GraphQL results the UI requests.
// Field names follow docs/schema.graphql.

export type SymbolKind = 'MODULE' | 'STRUCT' | 'ENUM' | 'TRAIT' | 'FUNCTION' | 'METHOD';
export type Relation = 'CALLS' | 'CALLS_CANDIDATE' | 'IMPORTS' | 'IMPLEMENTS';
export type Direction = 'DEPENDENCIES' | 'DEPENDENTS';
export type DependencyKind = 'CALLS' | 'DISPATCHES_TO' | 'IMPLEMENTS' | 'MAY_CALL';
export type Confidence = 'CERTAIN' | 'POSSIBLE';
export type ImpactLevel = 'LOW' | 'MEDIUM' | 'HIGH';
export type CycleLevel = 'FUNCTION' | 'FILE' | 'MODULE';
export type ArchitectureLevel = 'CRATE' | 'MODULE' | 'FILE';

export interface Repository {
	id: string;
	name: string;
	root: string;
	originUrl: string | null;
	branch: string | null;
	headSha: string | null;
	indexedSha: string | null;
	indexedAt: string;
	sourceFiles: number;
	loc: number;
	languages: string[];
}

export interface UnresolvedCall {
	line: number;
	callee: string;
	reason: string;
}

export interface Symbol {
	id: string;
	kind: SymbolKind;
	name: string;
	qualifiedName: string;
	file: string;
	crateName: string;
	startLine: number;
	endLine: number;
	visibility: string;
	signature: string | null;
	isTest: boolean;
	unresolvedCalls: UnresolvedCall[];
}

export interface Crate {
	id: string;
	name: string;
	package: string;
	kind: string;
	rootFile: string;
	rootModule: string | null;
}

export interface GraphEdge {
	from: string;
	to: string;
	relation: Relation;
	resolution: string | null;
	lines: number[];
}

export interface NeighborhoodNode {
	symbol: Symbol;
	depth: number;
	via: GraphEdge;
}

export interface Neighborhood {
	root: Symbol;
	direction: Direction;
	maxDepth: number;
	nodes: NeighborhoodNode[];
	edges: GraphEdge[];
	truncated: boolean;
}

export interface SymbolRef {
	id: string;
	kind: SymbolKind;
	qualifiedName: string;
	file: string;
	line: number;
	isTest: boolean;
}

export interface EvidenceStep {
	source: string;
	target: string;
	kind: DependencyKind;
	file: string;
	lines: number[];
	resolution: string | null;
}

export interface AffectedSymbol {
	symbol: SymbolRef;
	depth: number;
	confidence: Confidence;
	path: EvidenceStep[];
}

export interface AffectedGroup {
	name: string;
	symbols: number;
	tests: number;
}

export interface ScoreFactor {
	name: string;
	value: number;
	saturation: number;
	weight: number;
	normalized: number;
	contribution: number;
}

export interface ImpactReport {
	changed: SymbolRef[];
	maxDepth: number;
	includeAmbiguous: boolean;
	directCount: number;
	indirectCount: number;
	possibleCount: number;
	affected: AffectedSymbol[];
	files: AffectedGroup[];
	modules: AffectedGroup[];
	tests: string[];
	truncated: boolean;
	score: { total: number; level: ImpactLevel; factors: ScoreFactor[] };
}

export interface EvidenceRef {
	from: string;
	to: string;
	relation: string;
	file: string;
	line: number;
}

export interface Hop {
	from: string;
	to: string;
	weight: number;
	via: string[];
	evidence: EvidenceRef[];
}

export interface Cycle {
	members: string[];
	hops: Hop[];
}

export interface ArchitectureGraph {
	level: ArchitectureLevel;
	nodes: { id: string; fanIn: number; fanOut: number; inCycle: boolean }[];
	edges: { from: string; to: string; weight: number; via: string[] }[];
}

export interface SourceSnippet {
	file: string;
	startLine: number;
	endLine: number;
	totalLines: number;
	lines: string[];
}

export interface SearchPage {
	edges: { cursor: string; node: Symbol }[];
	pageInfo: { hasNextPage: boolean; endCursor: string | null };
}

export interface IndexResult {
	repository: Repository;
	filesAnalyzed: number;
	nodes: number;
	relationships: number;
	resolutionRate: number | null;
	analysisMs: number;
	writeMs: number;
}
