# CodeAtlas

CodeAtlas analyses a Git repository, turns its source code into a dependency
graph, and answers change-impact questions: *if I change this function,
type, module or file, what else could be affected, and why?*

Answers come from static analysis and graph traversal, not from a language
model guessing about the code. Every conclusion is meant to be traceable to
source locations.

> **Status: Milestone 1 of 11.** Ingestion, source discovery, repository
> metadata, Cargo layout mapping, tree-sitter parsing and symbol/reference
> extraction for Rust are implemented and tested. Symbol resolution, the
> Neo4j graph, impact analysis, the GraphQL API and the web UI are not built
> yet. See [docs/architecture.md](docs/architecture.md#milestones).

## Why static analysis

Impact analysis is only useful if it can be trusted and explained. A parser
sees exactly what is written; a symbol table decides what each name refers
to, or reports that it cannot. A graph traversal over those facts produces
a result that can be reproduced and justified edge by edge. Rust is the first
supported language because its module system and explicit `use` paths allow
a high share of references to be resolved without full type inference.

## Architecture

```
ingest → layout → tree-sitter parse → extract → resolve → Neo4j → algorithms / impact → GraphQL → SvelteKit
└──────────────── implemented ────────────────┘
```

* `crates/analyzer`: the analysis core, a pure library with no database code.
* `crates/cli`: the `codeatlas` command.
* `fixtures/`: small Rust repositories whose contents tests assert against.
* `docs/`: [architecture](docs/architecture.md) and the
  [graph model](docs/graph-model.md).

## What is extracted today

For each Rust file: modules (file and inline), structs, enums, traits,
functions, methods (inherent, trait-impl, trait default and required), test
functions (`#[test]`, `#[tokio::test]`, …), `#[cfg(test)]` scopes, flattened
`use` imports, impl blocks, and every call site with its enclosing caller.
Calls inside `assert!`-style macros are recovered by re-parsing macro
arguments.

Each symbol has a stable ID (`method:shop::payments::PaymentService::authorize`),
kind, name, qualified name, file, line span, parent, visibility and
signature. See [Extraction](docs/architecture.md#extraction-analyzersrcsymbols).

## Installation

Requirements: Rust 1.80+ (`rustup` recommended) and `git` on `PATH`.

```bash
git clone <this repository> codeatlas
```

```bash
cd codeatlas && cargo build --release
```

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
Parsing     6 files, 69 LOC, 0 with syntax errors, parse 0.7 ms, total 1.1 ms
```

The fixture lives inside this repository, so its revision line shows the
enclosing checkout's branch and commit.

Analyse a remote repository (cloned into the clone directory first):

```bash
./target/release/codeatlas analyze https://github.com/BurntSushi/ripgrep
```

Full machine-readable output:

```bash
./target/release/codeatlas analyze . --format json --output analysis.json
```

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
| `RUST_LOG` | `warn,codeatlas_analyzer=info` | Log filter; logs go to stderr |

## Testing

```bash
cargo test
```

* Unit tests sit next to the code they cover: discovery, language
  detection, layout mapping, `use`-tree flattening, attribute handling,
  parsing and extraction.
* `crates/analyzer/tests/fixtures.rs` analyses `fixtures/simple-repo` and
  `fixtures/duplicate-symbols` end to end. The expected symbols, lines and
  calls were written from the fixture sources.
* `crates/analyzer/tests/git_ingest.rs` builds Git repositories in temporary
  directories to test branch/SHA detection, detached HEAD, cloning from a
  URL, updating a clone, and clone failures. These tests need `git`.

Formatting and lints, as enforced in CI:

```bash
cargo fmt --all --check
```

```bash
cargo clippy --all-targets -- -D warnings
```

## Benchmarks

The benchmark harness (`codeatlas bench`, with JSON results under
`benchmarks/results/`) is planned for Milestone 10. No performance figures
are published until it exists.

## Known limitations

* Only Rust is parsed. Other languages are counted in repository statistics
  but not analysed.
* Module paths come from file paths and Cargo target declarations;
  `#[path]` attributes and `include!` are not followed.
* Macros: calls inside `(..)` / `[..]` macro arguments are recovered;
  brace-delimited macro bodies and macro-generated items are not.
* `#[cfg(...)]` is not evaluated. All configurations are analysed, and
  duplicate definitions receive `#N` ID suffixes.
* Call sites are not resolved to definitions yet (Milestone 2).
* LOC counts non-blank lines, including comments.

## Roadmap

Symbol resolution → Neo4j graph and bounded queries → graph algorithms and
change-impact engine → GraphQL API → SvelteKit workspace UI (MVP) → Git diff
impact → incremental indexing → test-impact evaluation → architecture and
cycle views with benchmarks → optional local-model explanations grounded in
graph evidence. Details are in [docs/architecture.md](docs/architecture.md).
