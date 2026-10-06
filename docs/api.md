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
| `CODEATLAS_STATE_DIR` | `~/.cache/codeatlas/index` | Incremental indexing state, one file per repository |
| `CODEATLAS_NEO4J_*` | see README | Database connection |
| `CODEATLAS_OLLAMA_URL`, `CODEATLAS_OLLAMA_MODEL`, `CODEATLAS_OLLAMA_TIMEOUT_SECS` | `http://127.0.0.1:11434`, `llama3.2`, 120 | Local model for `explainImpact` |
| `CODEATLAS_EXPLAIN` | unset | `template` disables the model for `explainImpact` |

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
| `testImpact(repoId, symbolIds, maxDepth, includeAmbiguous)` | Tests to run for one or more changed symbols (up to 100): direct and transitive tests with their chains and the changed symbols each reaches, possible tests (ambiguous calls), changed tests, and changed functions no test reaches |
| `explainImpact(repoId, symbolId, affectedId, maxDepth, useModel)` | Why changing `symbolId` can affect `affectedId` (or, without `affectedId`, what it affects): the status (`SUFFICIENT`, `POSSIBLE`, `INSUFFICIENT` with a `reason`), numbered `facts` with their symbols, file and lines, `chains` of fact IDs, and `text` citing the facts as `[E1]`. `source` says whether the text is `TEMPLATE` (built from the facts) or `MODEL` (written by `model` and accepted by the check in `verification`); a rejected answer is returned in `rejectedText`, and `notes` say what happened |
| `circularDependencies(repoId, level)` | Cycles with per-hop evidence |
| `hotspots(repoId, level, first)` | Betweenness, fan-in, fan-out |
| `layers(repoId, level)` | Dependency layers |
| `architectureGraph(repoId, level)` | Crate, module or file dependency graph: nodes with fan-in, fan-out and cycle membership; weighted edges with the relation kinds behind them. Crate edges are module dependencies rolled up by crate |
| `source(repoId, file, startLine, endLine)` | Up to 400 lines from the repository's working tree |
| `gitRefs(repoId, first)` | Branches, remote-tracking branches and tags (newest first), recent commits, current branch |
| `gitImpact(repoId, base, head, maxDepth, includeAmbiguous)` | Changes between two revisions (`head: null` is the working tree): changed files with hunks; added, removed, modified, moved and cosmetic symbols with changed lines and signature changes; downstream symbols with evidence chains; affected files, modules and tests; modified functions no test reaches (`untested`); a summary |
| `indexRepository(source, full)` | Analyse a path or URL and store its graph: incrementally when this server indexed it before (only changed files parsed, only the difference written), in full otherwise or with `full: true`. The result reports the mode, the reason for a full write, files changed / parsed / reused, and nodes and relationships added, removed and changed |
| `removeRepository(id)` | Delete a repository's graph |

`gitImpact` analyses both revisions from the repository's Git history on
the server (the clone made by `indexRepository` for URLs). It does not use
the stored graph, so it works for any two commits, not only the indexed
one; each request analyses both revisions again (about 0.8 s on ripgrep),
and at most two run at once. Revisions are resolved with `git rev-parse`;
anything starting with `-` is rejected. See
[impact-analysis.md](impact-analysis.md#git-diff-impact) for the method.

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
| `NOT_FOUND` | Unknown repository, symbol, file or Git revision (nullable lookups such as `repository` return `null` instead) |
| `BAD_USER_INPUT` | Invalid argument: depth or page size out of range, both or neither of `symbolId` / `file`, a source path outside the repository, a revision that looks like an option |
| `AMBIGUOUS` | A repository name matches several repositories |
| `OUTDATED_INDEX` | The repository was indexed by an older CodeAtlas; index it again |
| `FORBIDDEN` | Indexing mutations are disabled |
| `INDEX_FAILED` | Analysis of the given source failed (the message says why) |
| `SOURCE_UNAVAILABLE` | The file exists but is too large or unreadable, or the repository directory is gone from the server |
| `NOT_A_GIT_REPOSITORY` | `gitRefs` / `gitImpact` on a repository without Git history |
| `DIFF_FAILED` | Exporting or analysing a revision failed (the message says why) |
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
* **Explanations** send only the question and the facts to the configured
  model server, never source code, and never anything when the evidence
  is insufficient. Each model call is bounded by
  `CODEATLAS_OLLAMA_TIMEOUT_SECS`; Ollama queues concurrent requests
  itself. Point `CODEATLAS_OLLAMA_URL` only at a server you trust with
  symbol and file names.
* **CORS** allows only the configured origins. There is no authentication:
  the server is meant to run next to the developer, like a language server.

## Graph cache

Impact, cycle, hotspot and layer queries run on an in-memory `CodeGraph`.
The server caches one per repository (up to 8, least recently used
evicted) and reloads it when the repository's `indexed_at` changes. The
stored format version is checked on every access, cached or not. On ripgrep
(3,536 symbols), a single local run measured the first `impact` request at
116 ms, including the load from Neo4j, and repeats at about 2 ms.
