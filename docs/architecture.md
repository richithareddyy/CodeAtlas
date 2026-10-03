# Architecture

CodeAtlas answers one question: *if this symbol changes, what else could be
affected, and why?* Every answer is derived from static analysis and graph
traversal. The pipeline is ordered so that each stage is deterministic and
independently testable:

```
Git repository
  → ingest      (local path or clone, file discovery, repository metadata)
  → layout      (Cargo targets and module paths)
  → parser      (tree-sitter syntax trees)
  → symbols     (declarations, imports, impl blocks, call sites)
  → resolver    (call sites → symbol IDs, with explicit resolution status)   [M2]
  → store       (Neo4j code graph)                                           [M3]
  → algorithms / impact                                                       [M4]
  → GraphQL API                                                               [M5]
  → SvelteKit UI                                                              [M6]
```

## Crates

| Crate | Responsibility | Depends on |
|---|---|---|
| `crates/analyzer` | Ingestion, layout, parsing, extraction, resolution, graph algorithms, impact and diff analysis. Pure library; no database or network code beyond invoking `git`. | tree-sitter, ignore, toml |
| `crates/store` *(M3)* | Neo4j schema, batched writes, bounded Cypher queries. | analyzer, neo4rs |
| `crates/server` *(M5)* | GraphQL API over the store and analyzer. | analyzer, store, async-graphql, axum, tokio |
| `crates/cli` | `codeatlas` binary: `analyze`, `ast`, and later `index` and `bench`. | analyzer |
| `web/` *(M6)* | SvelteKit + TypeScript + Cytoscape.js workspace UI. | GraphQL API |

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

**GraphQL: async-graphql** *(M5)*. Actively maintained, derives schema types
from Rust types, supports cursor connections, and has built-in query depth
and complexity limits, which are required to stop clients from requesting
unbounded traversals. Juniper was the alternative; its async support and
release cadence are weaker.

**Parallelism.** Parsing is CPU-bound and currently sequential. It will be
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

Each Rust file is assigned to a Cargo target and a module path, following
Cargo's conventions (`src/lib.rs`, `src/main.rs`, `src/bin/*`, `tests/*`,
`examples/*`, `benches/*`, `build.rs`) and any non-conventional `path` keys in
`[lib]`, `[[bin]]`, `[[test]]`, `[[example]]` and `[[bench]]`. Module paths
come from file paths (`a/b.rs` and `a/b/mod.rs` → `a::b`).

Known gaps: `#[path = "..."]` attributes, `include!`, and files that are
never reached by a `mod` declaration are mapped by path alone. When a package
has both a library and `src/main.rs`, the binary crate is named
`<package>_bin` so its symbols cannot collide with the library's.

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
| `use` trees | one `Import` per bound name (groups, aliases, globs, `self`) |
| calls | `CallSite` with the innermost enclosing symbol as caller |

**Symbol IDs** are `<kind>:<qualified name>`, e.g.
`method:shop::payments::PaymentService::authorize`. Trait-impl methods use
Rust's qualified form, `shop::money::<Money as std::fmt::Display>::fmt`, so
methods of different trait impls do not collide. IDs contain no line numbers,
so they are stable across edits that move code. Genuine duplicates (for
example `#[cfg(unix)]` and `#[cfg(not(unix))]` versions of one function) get
a `#2`, `#3`, … suffix in path and source order.

**Call sites** are classified as paths (`f()`, `a::b::f()`, `Type::f()`,
`<T as Trait>::f()`), method calls (`receiver.f()`, with `self` receivers
marked) or dynamic calls (closures and other expressions, which cannot be
resolved without type information).

**Macros.** Tree-sitter does not parse macro arguments; they arrive as raw
token trees. For `(..)` and `[..]` macros the extractor re-parses the
arguments as an array expression and maps the lines back, so calls in
`assert_eq!(compute(x), 1)` or `vec![build(); n]` are recorded with
`in_macro = true`. Brace-delimited macros (`my_dsl! { .. }`) are DSLs more
often than expressions and are not re-parsed. Macro-generated items are not
seen.

## Milestones

| # | Scope | Status |
|---|---|---|
| 1 | Workspace, ingestion, discovery, metadata, layout, parsing, extraction, CLI | Done |
| 2 | Symbol resolution with explicit resolved / ambiguous / unresolved status and a resolution-rate metric | Planned |
| 3 | Neo4j store and bounded Cypher queries | Planned |
| 4 | Graph algorithms (BFS with evidence, Tarjan SCC, centrality, topological order) and the impact engine | Planned |
| 5 | GraphQL API | Planned |
| 6 | SvelteKit workspace UI (completes the MVP) | Planned |
| 7 | Git diff analysis and PR impact reports | Planned |
| 8 | Incremental indexing | Planned |
| 9 | Test-impact analysis with precision/recall on ground-truth fixtures | Planned |
| 10 | Architecture zoom, cycle view, benchmark suite | Planned |
| 11 | Optional local-model explanations grounded in graph evidence | Planned |
