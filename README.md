# CodeAtlas

CodeAtlas analyses a Git repository, turns its source code into a dependency
graph, and answers change-impact questions: *if I change this function,
type, module or file, what else could be affected, and why?*

Answers come from static analysis and graph traversal, not from a language
model guessing about the code. Every conclusion is meant to be traceable to
source locations.

> **Status: Milestone 4 of 11.** Ingestion, Rust module-tree construction,
> tree-sitter parsing, symbol extraction, symbol resolution (with measured
> quality), the Neo4j code graph, bounded graph queries, graph algorithms
> and the change-impact engine are implemented and tested. The GraphQL API
> and the web UI are not built yet. See
> [docs/architecture.md](docs/architecture.md#milestones).

## Why static analysis

Impact analysis is only useful if it can be trusted and explained. A parser
sees exactly what is written; a symbol table decides what each name refers
to, or reports that it cannot. A graph traversal over those facts produces
a result that can be reproduced and justified edge by edge. Rust is the first
supported language because its module system and explicit `use` paths allow
a high share of references to be resolved without full type inference.

## Architecture

```
ingest → module tree → tree-sitter parse → extract → resolve → Neo4j + queries → algorithms / impact → GraphQL → SvelteKit
└────────────────────────────────────── implemented ──────────────────────────────────────┘
```

* `crates/analyzer`: the analysis core, a pure library with no database code.
* `crates/store`: Neo4j persistence and bounded graph queries.
* `crates/cli`: the `codeatlas` command.
* `fixtures/`: small Rust repositories with hand-written ground truth
  (`expected.json`).
* `docs/`: [architecture](docs/architecture.md),
  [graph model](docs/graph-model.md),
  [symbol resolution](docs/symbol-resolution.md) and
  [change impact and graph algorithms](docs/impact-analysis.md).

## What it does today

**Extraction.** For each Rust file: modules, structs, enums, traits,
functions, methods (inherent, trait-impl, trait default and required),
tests (`#[test]`, `#[tokio::test]`, …), `#[cfg(test)]` scopes, imports
(including `extern crate … as …`), type aliases, impl blocks, local
bindings, struct field types and every call site with its enclosing caller.
Calls inside `assert!`-style macros are recovered by re-parsing macro
arguments. Each symbol has a stable ID
(`method:shop::payments::PaymentService::authorize`), kind, name, qualified
name, file, line span, parent, visibility and signature.

**Module tree.** Files are placed in crates and modules by following `mod`
declarations from each crate root, including `#[path]`, as the compiler
does.

**Resolution.** Every call site is classified as *resolved* (one
definition, becomes a `CALLS` edge), *ambiguous* (candidates and a reason
are kept), *unresolved* (with a reason), *external*, *constructor* or
*local*. Paths are resolved with Rust's scoping and visibility rules;
method calls through the receiver's type where it can be determined from
parameters, fields, constructor calls, `?` and builder chains. Nothing is
matched by name similarity. Details:
[docs/symbol-resolution.md](docs/symbol-resolution.md).

**Code graph.** Repositories are stored in Neo4j as symbols, files and
crates connected by `CALLS`, `IMPORTS`, `IMPLEMENTS`, `CONTAINS`,
`DEFINES`, `DEPENDS_ON` (derived file and module dependencies) and
`CALLS_CANDIDATE` (ambiguous calls, kept apart from resolved ones).
Queries answer *who calls this*, *what does this call*, *what depends on
this transitively*, *which tests reach this*, *how are A and B connected*
and *which files or modules depend on this*. Every query has depth and size
limits, and every result carries the edges and source lines that justify
it. Schema and queries: [docs/graph-model.md](docs/graph-model.md).

**Change impact.** Given a changed function, method, type, module or file,
CodeAtlas lists every symbol that could be affected, with depth, the
affected files, modules and tests, and for each one the chain of facts
connecting it to the change: *`test_checkout` calls `checkout` (line 16),
which calls `authorize` (line 5), which calls `validate_amount` (line 13)*.
Impact follows resolved calls and trait dispatch. Ambiguous calls are
followed only on request and reported as *possible*. A blast-radius score
summarises the reach and is broken down into its six factors; it measures
reach, not the likelihood of breakage. Details:
[docs/impact-analysis.md](docs/impact-analysis.md).

**Architecture.** Circular dependencies (Tarjan SCC) at module, file and
function level, each with a shortest cycle and the source lines of every
hop; hotspots by betweenness centrality and fan-in/fan-out; and dependency
layers with cycles collapsed.

## Installation

Requirements: Rust 1.80+ (`rustup` recommended), `git` on `PATH`, and
Docker for the graph database (`analyze` and `evaluate` work without it).

```bash
git clone <this repository> codeatlas
```

```bash
cd codeatlas && cargo build --release
```

Start Neo4j. Copy `.env.example` to `.env` and set
`CODEATLAS_NEO4J_PASSWORD` (at least 8 characters) first:

```bash
cp .env.example .env
```

```bash
docker compose up -d
```

`docker-compose.yml` runs `neo4j:5.26-community` with the password from
`.env`, a named data volume, and the browser UI on
<http://localhost:7474>.

## Usage

Analyse a local repository:

```bash
./target/release/codeatlas analyze fixtures/simple-repo
```

```
Repository  simple-repo  (/…/codeatlas/fixtures/simple-repo)
Revision    main @ <commit sha>
Languages   Rust 6 files / 69 LOC
Crates      checkout_test (test), simple_repo (lib)
Symbols     7 modules, 2 structs, 1 enums, 0 traits, 6 functions, 2 methods (2 tests)
References  14 call sites (5 inside macros), 8 imports, 1 impl blocks
Calls       8 resolved, 0 ambiguous, 0 unresolved; 6 external, 0 constructors, 0 local; resolution rate 100.0%
Imports     8 resolved, 0 external, 0 unresolved
Parsing     6 files, 69 LOC, 0 with syntax errors, parse 4.6 ms, resolve 0.1 ms, total 5.7 ms
```

The fixture lives inside this repository, so its revision line shows the
enclosing checkout's branch and commit.

Analyse a remote repository (cloned into the clone directory first):

```bash
./target/release/codeatlas analyze https://github.com/BurntSushi/ripgrep
```

Resolution breakdown, with the ambiguous and unresolved call sites listed:

```bash
./target/release/codeatlas analyze fixtures/cross-module --format resolution
```

Compare resolution against a ground-truth file (exits non-zero on any
difference):

```bash
./target/release/codeatlas evaluate fixtures/cross-module
```

Full machine-readable output (symbols, edges, ambiguous and unresolved
sites, statistics):

```bash
./target/release/codeatlas analyze . --format json --output analysis.json
```

### Graph commands (need Neo4j)

Index a repository (replaces any earlier index of it):

```bash
./target/release/codeatlas index fixtures/simple-repo
```

```
Indexed     simple-repo (edf568848e9517df)
Graph       27 nodes, 55 relationships, written in 42 ms
Labels      Crate 2, Enum 1, File 6, Function 6, Method 2, Module 7, Repository 1, Struct 2, Symbol 18, Test 2
Relations   CALLS 8, CONTAINS 15, DEFINES 11, DEPENDS_ON 13, IMPORTS 8
```

Query it. `--repo` takes an ID or name and can be omitted when only one
repository is indexed. Symbols can be given as an ID, a qualified name,
`Type::method` or a unique name. `--json` prints machine-readable output.

```bash
./target/release/codeatlas query -r simple-repo callers PaymentService::authorize --depth 3
```

```
Dependents of method:simple_repo::payments::PaymentService::authorize (src/payments/mod.rs:17), depth <= 3
  1  fn:simple_repo::payments::process_payment                    src/payments/mod.rs:25
       via fn:simple_repo::payments::process_payment -[CALLS receiver_type @27]-> method:simple_repo::payments::PaymentService::authorize
  1  fn:simple_repo::payments::tests::rejects_amounts_over_limit  src/payments/mod.rs:35
       via fn:simple_repo::payments::tests::rejects_amounts_over_limit -[CALLS receiver_type @37]-> method:simple_repo::payments::PaymentService::authorize
  2  fn:simple_repo::checkout::checkout                           src/checkout.rs:9
       via fn:simple_repo::checkout::checkout -[CALLS import @11]-> fn:simple_repo::payments::process_payment
  3  fn:checkout_test::checkout_succeeds_for_small_orders         tests/checkout_test.rs:4
       via fn:checkout_test::checkout_succeeds_for_small_orders -[CALLS import @9]-> fn:simple_repo::checkout::checkout
```

```bash
./target/release/codeatlas query -r simple-repo tests stripe_call
```

```
fn:simple_repo::payments::tests::rejects_amounts_over_limit  (depth 2, src/payments/mod.rs:35)
    fn:simple_repo::payments::tests::rejects_amounts_over_limit -[CALLS receiver_type @37]-> method:simple_repo::payments::PaymentService::authorize
    method:simple_repo::payments::PaymentService::authorize -[CALLS scope @21]-> fn:simple_repo::payments::gateway::stripe_call
fn:checkout_test::checkout_succeeds_for_small_orders  (depth 4, tests/checkout_test.rs:4)
    fn:checkout_test::checkout_succeeds_for_small_orders -[CALLS import @9]-> fn:simple_repo::checkout::checkout
    fn:simple_repo::checkout::checkout -[CALLS import @11]-> fn:simple_repo::payments::process_payment
    fn:simple_repo::payments::process_payment -[CALLS receiver_type @27]-> method:simple_repo::payments::PaymentService::authorize
    method:simple_repo::payments::PaymentService::authorize -[CALLS scope @21]-> fn:simple_repo::payments::gateway::stripe_call
```

Impact of a change:

```bash
./target/release/codeatlas query -r change-impact impact validate_amount
```

```
Impact of fn:change_impact::payments::validate_amount (src/payments.rs:18)
Affected    4 symbols (1 direct, 3 indirect) in 3 files and 3 modules; 2 tests
Score       41.2 / 100 (Medium): size of the affected graph, not a probability of breakage
  direct_dependents          1  saturates at 10   weight 0.20  ->   5.8
  transitive_dependents      4  saturates at 100  weight 0.25  ->   8.7
  affected_modules           3  saturates at 10   weight 0.20  ->  11.6
  affected_tests             2  saturates at 25   weight 0.15  ->   5.1
  fan_in                     1  saturates at 10   weight 0.10  ->   2.9
  dependency_depth           3  saturates at 6    weight 0.10  ->   7.1

Affected symbols (depth, symbol, location, then why)
   1  method:change_impact::payments::PaymentService::authorize src/payments.rs:12
        change_impact::payments::PaymentService::authorize calls change_impact::payments::validate_amount  (src/payments.rs:13)
   …
   3  fn:payment_tests::test_checkout tests/payment_tests.rs:14  [test]
        payment_tests::test_checkout calls change_impact::checkout::checkout  (tests/payment_tests.rs:16)
        change_impact::checkout::checkout calls change_impact::payments::PaymentService::authorize  (src/checkout.rs:5)
        change_impact::payments::PaymentService::authorize calls change_impact::payments::validate_amount  (src/payments.rs:13)
…
Tests to run
  fn:payment_tests::test_authorize_valid
  fn:payment_tests::test_checkout
```

`impact` also takes `--file <path>`, `--depth n`, `--include-ambiguous` and
`--limit n`. Circular dependencies, with the lines behind each hop:

```bash
./target/release/codeatlas query -r circular-dependency cycles --level module
```

```
1 Module-level cycles

1. 3 members: mod:circular_dependency::checkout, mod:circular_dependency::orders, mod:circular_dependency::payments
   mod:circular_dependency::checkout -> mod:circular_dependency::payments  (2 edges: CALLS, IMPORTS)
       CALLS      fn:circular_dependency::checkout::checkout -> fn:circular_dependency::payments::charge  src/checkout.rs:5
       IMPORTS    mod:circular_dependency::checkout -> mod:circular_dependency::payments  src/checkout.rs:1
   …
```

Other queries: `repos`, `search <text> [--kind method]`, `symbol <s>`,
`callees <s> [--depth n]`, `path <from> <to>`, `file-dependents <path>`,
`file-dependencies <path>`, `module-dependents <module>`,
`module-dependencies <module>`, `hotspots [--level function|file|module]`,
`layers [--level …]`, `cycles --level function|file|module`. Remove an
indexed repository with `codeatlas remove <repo>`. If a repository was
indexed by an older version of CodeAtlas, the graph-based analyses ask you
to index it again.

### Debugging the extractor

Inspect the tree-sitter syntax tree of a file (useful when extending the
extractor):

```bash
./target/release/codeatlas ast src/lib.rs
```

### Configuration

| Variable / flag | Default | Purpose |
|---|---|---|
| `CODEATLAS_CLONE_DIR` / `--clone-dir` | `$XDG_CACHE_HOME/codeatlas/repos`, else `~/.cache/codeatlas/repos` | Where remote repositories are cloned |
| `--no-gitignore` | off | Also analyse files excluded by `.gitignore` |
| `--limit` | 20 | Call sites listed per category by `--format resolution` |
| `CODEATLAS_NEO4J_PASSWORD` | none (required for graph commands) | Neo4j password; also used by `docker-compose.yml` |
| `CODEATLAS_NEO4J_URI` | `bolt://localhost:7687` | Neo4j Bolt address |
| `CODEATLAS_NEO4J_USER` | `neo4j` | Neo4j user |
| `CODEATLAS_NEO4J_DATABASE` | `neo4j` | Neo4j database |
| `RUST_LOG` | `warn,codeatlas_analyzer=info,codeatlas_store=info` | Log filter; logs go to stderr |

Variables are read from the environment and, if present, from `.env` in the
working directory or its parents; real environment variables take
precedence. `.env` is git-ignored.

## Testing

```bash
cargo test
```

* Unit tests sit next to the code they cover: discovery, layout, module
  tree, `use`-tree flattening, attributes, parsing, extraction, and one test
  per resolution rule (scoping, shadowing, visibility, re-exports, aliases,
  receiver inference, re-homing, cyclic globs, …).
* `crates/analyzer/tests/fixtures.rs` checks extraction on the fixtures.
* `crates/analyzer/tests/resolution.rs` compares resolution on every
  fixture with its `expected.json` and fails on any missing or unexpected
  edge, ambiguous call or unresolved call.
* `crates/analyzer/tests/impact.rs` checks impact results on the
  `change-impact` fixture (call chains, trait dispatch, trait-method
  changes, type/module/file expansion, ambiguous calls, score inputs,
  limits) and cycles, layers and hotspots on `circular-dependency`. All
  expected values were derived by hand.
* `crates/analyzer/src/graph/algorithms.rs` unit-tests each algorithm on
  hand-built graphs, including a 200,000-node chain for the iterative
  Tarjan.
* `crates/analyzer/tests/git_ingest.rs` builds Git repositories in temporary
  directories (branch/SHA detection, detached HEAD, cloning, updating a
  clone, clone failures). These tests need `git`.
* `crates/store/tests/neo4j.rs` indexes fixtures into a real Neo4j and checks
  stored counts against the analysis, re-indexing, callers and callees with
  evidence paths, related tests, shortest paths, file and module
  dependencies, search, ambiguous and unresolved calls, limits,
  deletion, format-version checks, and that the graph loaded back from
  Neo4j equals the graph built from the analysis on every fixture. With
  `CODEATLAS_EQUIVALENCE_REPO=<path>` that equivalence check also runs on a
  real repository. Each test uses its own repository ID. They run when
  `CODEATLAS_NEO4J_PASSWORD` is set (directly or via `.env`) and print
  `skipping` otherwise; if Neo4j is configured but unreachable they fail.
  CI runs them against a Neo4j service container.

Formatting and lints, as enforced in CI:

```bash
cargo fmt --all --check
```

```bash
cargo clippy --all-targets -- -D warnings
```

## Resolution quality

Measured, not estimated; full details and method in
[docs/symbol-resolution.md](docs/symbol-resolution.md#measured-quality).

* All six fixtures match their hand-written ground truth exactly. They
  were written alongside the resolver, so this shows the rules work as
  designed rather than accuracy on unfamiliar code.
* On ripgrep (commit `3fce3b5`, ~51k non-blank lines), a single run
  resolved 76.3% of call sites that may target repository code; 2,873 were
  ambiguous and 6 unresolved. A random sample of 30 receiver-type edges
  checked by hand was 30/30 correct. That is a spot check, not a precision
  measurement.

## Benchmarks

The benchmark harness (`codeatlas bench`, with JSON results under
`benchmarks/results/`) is planned for Milestone 10. Until it exists, the
only figures in this repository are single-run measurements quoted with
their conditions. For example, indexing ripgrep (commit `3fce3b5`) wrote
3,668 nodes and 19,010 relationships in 0.76–0.91 s over two runs. Warm
`codeatlas query` invocations on that graph took 31–42 ms end to end,
including process start and connection. Impact, cycle and hotspot queries,
which load the whole stored graph (3,536 symbols) into memory, took
103–126 ms end to end. Machine: Apple Silicon Mac, local Docker Neo4j 5.26.

## Known limitations

* Only Rust is parsed. Other languages are counted in repository statistics
  but not analysed.
* No general type inference: element types of iterators and `for` loops,
  `match` / `if let` bindings, generic instantiation and standard-library
  return types are not modelled. Calls on such receivers are reported as
  ambiguous.
* Macro-generated items are invisible. Calls inside `(..)` / `[..]` macro
  arguments are recovered; brace-delimited macro bodies are not.
* `#[cfg(...)]` is not evaluated. All configurations are analysed, and
  duplicate definitions receive `#N` ID suffixes.
* `include!` is not followed.
* LOC counts non-blank lines, including comments.
* Indexing replaces a repository's whole graph; incremental updates are
  planned (Milestone 8).
* `query tests` follows resolved `CALLS` only; use `query impact`, which
  also follows trait dispatch.
* Type usage is not tracked yet, so changing a struct reaches the callers
  of its methods but not code that only constructs it or reads its fields.
* Search matches name prefixes (`store` finds `Store` and its methods, not
  `MemoryStore`).

## Roadmap

GraphQL API → SvelteKit workspace UI (MVP) → Git diff impact →
incremental indexing → test-impact evaluation → architecture and cycle
views with benchmarks → optional local-model explanations grounded in graph
evidence. Details are in [docs/architecture.md](docs/architecture.md).
