# Architecture

CodeAtlas answers one question: *if this symbol changes, what else could be
affected, and why?* Every answer is derived from static analysis and graph
traversal. The pipeline is ordered so that each stage is deterministic and
independently testable:

```
Git repository
  → ingest      (local path or clone, file discovery, repository metadata)
  → layout      (Cargo targets; module tree from `mod` declarations)
  → parser      (tree-sitter syntax trees)
  → symbols     (declarations, imports, impl blocks, call sites, bindings)
  → resolver    (references → symbol IDs, with explicit outcomes)
  → dependencies (derived file / module DEPENDS_ON aggregates)
  → store       (Neo4j code graph, bounded queries)
  → graph       (in-memory CodeGraph: algorithms, impact, architecture)
  → diff        (two revisions compared symbol by symbol, impact of the change)
  → server      (GraphQL API)
  → web         (SvelteKit workspace UI)
```

## Crates

| Crate | Responsibility | Depends on |
|---|---|---|
| `crates/analyzer` | Ingestion, layout, parsing, extraction, resolution and its evaluation, derived dependencies, graph algorithms, impact and architecture analyses, Git diff analysis. Pure library; no database or network code beyond invoking `git` (and `tar` to unpack `git archive`). | tree-sitter, ignore, toml |
| `crates/store` | Neo4j schema, batched writes, bounded queries (traversals with evidence paths, shortest path, search, file/module dependencies), loading a stored graph back into a `CodeGraph`. | analyzer, neo4rs, tokio |
| `crates/server` | GraphQL API over the store and analyzer, graph cache, guarded source reads. | analyzer, store, async-graphql, axum, tokio |
| `crates/cli` | `codeatlas` binary: `analyze`, `evaluate`, `index`, `query`, `diff`, `remove`, `ast`; later `bench`. | analyzer, store |
| `web/` | SvelteKit + TypeScript + Cytoscape.js workspace UI; a static single-page app. | GraphQL API only |

The analyzer is kept free of storage concerns so that extraction, resolution
and the algorithms can be tested on in-memory data, and so that the CLI, the
server and the benchmark harness share exactly the same analysis code.

## Technology decisions

**Git: the `git` CLI.** Cloning through the CLI honours the user's credential
helpers and SSH configuration, and diff/rename semantics match what the user
sees in their terminal. The wrapper (`analyzer/src/git.rs`) runs commands with
`GIT_TERMINAL_PROMPT=0` so it never blocks on input, returns typed errors, and
strips credentials from remote URLs before they are stored.

**Parsing: tree-sitter.** It is error-tolerant (a file with a syntax error
still yields a usable tree for the valid regions), fast, and gives a concrete
syntax tree with field names, which keeps extraction rules explicit.

**GraphQL: async-graphql 7.2.1 (pinned) on axum 0.8.** Actively
maintained, derives schema types from Rust types, supports cursor
connections, and has built-in query depth and complexity limits, which are
required to stop clients from requesting unbounded traversals. Juniper was
the alternative; its async support and release cadence are weaker. 8.0 was
still a release candidate. See [api.md](api.md).

**Parallelism and memory.** Parsing is CPU-bound and currently sequential.
Analysis makes two passes (module declarations, then extraction) and
re-parses in the second pass instead of keeping every syntax tree alive:
on ripgrep this raised parse time from about 77 ms to 147 ms but lowered
peak memory from about 89 MB to 38 MB in single runs. Parsing will be
parallelised once the benchmark harness can show the effect; Tokio is
reserved for the server's I/O.

## Ingestion (`analyzer/src/ingest`)

* `RepoSource::parse` treats inputs containing `://`, `git@host:` or a
  non-existent `*.git` path as URLs; everything else is a local path.
* Remote repositories are cloned with `--filter=blob:none` (full history for
  diff analysis, blobs fetched lazily) into `$CODEATLAS_CLONE_DIR`, defaulting
  to `$XDG_CACHE_HOME/codeatlas/repos` or `~/.cache/codeatlas/repos`. An
  existing clone is updated with `git pull --ff-only`.
* Discovery walks the tree with the `ignore` crate, honouring `.gitignore`
  even outside a Git work tree, and always skipping `.git`, `target`, `build`,
  `dist`, `vendor`, `node_modules`, `.svelte-kit` and `__pycache__`. Files are
  skipped when they are larger than 2 MiB, not UTF-8, named like generated
  code (`*.pb.rs`, `*_generated.*`) or carry a generated-code marker
  (`@generated`, `DO NOT EDIT`, …) in their first five lines.
* Metadata: repository ID (FNV-1a of the origin URL, or of the canonical path
  when there is no remote), name, branch (absent when detached), HEAD SHA,
  per-language file and LOC counts, and the analysis timestamp. LOC counts
  non-blank lines. The last *indexed* commit is a property of the stored
  graph and is recorded by the store.

## Layout (`analyzer/src/layout.rs`)

Targets come from Cargo's conventions (`src/lib.rs`, `src/main.rs`,
`src/bin/*`, `tests/*`, `examples/*`, `benches/*`, `build.rs`) and from
`[lib]`, `[[bin]]`, `[[test]]`, `[[example]]` and `[[bench]]` entries in
`Cargo.toml` (names and paths). Dependency names are collected from every
`[*dependencies]` table for the resolver.

Module paths are then computed as the compiler does, by following
`mod name;` declarations from each crate root (`module_tree.rs`), including
`#[path = "..."]` and inline modules. A file that another file declares as
a module is never treated as its own crate root (`tests/util.rs` declared by
`tests/tests.rs`), and a file declared only by `main.rs` belongs to the
binary. Files no crate root reaches fall back to a path-based mapping
(`a/b.rs` and `a/b/mod.rs` → `a::b`).

When a package has both a library and `src/main.rs`, the binary crate is
named `<package>_bin` so its symbols cannot collide with the library's.
`include!` is not followed.

## Extraction (`analyzer/src/symbols`)

The extractor walks the syntax tree and records what is written in source:

| Construct | Recorded as |
|---|---|
| File, `mod x { }` | `Module` symbol |
| `mod x;` | `ModuleDecl` (its visibility is applied to the file module) |
| `struct`, `union` / `enum` / `trait` | `Struct` / `Enum` / `Trait` symbol with signature |
| `fn` | `Function` (nested functions are children of the enclosing function) |
| `fn` in `impl` / `trait` | `Method`, plus an `ImplBlock` recording self type and trait |
| `#[test]`, `#[tokio::test]`, `#[rstest]`, … | `is_test = true` |
| `#[cfg(test)]` on an item or enclosing module | `cfg_test = true` |
| `use` trees, `extern crate a as b` | one `Import` per bound name (groups, aliases, globs, `self`) |
| `type A = B;` | `TypeAlias` |
| parameters, `let`, closure parameters | `LocalBinding` with the written type, constructor call or literal, if any |
| struct fields | `FieldDecl` with the declared type |
| generic parameters | `type_params` on functions, methods and types |
| calls | `CallSite` with the innermost enclosing symbol as caller |

**Symbol IDs** are `<kind>:<qualified name>`, e.g.
`method:shop::payments::PaymentService::authorize`. Trait-impl methods use
Rust's qualified form, `shop::money::<Money as std::fmt::Display>::fmt`, so
methods of different trait impls do not collide. IDs contain no line numbers,
so they are stable across edits that move code. Impl methods are named
after their self type once the resolver has located it, wherever the
`impl` block is written. Genuine duplicates (for
example `#[cfg(unix)]` and `#[cfg(not(unix))]` versions of one function) get
a `#2`, `#3`, … suffix in path and source order.

**Call sites** are classified as paths (`f()`, `a::b::f()`, `Type::f()`,
`<T as Trait>::f()`), method calls or dynamic calls (closures stored in
fields and other expressions). Method receivers are recorded structurally
(`self`, `self.field`, a variable, a constructor call, a method-call chain,
`expr?`) so the resolver can infer their types.

**Macros.** Tree-sitter does not parse macro arguments; they arrive as raw
token trees. For `(..)` and `[..]` macros the extractor re-parses the
arguments as an array expression and maps the lines back, so calls in
`assert_eq!(compute(x), 1)` or `vec![build(); n]` are recorded with
`in_macro = true`, together with `let` and closure-parameter bindings
inside them. Brace-delimited macros (`my_dsl! { .. }`) are DSLs more often
than expressions and are not re-parsed. Macro-generated items are not seen.

## Resolution (`analyzer/src/resolver`)

See [symbol-resolution.md](symbol-resolution.md). Every call site ends in
one outcome: resolved, ambiguous (with candidates and a reason),
unresolved (with a reason), external, constructor or local. Only resolved
targets become `CALLS` edges. `evaluation.rs` compares the output with
hand-written ground truth and reports precision and recall.

## Store (`crates/store`)

See [graph-model.md](graph-model.md) for the schema, the write procedure
and the query catalogue. Choices worth noting:

* **Driver: `neo4rs` 0.8.0, pinned.** It is the maintained async Bolt
  driver for Rust; 0.9 was still a release candidate. It is confined to
  this crate, so replacing it would not touch the analyzer.
* **Snapshots and deltas.** The graph of an analysis is computed as a
  comparable snapshot; a full write creates it, an incremental write
  applies the difference to the previous snapshot in one transaction. See
  [Incremental indexing](#incremental-indexing).
* **Traversal in Rust, expansion in Cypher.** Each BFS level is one
  parameterised Cypher query. This bounds work by depth and node count and
  records the edge that reached each node, so every result has an evidence
  path.
* **Configuration from the environment** (`CODEATLAS_NEO4J_*`, optionally
  via `.env`). The password is required and never logged; `StoreConfig`'s
  `Debug` output redacts it.

## Graph analyses (`analyzer/src/graph`)

See [impact-analysis.md](impact-analysis.md). `algorithms.rs` is generic
(BFS, Tarjan SCC, shortest paths and cycles, topological order, layering,
degree and betweenness centrality) and unit-tested on hand-built graphs.
`impact.rs` and `architecture.rs` give it code semantics. All of it runs on
the in-memory `CodeGraph`, which is built from an analysis or loaded from
Neo4j; both produce identical graphs. Whole-graph algorithms such as SCC
and betweenness run in Rust rather than in Neo4j, so the Graph Data
Science plugin is not needed.

## Diff analysis (`analyzer/src/diff`)

See [impact-analysis.md](impact-analysis.md#git-diff-impact). Choices
worth noting:

* **Both revisions are analysed from scratch.** Each commit is exported
  with `git archive` and analysed in full, so the comparison never depends
  on what happens to be indexed, and the work tree is never checked out or
  modified. On ripgrep a diff takes about 0.8 s. Unlike indexing, diffs do
  not yet reuse per-file results between revisions.
* **Fingerprints instead of text comparison.** Each item's token hash
  (without whitespace and comments) separates real modifications from
  reformatting, and pairs code that moved under a new ID.
* **Impact on both graphs.** Modified code is traced on the head graph,
  removed code on the base graph; the results are merged with their
  evidence and the revision they come from.
* The CLI (`codeatlas diff`, text, Markdown or JSON) and the API
  (`gitImpact`, `gitRefs`) are thin layers over `analyze_diff`; neither
  needs the database.

## Incremental indexing

`analyzer::incremental` and `store::incremental`. The pipeline:

1. **Changed files.** Discovery already reads every file to count lines,
   so it also hashes each one. A file's per-file results are reused when
   its content hash, crate and module path match the saved state. Hashing
   covers what `git diff` against the indexed commit would report, plus
   uncommitted and untracked edits, and works without Git.
2. **Re-parse changed files.** Only files with a new hash are read and
   parsed for their `mod` declarations; the module tree is rebuilt from the
   declarations of all files (saved ones for the rest). Files are
   extracted again when their content changed *or* their module path or
   crate did (a moved `mod`, a renamed package), even if their content did
   not.
3. **Re-resolve.** ID de-duplication, module linking and resolution run
   over all files every time. A change in one file can change what a name
   means in another (glob imports, re-exports, shadowing, a removed
   definition), and tracking that precisely would be a second resolver; on
   ripgrep the whole step takes about 20 ms, against about 150 ms for
   parsing. The analysis is therefore identical to a full one, which the
   tests check field by field.
4. **Changed symbols and obsolete relationships.** The previous analysis is
   rebuilt from the saved per-file results (no parsing), both analyses are
   turned into graph snapshots, and their difference is computed per node
   and per relationship.
5. **Update Neo4j.** The difference is applied in one transaction (see
   [graph-model.md](graph-model.md#writing)).

The state (per-file declarations and extraction results, crates and
dependencies, and the token of the write) is saved as JSON in
`CODEATLAS_STATE_DIR`, default `~/.cache/codeatlas/index`, one file per
repository; about 6.4 MB for ripgrep. It is used only if it was written by
the same analyzer version (`CACHE_VERSION` and the crate version) and its
token matches the stored graph; otherwise the run indexes in full and saves
a fresh state. Losing the state therefore costs one full index, never a
wrong graph.

## Benchmarks (`cli/src/bench.rs`)

`codeatlas bench` produces the measurements in
[benchmarks/](../benchmarks). Choices worth noting:

* **Analyses in fresh processes.** Each analysis run starts a new
  `codeatlas` process (a hidden `bench-analysis` subcommand), so memory is
  that process's peak resident size and no run benefits from another's
  allocations. The operating system's file cache is warm after the first
  run, as it would be for a developer re-running an analysis.
* **Indexing on a scratch copy** under a repository ID of its own, so a
  benchmark never touches an existing index or the source.
* **Raw samples kept.** Results contain every sample next to the summary,
  plus the environment (CPU, Neo4j version, build profile), so they can be
  checked and compared, not just quoted.

## Web UI (`web/`)

A SvelteKit 3 app (Svelte 5, TypeScript) built with `adapter-static` as a
single-page app; it has no server-side code and reads everything through
the GraphQL API.

* `src/lib/api`: a small `fetch` client (errors carry the API's
  `extensions.code`) and one typed function per operation. No GraphQL
  client library is used, since the app needs no normalised cache.
* `src/lib/state/workspace.svelte.ts`: one state object built on Svelte
  runes, holding the repository, selection, per-view data and the actions
  that load it. Selections are mirrored into the URL with shallow
  navigation, and late responses for an older selection are discarded.
* `src/lib/graph`: pure TypeScript, unit-tested without a browser. The
  `GraphModel` stores the explored neighbourhood and which node expanded
  which, so collapsing removes only the nodes an expansion added. The
  element builders turn API results into canvas elements. The layouts are
  layered: trees by distance from the focused symbol; dependency graphs by
  longest path, with cycle edges ignored; each in whichever orientation
  draws the graph largest, wrapping wide layers. Cycles are drawn as a ring
  in hop order. `zoom.ts` aggregates the module graph by hierarchy: inside
  a scope each submodule tree is one node (weights summed), so a crate
  with hundreds of modules (448 in tokio) shows its top level first.
* `src/lib/components`: the explorer, graph canvas (Cytoscape.js, updated
  by diffing elements so layouts only rerun when the structure changes),
  inspector, command palette and the five views. Colours are CSS custom
  properties with light and dark values; the canvas stylesheet reads them,
  so the graph follows the theme.

**Why Cytoscape.js.** It handles the interaction needed here (selection,
double-click, hover, zoom, pan) and thousands of elements on canvas, ships
TypeScript types, and accepts precomputed positions, so layouts can be
plain, tested functions.

## Milestones

| # | Scope | Status |
|---|---|---|
| 1 | Workspace, ingestion, discovery, metadata, layout, parsing, extraction, CLI | Done |
| 2 | Module tree, symbol resolution with explicit outcomes, receiver-type inference, ground-truth evaluation | Done |
| 3 | Neo4j store, derived dependencies, bounded queries, `index` / `query` CLI | Done |
| 4 | Graph algorithms, impact engine with evidence chains and a decomposable score, cycles, hotspots and layers | Done |
| 5 | GraphQL API: queries, mutations, pagination, error codes, limits, graph cache | Done |
| 6 | SvelteKit workspace UI: explorer, graph, impact, architecture zoom and cycle views, inspector with source, command palette (completes the MVP) | Done |
| 7 | Git diff analysis: changed files and symbols (signatures, moves, cosmetic edits), impact of a diff, `codeatlas diff` (text / Markdown / JSON), `gitImpact` and `gitRefs`, Changes view | Done |
| 8 | Incremental indexing: per-file reuse by content hash and module path, global re-resolution, graph snapshots and deltas written in one transaction, measured against full indexing | Done |
| 9 | Test selection (direct, transitive, possible, untested changes) with evidence; ground truth by panic probes (`probe-tests`), precision and recall (`evaluate-tests`) on fixtures and ripgrep | Done |
| 10 | Benchmark suite (`codeatlas bench`): corpus, resolution, analysis time and throughput, peak memory, full and incremental indexing, query latency, test-impact precision/recall, as JSON; results on ripgrep, CodeAtlas and tokio; UI checked on tokio | Done |
| 11 | Optional local-model explanations grounded in graph evidence | Planned |
