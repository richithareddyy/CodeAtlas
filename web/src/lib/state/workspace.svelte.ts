// Application state and actions shared by all panels.

import { goto } from '$app/navigation';
import * as api from '../api/queries';
import { ApiError, describeError } from '../api/client';
import type {
	ArchitectureGraph,
	ArchitectureLevel,
	Crate,
	Cycle,
	CycleLevel,
	GitImpactReport,
	GitRefs,
	ImpactReport,
	Relation,
	Repository,
	Symbol,
	SymbolKind
} from '../api/types';
import { GraphModel } from '../graph/model';

export type View = 'graph' | 'impact' | 'changes' | 'architecture' | 'cycles';
export type GraphDirection = 'DEPENDENTS' | 'DEPENDENCIES' | 'BOTH';

const VIEWS: View[] = ['graph', 'impact', 'changes', 'architecture', 'cycles'];

/** Branch names tried, in order, as the default base of a comparison. */
const DEFAULT_BASES = ['main', 'master', 'trunk', 'develop'];

class Workspace {
	repositories = $state<Repository[]>([]);
	repoId = $state<string | null>(null);
	crates = $state<Crate[]>([]);
	view = $state<View>('graph');

	/** Symbol shown in the inspector. */
	selected = $state<Symbol | null>(null);
	/** Line to highlight in the source view (from evidence links). */
	focusLine = $state<number | null>(null);

	// Graph view
	model = $state.raw<GraphModel | null>(null);
	/** Incremented whenever `model` is mutated in place. */
	modelVersion = $state(0);
	direction = $state<GraphDirection>('DEPENDENTS');
	depth = $state(1);
	relations = $state<Relation[]>(['CALLS']);
	hiddenKinds = $state<SymbolKind[]>([]);

	// Impact view
	impact = $state.raw<ImpactReport | null>(null);
	/** The symbol `impact` was computed for. */
	impactSubject = $state<string | null>(null);
	impactDepth = $state(8);
	includeAmbiguous = $state(false);

	// Architecture view
	archLevel = $state<ArchitectureLevel>('CRATE');
	arch = $state.raw<ArchitectureGraph | null>(null);
	/** Crate (or module path prefix) the architecture view is limited to. */
	archScope = $state<string | null>(null);

	// Cycles view
	cycleLevel = $state<CycleLevel>('MODULE');
	cycles = $state.raw<Cycle[] | null>(null);
	cycleIndex = $state(0);

	// Changes view (Git diff impact)
	gitRefs = $state.raw<GitRefs | null>(null);
	/** Why Git revisions cannot be compared for this repository, if so. */
	gitUnavailable = $state<string | null>(null);
	diffBase = $state('');
	/** Empty for the working tree. */
	diffHead = $state('');
	diff = $state.raw<GitImpactReport | null>(null);

	// Status
	pending = $state(0);
	error = $state<string | null>(null);
	apiReachable = $state<boolean | null>(null);
	paletteOpen = $state(false);
	indexOpen = $state(false);

	repository = $derived(this.repositories.find((r) => r.id === this.repoId) ?? null);
	busy = $derived(this.pending > 0);

	/** Bumped by every selection, so late responses for older ones are ignored. */
	private selectToken = 0;

	private async track<T>(work: () => Promise<T>): Promise<T | undefined> {
		this.pending++;
		try {
			const result = await work();
			this.apiReachable = true;
			return result;
		} catch (error) {
			this.error = describeError(error);
			if ((error as { code?: string }).code === 'NETWORK') this.apiReachable = false;
			return undefined;
		} finally {
			this.pending--;
		}
	}

	async init(url: URL) {
		const repos = await this.track(() => api.repositories());
		if (!repos) return;
		this.repositories = repos;
		const wanted = url.searchParams.get('repo');
		const repo = repos.find((r) => r.id === wanted || r.name === wanted) ?? repos[0];
		const view = url.searchParams.get('view') as View | null;
		if (view && VIEWS.includes(view)) this.view = view;
		this.diffBase = url.searchParams.get('base') ?? '';
		this.diffHead = url.searchParams.get('head') ?? '';
		if (!repo) return;
		// Also loads the architecture or cycles view when that is the current view.
		await this.selectRepository(repo.id, false);
		const symbolId = url.searchParams.get('symbol');
		if (symbolId) await this.focus(symbolId, { view: this.view, keepUrl: true });
	}

	async reloadRepositories() {
		const repos = await this.track(() => api.repositories());
		if (repos) this.repositories = repos;
	}

	async selectRepository(id: string, updateUrl = true) {
		this.repoId = id;
		this.selected = null;
		this.model = null;
		this.impact = null;
		this.arch = null;
		this.archScope = null;
		this.cycles = null;
		this.gitRefs = null;
		this.gitUnavailable = null;
		this.diff = null;
		if (updateUrl) {
			this.diffBase = '';
			this.diffHead = '';
		}
		this.crates = (await this.track(() => api.crates(id))) ?? [];
		if (updateUrl) this.syncUrl();
		if (this.view === 'architecture') await this.loadArchitecture();
		if (this.view === 'cycles') await this.loadCycles();
		if (this.view === 'changes') await this.openChanges();
	}

	setView(view: View) {
		this.view = view;
		this.syncUrl();
		if (view === 'architecture' && !this.arch) void this.loadArchitecture();
		if (view === 'cycles' && !this.cycles) void this.loadCycles();
		if (view === 'changes' && !this.gitRefs) void this.openChanges();
		if (view === 'impact' && this.selected && !this.impactFor(this.selected.id)) {
			void this.runImpact();
		}
		if (view === 'graph' && this.selected && this.model?.rootId !== this.selected.id) {
			void this.loadNeighborhood(this.selected.id);
		}
	}

	/**
	 * Shows a symbol in the inspector without changing the graph. Returns
	 * whether the symbol is in the stored index; with `quiet`, a missing
	 * symbol is not reported as an error.
	 */
	async select(id: string, line: number | null = null, quiet = false): Promise<boolean> {
		const repo = this.repoId;
		if (!repo) return false;
		const token = ++this.selectToken;
		this.focusLine = line;
		if (this.selected?.id === id) return true;
		const symbol = await this.track(() => api.symbol(repo, id));
		if (token !== this.selectToken) return false;
		if (symbol) {
			this.selected = symbol;
			this.syncUrl();
			return true;
		}
		if (symbol === null && !quiet) {
			this.error = `Symbol ${id} is not in this repository's index.`;
		}
		return false;
	}

	/** Selects a symbol and makes it the subject of the current view. */
	async focus(id: string, options: { view?: View; keepUrl?: boolean } = {}) {
		const view =
			options.view ??
			(this.view === 'architecture' || this.view === 'cycles' || this.view === 'changes'
				? 'graph'
				: this.view);
		this.view = view;
		await this.select(id);
		if (!this.selected || this.selected.id !== id) return;
		if (view === 'impact') await this.runImpact();
		else if (view === 'graph') await this.loadNeighborhood(id);
		if (!options.keepUrl) this.syncUrl();
	}

	private async fetchNeighborhood(id: string, depth: number) {
		const repo = this.repoId;
		if (!repo) return undefined;
		const directions =
			this.direction === 'BOTH'
				? (['DEPENDENTS', 'DEPENDENCIES'] as const)
				: ([this.direction] as const);
		return this.track(() =>
			Promise.all(directions.map((d) => api.neighborhood(repo, id, d, depth, this.relations)))
		);
	}

	async loadNeighborhood(id: string) {
		const hoods = await this.fetchNeighborhood(id, this.depth);
		if (!hoods || this.selected?.id !== id) return;
		const model = GraphModel.focus(hoods[0]);
		for (const extra of hoods.slice(1)) model.merge(id, extra, 0);
		this.model = model;
		this.modelVersion++;
	}

	/** Double-click: expand a node one level, or collapse it if already expanded. */
	async toggleExpand(id: string) {
		const model = this.model;
		if (!model) return;
		if (model.isExpanded(id) && id !== model.rootId) {
			model.collapse(id);
			this.modelVersion++;
			return;
		}
		const hoods = await this.fetchNeighborhood(id, 1);
		if (!hoods || this.model !== model) return;
		for (const hood of hoods) model.merge(id, hood);
		this.modelVersion++;
	}

	collapse(id: string) {
		if (!this.model) return;
		this.model.collapse(id);
		this.modelVersion++;
	}

	async refreshGraph() {
		const root = this.model?.rootId ?? this.selected?.id;
		if (root) await this.loadNeighborhood(root);
	}

	impactFor(id: string): boolean {
		return this.impact?.changed.length === 1 && this.impact.changed[0].id === id;
	}

	async runImpact(id = this.selected?.id) {
		const repo = this.repoId;
		if (!repo || !id) return;
		this.view = 'impact';
		const report = await this.track(() =>
			api.impact(repo, id, this.impactDepth, this.includeAmbiguous)
		);
		if (report && this.selected?.id === id) {
			this.impact = report;
			this.impactSubject = id;
		}
		this.syncUrl();
	}

	async loadArchitecture(level = this.archLevel) {
		const repo = this.repoId;
		if (!repo) return;
		this.archLevel = level;
		const graph = await this.track(() => api.architecture(repo, level));
		if (graph && this.repoId === repo) this.arch = graph;
	}

	/** Zooms the architecture view into one crate (or module) at the next level down. */
	async drillDown(id: string) {
		if (this.archLevel === 'CRATE') {
			this.archScope = id;
			await this.loadArchitecture('MODULE');
		} else if (this.archLevel === 'MODULE' && id.startsWith('group:')) {
			// Zoom into a module tree; the graph is already loaded.
			this.archScope = id.slice('group:'.length);
		} else if (this.archLevel === 'MODULE' && id.startsWith('mod:')) {
			await this.focus(id, { view: 'graph' });
		}
	}

	async loadCycles(level = this.cycleLevel) {
		const repo = this.repoId;
		if (!repo) return;
		this.cycleLevel = level;
		this.cycleIndex = 0;
		const found = await this.track(() => api.cycles(repo, level));
		if (found && this.repoId === repo) this.cycles = found;
	}

	/** Loads the repository's refs, picks default revisions, and compares them. */
	async openChanges() {
		const repo = this.repoId;
		if (!repo) return;
		this.pending++;
		try {
			const refs = await api.gitRefs(repo);
			if (this.repoId !== repo) return;
			this.gitRefs = refs;
			this.gitUnavailable = null;
			this.apiReachable = true;
			if (!this.diffBase) this.defaultRevisions(refs);
		} catch (error) {
			const code = error instanceof ApiError ? error.code : null;
			if (code === 'NOT_A_GIT_REPOSITORY' || code === 'SOURCE_UNAVAILABLE') {
				this.gitUnavailable = describeError(error);
			} else {
				this.error = describeError(error);
				if (code === 'NETWORK') this.apiReachable = false;
			}
			return;
		} finally {
			this.pending--;
		}
		if (this.diffBase && !this.diff) await this.runDiff();
	}

	/**
	 * Picks what to compare: the main branch against the current branch, or,
	 * when the main branch is checked out, against the most recently
	 * committed other branch; otherwise the last commit against its parent.
	 */
	private defaultRevisions(refs: GitRefs) {
		// `refs` is sorted by date, newest first.
		const branches = refs.refs.filter((r) => r.kind === 'BRANCH').map((r) => r.name);
		const current = refs.currentBranch;
		const main = DEFAULT_BASES.find((b) => branches.includes(b));
		const other = branches.find((b) => b !== main);
		if (main && current && main !== current) {
			this.diffBase = main;
			this.diffHead = current;
		} else if (main && other) {
			this.diffBase = main;
			this.diffHead = other;
		} else if (refs.commits.length > 1) {
			this.diffBase = 'HEAD~1';
			this.diffHead = 'HEAD';
		}
	}

	async runDiff() {
		const repo = this.repoId;
		const base = this.diffBase.trim();
		if (!repo || !base) return;
		const head = this.diffHead.trim() || null;
		this.view = 'changes';
		const report = await this.track(() =>
			api.gitImpact(repo, base, head, this.impactDepth, this.includeAmbiguous)
		);
		if (report && this.repoId === repo) this.diff = report;
		this.syncUrl();
	}

	private syncUrl() {
		const params = new URLSearchParams();
		if (this.repoId) params.set('repo', this.repoId);
		if (this.selected) params.set('symbol', this.selected.id);
		if (this.view !== 'graph') params.set('view', this.view);
		if (this.view === 'changes' && this.diffBase) {
			params.set('base', this.diffBase);
			if (this.diffHead) params.set('head', this.diffHead);
		}
		const query = params.toString();
		void goto(query ? `?${query}` : '?', { replace: true, shallow: true }).catch(() => {});
	}
}

export const workspace = new Workspace();
