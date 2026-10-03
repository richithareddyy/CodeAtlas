# Change impact and graph algorithms

CodeAtlas answers *"if I change this, what else could be affected, and
why?"* by traversing the resolved code graph. The answer is deterministic:
the same graph and the same request always give the same result, and every
reported symbol comes with the chain of source facts that connects it to
the change. Code: `crates/analyzer/src/graph/`.

## Which edges propagate impact

Impact flows backwards along dependencies. In one step, symbol `S` becomes
affected through a symbol `T` that is already affected (or changed) when:

| Rule | Condition | Evidence step |
|---|---|---|
| Calls | `S` calls `T` (resolved `CALLS`) | `S calls T` with the call lines in `S`'s file |
| Dispatch | `T` is an implementation `<X as Tr>::m` and `S` is the trait method `Tr::m` | `calls to S may dispatch to T` |
| Implementations | `T` is a **changed** trait method and `S` implements it | `S implements T` |
| Ambiguous calls (opt-in) | `S` has an ambiguous call with `T` as a candidate | `S may call T` |

Three decisions behind these rules:

* **Implementations only follow changed trait methods.** Changing
  `StripeGateway::charge` reaches `Gateway::charge` (dispatch) and its
  callers, but not `FakeGateway::charge`. Changing the declaration
  `Gateway::charge` does reach every implementation, because they must
  match it.
* **Imports do not propagate.** A `use` alone runs no code. Callers are
  found through `CALLS`; module coupling is reported by the architecture
  analyses.
* **Ambiguous calls are opt-in and marked.** With `--include-ambiguous`,
  symbols reachable *only* through an ambiguous call are reported as
  `possible`, in a separate list, and never counted in the score.

## What counts as changed

| Requested | Changed set |
|---|---|
| function / method | the symbol |
| struct / enum / trait | the type and its methods |
| module | every symbol written in the module or its submodules |
| file (`--file`) | every symbol defined in the file |
| several symbols (Milestone 7, diffs) | the union |

Changed symbols are not listed as affected.

## Traversal

Breadth-first search from all changed symbols at once
(`algorithms::bfs`), so each affected symbol gets its smallest depth and a
shortest evidence chain. A first pass follows only certain edges; with
ambiguous calls enabled, a second pass adds the symbols that only it
reaches. The search is bounded by depth (default 8, at most 10 through the
CLI) and by 5,000 affected symbols. Hitting a bound sets `truncated`.

## Report

* `affected`: symbol, depth, confidence (`certain` / `possible`) and the
  evidence path (`path[0].source` is the affected symbol; the last step's
  `target` is a changed symbol).
* `files`, `modules`: affected symbols grouped by where their code is
  written, with test counts.
* `tests`: affected test functions. These are the tests to run, with their
  chains.
* `score`: see below.

## Blast-radius score

The score summarises how much of the graph the change reaches. It does not
predict whether anything will break. It is the sum of six factor
contributions, each shown in the output:

```
contribution = 100 × weight × min(1, ln(1 + value) / ln(1 + saturation))
```

| Factor | Meaning | Weight | Saturation |
|---|---|---|---|
| `direct_dependents` | certain affected symbols at depth 1 | 0.20 | 10 |
| `transitive_dependents` | all certain affected symbols | 0.25 | 100 |
| `affected_modules` | distinct modules of affected symbols | 0.20 | 10 |
| `affected_tests` | affected test functions | 0.15 | 25 |
| `fan_in` | distinct direct callers of the changed symbols | 0.10 | 10 |
| `dependency_depth` | largest depth reached | 0.10 | 6 |

The logarithm makes the first few dependents count most, and the
saturation caps each factor so one large value cannot dominate. Levels:
`low` below 25, `medium` below 60, `high` otherwise. Weights and
saturations are fixed constants (`ImpactScore::FACTORS`), not fitted to
data; they express a judgement about reach and are documented here so
they can be argued with.

## Architecture analyses

`graph::architecture` runs on three projections:

| Level | Nodes | Edges |
|---|---|---|
| function | functions and methods | resolved `CALLS` (cycles are recursion) |
| file | files | derived `DEPENDS_ON` |
| module | modules (file and inline) | derived `DEPENDS_ON` |

* **Cycles.** Strongly connected components (iterative Tarjan), largest
  first. For each one, a shortest cycle through it is shown hop by hop,
  each hop with up to five underlying symbol edges and their file and line.
* **Hotspots.** Betweenness centrality (Brandes; shortest dependency paths
  between other nodes that pass through a node) with fan-in and fan-out.
* **Layers.** The condensation (each cycle collapsed to one node) ordered
  by longest dependency chain: layer 0 depends on nothing at that level,
  and every node sits one above the highest layer it depends on.

`graph::algorithms` contains the generic, unit-tested implementations:
BFS with predecessor edges, shortest path, Tarjan SCC, shortest cycle
through a component, topological order (Kahn), dependency layering, degrees
and betweenness.

## Where it runs

The algorithms run on an in-memory `CodeGraph`, built from an analysis or
loaded from Neo4j (`GraphStore::load_graph`). An integration test asserts
that both paths produce identical graphs, on every fixture and, as an
opt-in run, on ripgrep. Impact results through the database are therefore
the same as in-memory results.

## Limitations

* Calls the resolver could not pin down (ambiguous or unresolved) can hide
  real dependents. Use `--include-ambiguous` to see the ambiguous ones;
  unresolved calls are listed on each function.
* Type usage (`REFERENCES`) is not tracked yet. Changing a struct reaches
  the callers of its methods, not code that only constructs it or reads
  its fields.
* Trait calls through generic parameters are ambiguous (see
  [symbol-resolution.md](symbol-resolution.md)), so they only appear as
  possible.
* Macro-generated code and `#[cfg]`-disabled code are not analysed.
