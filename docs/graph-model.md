# Graph model

Status: design. The Neo4j store is implemented in Milestone 3; this document
fixes the schema beforehand so extraction and resolution produce exactly the
data it needs.

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
`start_line`, `end_line`, `visibility`, `signature` and `parent_id`.

| Label | Why it exists | Specific properties |
|---|---|---|
| `Repository` | Root of a graph; holds index metadata. | `name`, `origin_url`, `branch`, `head_sha`, `indexed_sha`, `indexed_at`, `languages`, `loc` |
| `Crate` | The *package* level of the architecture view and the anchor for `crate::` paths. | `name`, `package`, `target_kind`, `root_file` |
| `File` | Unit of incremental re-indexing; anchors file-level dependencies. | `path`, `loc`, `content_hash` |
| `Module` | Rust namespace; module-level dependencies and cycles are computed here. | `cfg_test` |
| `Struct`, `Enum`, `Trait` | Owners of methods and endpoints of `IMPLEMENTS`. | — |
| `Function`, `Method` | Units of call-graph impact. | `unresolved_calls` (callee text and reason per site) |
| `Test` (additional label) | Seeds test-impact queries without a separate model. | — |

`unresolved_calls` keeps unresolved references visible on the function that
contains them without creating a node per external call (`Vec::new`,
`Option::unwrap`, …), which would dominate the graph.

## Relationships

| Type | Direction | Why it exists | Properties |
|---|---|---|---|
| `CONTAINS` | Repository→Crate→Module→Module, Module→File | Structural hierarchy for the explorer and architecture zoom. | — |
| `DEFINES` | Module/Function→item, Struct/Enum/Trait→Method | Lexical ownership of symbols. | — |
| `CALLS` | callable → callable | Resolved call. The primary impact edge. | `resolution`, `lines` |
| `CALLS_CANDIDATE` | callable → callable | One edge per candidate of an ambiguous call. Excluded from impact by default and labelled when included. | `lines`, `reason` |
| `IMPORTS` | Module/Function → Symbol/Module | `use` declarations; drive name resolution and module coupling. | `alias`, `glob`, `line` |
| `IMPLEMENTS` | Struct/Enum → Trait, Method → trait Method | A change to a trait method reaches every implementation. | — |
| `REFERENCES` | callable → type | Type usage in signatures and paths (added after the MVP). | `lines` |
| `DEPENDS_ON` | File→File, Module→Module | Derived aggregation of the edges above, recomputed at index time; input for cycle detection and the architecture view. | `weight`, `via` |

`resolution` on `CALLS` records how the target was determined: `scope`
(defined in the enclosing scope), `import`, `path` (anchored at `crate`,
`self`, `super` or a crate name), `self_type` or `receiver_type`.
`IMPLEMENTS` edges carry `impl_block`. Resolution quality can therefore be
measured per strategy and filtered in queries. There is deliberately no
name-similarity strategy; see [symbol-resolution.md](symbol-resolution.md).

## Deliberate omissions

* **No `TESTS` edge.** A test is a function with the `:Test` label; the tests
  related to a symbol are those that reach it over `CALLS`. Materialising
  `TESTS` would duplicate the call graph and go stale under incremental
  indexing.
* **No per-call-site nodes.** Call sites are aggregated into one `CALLS` edge
  per caller/callee pair with the list of lines.

## Constraints and indexes

```cypher
CREATE CONSTRAINT symbol_id IF NOT EXISTS FOR (s:Symbol) REQUIRE (s.repo_id, s.id) IS UNIQUE;
CREATE CONSTRAINT file_path IF NOT EXISTS FOR (f:File) REQUIRE (f.repo_id, f.path) IS UNIQUE;
CREATE CONSTRAINT repo_id IF NOT EXISTS FOR (r:Repository) REQUIRE r.id IS UNIQUE;
CREATE INDEX symbol_file IF NOT EXISTS FOR (s:Symbol) ON (s.repo_id, s.file);
CREATE FULLTEXT INDEX symbol_search IF NOT EXISTS FOR (s:Symbol) ON EACH [s.name, s.qualified_name];
```

`(repo_id, file)` makes "delete everything defined in this file" cheap,
which is the first step of incremental re-indexing.
