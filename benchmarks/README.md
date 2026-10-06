# Benchmarks

Measurements of CodeAtlas on real repositories, produced by `codeatlas
bench` and stored as JSON in [results/](results). Every number in the
project's documentation that is not marked as a single run comes from one
of these files. Test-selection precision and recall have their own ground
truth in [test-impact/](test-impact).

## What is measured

| Measurement | How |
|---|---|
| Corpus | Rust files and non-blank lines analysed, symbols by kind, tests, call sites; relationships by kind (resolved calls, imports, implementations, ambiguous-call candidates); nodes and relationships stored in Neo4j |
| Resolution | Share of call sites that may target repository code and resolve to exactly one definition |
| Analysis time and throughput | `--runs` analyses, each in a fresh process: discovery, parsing (both passes), resolution. Throughput is Rust LOC per second of the median total and of the median parse time |
| Memory | Peak resident memory of each analysis process (`getrusage`, Unix only) |
| Full indexing | End to end (discovery, analysis, Neo4j write) without saved state, so everything is parsed and written; `--runs` times |
| Incremental indexing | With saved state: nothing changed; one line inserted at the top of the largest file (alternately added and removed), which shifts every line of that file; and, with `--base`, the change from that revision to `HEAD` and back, from commits exported with `git archive` |
| Query latency | Store queries through the driver (Neo4j) for `--targets` symbols (half the most-called functions, half a seeded sample), `--query-runs` times each; and analyses on the graph loaded into memory |
| Test impact | Precision and recall against a `probe-tests` ground truth, when `--test-truth` is given |

Indexing and queries run on a scratch copy of the repository under a
repository ID of their own, deleted afterwards. Medians, minima, maxima and
95th percentiles are reported next to every raw sample in the JSON.

## Results

| File | Repository | Notes |
|---|---|---|
| [ripgrep-3fce3b5.json](results/ripgrep-3fce3b5.json) | ripgrep `3fce3b5` | `--base HEAD~10`, with test-impact ground truth |
| [codeatlas-5de56d0.json](results/codeatlas-5de56d0.json) | CodeAtlas `5de56d0` | `--base HEAD~3` |
| [tokio-b2636752.json](results/tokio-b2636752.json) | tokio `b263675` | `--base HEAD~20` |

All three: `--runs 5`, 20 query targets × 3 runs, seed 1, release build,
Apple M5 (10 logical CPUs), macOS, Neo4j 5.26.31 community in Docker. A
summary table is in the main [README](../README.md#benchmarks).

## Running

```bash
cargo build --release
```

```bash
./target/release/codeatlas bench <path> --runs 5 --base HEAD~10 -o results/<name>-<commit>.json
```

`--no-database` measures analysis only. Indexing and queries need Neo4j
(see the main README). Timings depend on the machine, the Neo4j
configuration and what else is running; compare runs from the same
machine.

## Result format

`schema` 1. Top-level fields: `codeatlas` (version), `build` (`release` or
`debug`), `measured_at`, `repository` (name, origin URL, commit),
`environment` (OS, architecture, CPU, logical CPUs, Neo4j version),
`parameters`, `corpus`, `analysis` (samples and summaries), `indexing`
(series `full`, `unchanged`, `one_file`, `range_forward`, `range_back`,
each with samples including the write delta), `queries` (name, where it
runs, latency summary), `test_impact` and `notes` (what was not measured).
