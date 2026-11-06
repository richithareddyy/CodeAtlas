# CodeAtlas

You change a function in a Rust project. What else uses it, and which tests should you run? CodeAtlas reads the source code, traces those connections, and shows the files and lines behind each result. It helps you investigate a change before merging it.

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

**Keep uncertain connections visible.** The parser, tree-sitter, reads code structure but does not have the Rust compiler’s type information. When a call could refer to more than one definition, CodeAtlas records why it is uncertain and excludes it unless you ask to include ambiguous calls.

**The analysis works without a model.** An optional local model through Ollama can explain the report. It receives numbered analysis facts, and its answer is checked against those facts.

**Check the suggested tests against actual failures.** `probe-tests` deliberately makes functions panic, one at a time, and records which tests fail. On ripgrep, CodeAtlas finds only 4.7% of the affected tests, or 22.8% when ambiguous calls are included. Many tests come from macros that it cannot expand. The suggestions help with investigation; they cannot replace the full suite. See [test-impact quality](GUIDE.md#test-impact-quality).

## Limits

Macros, conditional compilation (`#[cfg]`), and indirect calls can hide connections from the analysis. A dependency path shows what might be affected; it does not prove that a change breaks anything. Analysis takes about 330 ms on ripgrep and 920 ms on tokio on an Apple M5, and incremental indexing is 4–8 times faster than a full index when little changed ([method and raw results](benchmarks/README.md)).

Run `cargo test --workspace` for the Rust suite; database-dependent checks need Neo4j.

[Architecture](docs/architecture.md) · [Symbol resolution](docs/symbol-resolution.md) · [GraphQL API](docs/api.md) · [Full guide](GUIDE.md)
