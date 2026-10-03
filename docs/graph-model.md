# Graph model

The Neo4j schema written by `crates/store` (`writer.rs`, `schema.rs`).
Every node except `Repository` carries `repo_id`, so any number of
repositories can share one database; every query is scoped to one of them.

## Principles

* Store what traversal and inspection need, nothing more. Source text is
  read from the repository on demand, never stored in the graph.
* Every edge that impact analysis can traverse must be explainable: it
  carries the source lines that justify it and how it was resolved.
* Uncertain facts are stored separately from certain ones, so that a query
  never silently mixes them.

## Nodes

Every code symbol carries the `:Symbol` label plus one kind label, and the
shared properties `id`, `repo_id`, `kind`, `name`, `qualified_name`, `file`,
`crate`, `module` (the module the code is written in), `start_line`,
`end_line`, `visibility`, `signature`, `parent_id`, `is_test`, `cfg_test`
and `unresolved_calls`.

| Label | Why it exists | Specific properties |
|---|---|---|
| `Repository` | Root of a graph; holds index metadata. | `id`, `format_version`, `name`, `root`, `origin_url`, `branch`, `head_sha`, `indexed_sha`, `analyzed_at`, `indexed_at`, `languages` / `language_files` / `language_loc` (parallel lists), `source_files`, `loc`, `calls_total`, `calls_resolved`, `calls_ambiguous`, `calls_unresolved`, `resolution_rate` |
| `Crate` | The *package* level of the architecture view. `id` is `crate:<package>:<name>`, because two packages may both have a test target named `integration`. | `name`, `package`, `target_kind`, `root_file` |
| `File` | Unit of incremental re-indexing; anchors file-level dependencies. | `path`, `crate`, `loc`, `syntax_errors`, `content_hash` |
| `Module` | Rust namespace; module-level dependencies and cycles are computed here. | — |
| `Struct`, `Enum`, `Trait` | Owners of methods and endpoints of `IMPLEMENTS`. | — |
| `Function`, `Method` | Units of call-graph impact. | — |
| `Test` (additional label) | Seeds test-impact queries without a separate model. | — |

`unresolved_calls` is a list of `line|callee|reason` strings, for example
`27|normalize|name_not_in_scope`. It keeps unresolved references visible on
the function that contains them without creating a node per external call
(`Vec::new`, `Option::unwrap`, …), which would dominate the graph.

## Relationships

| Type | Direction | Why it exists | Properties |
|---|---|---|---|
| `CONTAINS` | Repository→Crate, Crate→root Module, Module→Module, Module→File | Structural hierarchy for the explorer and architecture zoom. | — |
| `DEFINES` | Module/Function→item, Struct/Enum/Trait→Method | Ownership of symbols (methods are owned by their self type, wherever their `impl` is written). | — |
| `CALLS` | callable → callable | Resolved call. The primary impact edge. | `resolution`, `lines` |
| `CALLS_CANDIDATE` | callable → callable | One edge per candidate of an ambiguous call. Never followed unless a query asks for it explicitly. | `reason`, `candidates` (total candidate count), `lines` |
| `IMPORTS` | Module/Function → Symbol | `use` declarations; module coupling. | `resolution`, `lines` |
| `IMPLEMENTS` | Struct/Enum → Trait, Method → trait Method | A change to a trait method reaches every implementation. | `resolution`, `lines` |
| `DEPENDS_ON` | File→File, Module→Module | Derived aggregation of `CALLS`, `IMPORTS` and `IMPLEMENTS` between symbols written in different files or modules (`analyzer/src/dependencies.rs`); input for cycle detection and the architecture view. | `weight` (number of underlying edges), `via` (their types), `evidence` (up to five `from\|to\|TYPE\|line` samples) |

`resolution` records how the target was determined: `scope` (defined in the
enclosing scope), `import`, `path` (anchored at `crate`, `self`, `super` or
a crate name), `self_type`, `receiver_type`, or `impl_block` on
`IMPLEMENTS`. Resolution quality can therefore be measured per strategy and
filtered in queries. There is deliberately no name-similarity strategy; see
[symbol-resolution.md](symbol-resolution.md).

`REFERENCES` (type usage in signatures) is planned and not written yet.

## Deliberate omissions

* **No `TESTS` edge.** A test is a function with the `:Test` label; the tests
  related to a symbol are those that reach it over `CALLS`. Materialising
  `TESTS` would duplicate the call graph and go stale under incremental
  indexing.
* **No per-call-site nodes.** Call sites are aggregated into one `CALLS` edge
  per caller/callee pair with the list of lines.
* **No source text.** Signatures are stored; bodies are read from disk.

## Constraints and indexes

```cypher
CREATE CONSTRAINT repository_id IF NOT EXISTS FOR (r:Repository) REQUIRE r.id IS UNIQUE;
CREATE CONSTRAINT symbol_key IF NOT EXISTS FOR (s:Symbol) REQUIRE (s.repo_id, s.id) IS UNIQUE;
CREATE CONSTRAINT file_key IF NOT EXISTS FOR (f:File) REQUIRE (f.repo_id, f.path) IS UNIQUE;
CREATE CONSTRAINT crate_key IF NOT EXISTS FOR (c:Crate) REQUIRE (c.repo_id, c.id) IS UNIQUE;
CREATE INDEX symbol_repo IF NOT EXISTS FOR (s:Symbol) ON (s.repo_id);
CREATE INDEX symbol_name IF NOT EXISTS FOR (s:Symbol) ON (s.repo_id, s.name);
CREATE INDEX symbol_qualified_name IF NOT EXISTS FOR (s:Symbol) ON (s.repo_id, s.qualified_name);
CREATE INDEX symbol_file IF NOT EXISTS FOR (s:Symbol) ON (s.repo_id, s.file);
CREATE INDEX file_repo IF NOT EXISTS FOR (f:File) ON (f.repo_id);
CREATE INDEX crate_repo IF NOT EXISTS FOR (c:Crate) ON (c.repo_id);
CREATE FULLTEXT INDEX symbol_search IF NOT EXISTS FOR (s:Symbol) ON EACH [s.name, s.qualified_name];
```

`(repo_id, file)` makes "delete everything defined in this file" cheap,
which is the first step of incremental re-indexing. The statements run on
every connection; an "already exists" error from a concurrent client counts
as success.

## Writing

`GraphStore::index` replaces a repository's graph:

1. The previous subgraph is deleted in batches of 5,000 nodes per label,
   outside the write transaction, so deleting a large repository does not
   build one huge transaction.
2. All nodes and relationships are created in one transaction with batched
   `UNWIND` statements (1,000 rows per statement). Labels and relationship
   types are constants chosen in code; user data only ever travels as
   parameters.

If step 2 fails, the repository is absent rather than half-written; running
`codeatlas index` again restores it.

`format_version` records the layout written (currently 2).
`GraphStore::load_graph` rejects other versions and asks for a re-index, so
a graph written by an older CodeAtlas never yields silently wrong results.

## Loading for algorithms

`GraphStore::load_graph` reads a repository's symbols, symbol relationships
and `DEPENDS_ON` aggregates back into the analyzer's in-memory `CodeGraph`,
where the impact engine and the architecture analyses run (see
[impact-analysis.md](impact-analysis.md)). The loaded graph is tested to be
identical to the one built directly from the analysis.

## Queries

All read queries are scoped to one repository and bounded
(`QueryLimits`, defaults: depth ≤ 10, ≤ 500 nodes per traversal, ≤ 50
search results). Out-of-range depths are rejected with an error rather than
silently clamped.

| Question | Method | How |
|---|---|---|
| Who calls this function? / What does it call? | `callers`, `callees` | Breadth-first traversal over `CALLS`, one Cypher query per level. |
| What transitively depends on / is depended on by this symbol? | `traverse` | Same traversal over any set of `CALLS`, `IMPORTS`, `IMPLEMENTS`, `CALLS_CANDIDATE`, in either direction. |
| Which tests reach this symbol? | `related_tests` | Callers traversal filtered to `:Test` nodes, each with its call chain. |
| How is A connected to B? | `shortest_path` | Cypher `shortestPath` over `CALLS\|IMPORTS\|IMPLEMENTS` with the depth bound inlined as a validated integer. |
| Which files / modules depend on this one? | `file_dependencies`, `module_dependencies` | One hop over `DEPENDS_ON`, ordered by weight. |
| Find a symbol | `search`, `find_symbol` | Full-text prefix search (exact name matches first); exact ID, qualified name, `Type::method` suffix or unique name. |

Traversal is level-by-level from Rust, not a variable-length Cypher pattern,
for two reasons. Variable-length patterns enumerate every path and can
explode on dense call graphs, while level-by-level expansion touches each
node once. Recording the edge that first reached each node also gives a
shortest evidence path for every result (`Traversal::path_to`), which is
what makes a result explainable.

`related_tests` follows `CALLS` only. Impact analysis
(`codeatlas query impact`) also follows trait dispatch and reports the
tests that reach a changed implementation through its trait method.
