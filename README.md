# CodeAtlas

Before merging a change you want to know what it can break and which tests to run, and you want an answer you can check. CodeAtlas does this for Rust repositories using static analysis: it resolves references from parsed source, follows the resulting graph, and links every connection back to the code behind it.

```text
$ codeatlas diff /tmp/pr-shop --base main --head feature
PR impact   main..feature
Files       9 changed: 1 added, 1 removed, 6 modified, 1 renamed
Functions   4 modified, 1 added, 1 removed
Signatures  1 changed
Tests       1 modified

Potential impact (resolved calls and trait dispatch, depth <= 8)
  5 downstream symbols in 3 modules and 3 files; 3 tests to run

Changed symbols
  ~ method pr_shop::payments::PaymentService::authorize    src/payments.rs:13-14
      signature before: pub fn authorize(&self, cents: u64) -> Result
      signature after:  pub fn authorize(&self, amount: u64, currency: &str) -> Result
```

(Trimmed output from the `pr-impact` fixture.) There's also a GraphQL API and a SvelteKit workspace backed by Neo4j for exploring callers, paths, cycles, and architecture views.

![Impact view on the change-impact fixture](docs/images/impact.png)

## Try it

Requires Rust 1.90+ and Git. These need neither Neo4j nor a model:

```bash
cargo build --release
./target/release/codeatlas analyze fixtures/simple-repo
./target/release/codeatlas evaluate fixtures/cross-module
```

For the interactive workspace, follow the [installation guide](GUIDE.md#installation).

## Decisions worth explaining

**Label uncertainty instead of guessing.** tree-sitter gives syntax, not types, so some calls can't be pinned to one definition. The resolver records those as ambiguous, with a reason (receiver type unknown, generic parameter, and so on), and keeps them out of results unless you ask for them.

**The model comes last.** An optional local model (Ollama) can explain an impact report, but it only sees numbered facts from the analysis, and its answer is checked against them. The analysis runs without it.

**Test selection is scored against real failures.** `probe-tests` makes functions panic one at a time and records which tests actually run them, and `evaluate-tests` compares the static selection with that. On ripgrep the result is humbling: recall is 0.047, or 0.228 with ambiguous calls included, mostly because 716 of its 1,195 tests are macro-generated and CodeAtlas doesn't expand `macro_rules!`. For code like that, static selection is a starting point and doesn't replace running the suite. See [test-impact quality](GUIDE.md#test-impact-quality).

## Limits

This is static analysis, not the Rust compiler. Macros, `#[cfg]`, and indirect calls reduce coverage, and an impact path means a dependency exists, not that a change will break it. Analysis takes about 330 ms on ripgrep and 920 ms on tokio on an Apple M5, and incremental indexing is 4–8 times faster than a full index when little changed ([method and raw results](benchmarks/README.md)).

Run `cargo test --workspace` for the Rust suite; database-dependent checks need Neo4j.

[Architecture](docs/architecture.md) · [Symbol resolution](docs/symbol-resolution.md) · [GraphQL API](docs/api.md) · [Full guide](GUIDE.md)
