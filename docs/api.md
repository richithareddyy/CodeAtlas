# GraphQL API

`crates/server` serves CodeAtlas over GraphQL (async-graphql 7 on axum
0.8). The full, generated schema is in [schema.graphql](schema.graphql); a
test fails if it drifts from the code.

## Running

```bash
docker compose up -d
```

```bash
cargo run --release -p codeatlas-server
```

| Endpoint | Purpose |
|---|---|
| `POST /graphql` | GraphQL |
| `GET /graphql` | GraphiQL (disable with `CODEATLAS_GRAPHIQL=false`) |
| `GET /health` | `200 {"status":"ok","neo4j":"ok",…}` or `503` when Neo4j is unreachable |

| Variable | Default | Purpose |
|---|---|---|
| `CODEATLAS_HTTP_ADDR` | `127.0.0.1:8080` | Listen address |
| `CODEATLAS_CORS_ORIGINS` | `http://localhost:5173,http://127.0.0.1:5173` | Browser origins allowed to call the API |
| `CODEATLAS_ALLOW_INDEXING` | `true` | Enables `indexRepository` / `removeRepository` |
| `CODEATLAS_GRAPHIQL` | `true` | Serves GraphiQL |
| `CODEATLAS_CLONE_DIR` | as for the CLI | Where URLs given to `indexRepository` are cloned |
| `CODEATLAS_NEO4J_*` | see README | Database connection |

`codeatlas-server --print-schema` prints the SDL without connecting.

## Operations

Every repository argument accepts a repository ID or a unique repository
name. Symbol arguments take exact symbol IDs (use `searchSymbols` to find
them).

| Operation | Returns |
|---|---|
| `repositories`, `repository(id)` | Index metadata (`null` for an unknown ID) |
| `symbol(repoId, id)` | One symbol, including its unresolved calls |
| `crates(repoId)` | Crates with their kind and root module (libraries first); the roots of the explorer tree |
| `children(repoId, id)` | Symbols directly contained in a module or type: modules, traits, types, functions, then methods |
| `searchSymbols(repoId, query, kinds, first, after)` | Prefix search, cursor-paginated (`first` ≤ 50) |
| `dependencies` / `dependents(repoId, symbolId, depth, relations)` | Breadth-first neighbourhood; each node carries the edge that reached it |
| `dependencyPath(repoId, from, to, maxDepth)` | Shortest path over calls, imports and implementations, or `null` |
| `fileDependencies` / `moduleDependencies(…, direction)` | Aggregated `DEPENDS_ON`, by weight |
| `impact(repoId, symbolId \| file, maxDepth, includeAmbiguous)` | Affected symbols with evidence chains, files, modules, tests and the decomposed score |
| `affectedTests(repoId, symbolId, maxDepth)` | Affected tests with their chains (follows trait dispatch) |
| `circularDependencies(repoId, level)` | Cycles with per-hop evidence |
| `hotspots(repoId, level, first)` | Betweenness, fan-in, fan-out |
| `layers(repoId, level)` | Dependency layers |
| `architectureGraph(repoId, level)` | Crate, module or file dependency graph: nodes with fan-in, fan-out and cycle membership; weighted edges with the relation kinds behind them. Crate edges are module dependencies rolled up by crate |
| `source(repoId, file, startLine, endLine)` | Up to 400 lines from the repository's working tree |
| `indexRepository(source)` | Analyse a path or URL and replace its graph |
| `removeRepository(id)` | Delete a repository's graph |

Git-diff impact (`gitImpact`) is not part of the API yet; it arrives with
Milestone 7.

### Example

```graphql
query Impact($repo: ID!) {
  impact(repoId: $repo, symbolId: "fn:change_impact::payments::validate_amount") {
    directCount
    indirectCount
    score { total level factors { name value contribution } }
    affected {
      depth
      confidence
      symbol { id file line isTest }
      path { source target kind file lines }
    }
    tests
  }
}
```

## Errors

Errors are standard GraphQL errors with a machine-readable
`extensions.code`:

| Code | When |
|---|---|
| `NOT_FOUND` | Unknown repository, symbol or file (nullable lookups such as `repository` return `null` instead) |
| `BAD_USER_INPUT` | Invalid argument: depth or page size out of range, both or neither of `symbolId` / `file`, a source path outside the repository |
| `AMBIGUOUS` | A repository name matches several repositories |
| `OUTDATED_INDEX` | The repository was indexed by an older CodeAtlas; index it again |
| `FORBIDDEN` | Indexing mutations are disabled |
| `INDEX_FAILED` | Analysis of the given source failed (the message says why) |
| `SOURCE_UNAVAILABLE` | The file exists but is too large or unreadable |
| `INTERNAL` | Anything else. The message is generic; details go to the server log |

## Limits and safety

* **Query shape.** Nesting depth ≤ 12 and static complexity ≤ 1,000 per
  document, enforced by async-graphql before execution.
* **Traversal size.** Depth ≤ 10 for every traversal, ≤ 500 nodes per
  neighbourhood, ≤ 5,000 affected symbols per impact report. Results say
  when they were truncated.
* **Source reads** accept only relative paths without `..` or absolute
  components. They are resolved and checked to stay inside the
  repository root, symlinks included. Files over 2 MiB are refused.
* **Indexing** makes the server read any local path or clone any URL it can
  reach. The server binds to `127.0.0.1` by default and warns at start-up if
  it listens elsewhere with indexing enabled; set
  `CODEATLAS_ALLOW_INDEXING=false` on shared deployments.
* **CORS** allows only the configured origins. There is no authentication:
  the server is meant to run next to the developer, like a language server.

## Graph cache

Impact, cycle, hotspot and layer queries run on an in-memory `CodeGraph`.
The server caches one per repository (up to 8, least recently used
evicted) and reloads it when the repository's `indexed_at` changes. The
stored format version is checked on every access, cached or not. On ripgrep
(3,536 symbols), a single local run measured the first `impact` request at
116 ms, including the load from Neo4j, and repeats at about 2 ms.
