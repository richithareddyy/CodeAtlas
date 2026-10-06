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

## Test selection

`graph::test_selection` answers *which tests should run for this change?*
using the same traversal as impact analysis, restricted to tests:

* **Direct** tests call changed code: their evidence chain contains one
  call. Dispatch and implementation steps do not count, so a test calling
  `Gateway::charge` is direct for a changed `<Stripe as Gateway>::charge`.
* **Transitive** tests reach changed code through other code.
* **Possible** tests are reached only through ambiguous calls (on request).
* **Changed tests** are tests among the changed symbols; they run too.
* **Untested** changes are changed functions and methods that no test
  reaches. One traversal per changed callable also records, for every
  selected test, which changed symbols it reaches.

Every selected test carries its evidence chain. Diff reports list the
untested modified functions as well (code in test, bench and example
crates excluded).

### Measuring selection against reality

Selection is only useful if it finds the tests that actually exercise a
change, so CodeAtlas measures it against observed behaviour instead of a
hand-written list:

1. **Probe.** `codeatlas probe-tests` copies the repository, and for each
   probed function inserts `panic!("codeatlas probe");` at the start of
   its body, builds the tests and runs every test binary. Tests that fail
   with the probe and pass without it are exactly the tests that executed
   the function: the ground truth. Tests are mapped to symbol IDs through
   the crate root of their binary; tests CodeAtlas has no symbol for (for
   example generated by macros) are kept as `unmapped:` entries, so they
   count against recall instead of disappearing.
2. **Evaluate.** `codeatlas evaluate-tests` selects tests for each probed
   function and compares: *precision* is the share of selected tests that
   executed the function, *recall* the share of executing tests that were
   selected, both micro-averaged over probes; every miss and extra
   selection is listed.

Precision below 1 is expected of any static selection: a selected test may
not execute the function in this run (a branch not taken, another
implementation behind the same trait) and would still be affected by some
changes. Misses are the important number: a test that executed changed
code but was not selected.

What the probes cannot see: a test with `#[should_panic]` or one that
catches panics passes despite the probe, a panic swallowed on another
thread may not fail its test, and doc tests are not run. These make the
ground truth miss some executions, not invent them.

The fixtures `test-impact` (built for this, one pattern per function),
`change-impact` and `simple-repo` carry their recorded ground truth as
`expected-tests.json`. A test re-records it in CI and requires the same
result; another checks the evaluation numbers.

## Git diff impact

`analyzer::diff` answers *what does this branch (or commit, or uncommitted
change) affect?*

1. **Revisions.** `base` and `head` are resolved to commits with
   `git rev-parse --verify --end-of-options <rev>^{commit}`. Each commit
   is exported with `git archive` into a temporary directory (the
   repository and its work tree are not touched) and analysed like any
   repository, under the repository's own name so that symbol IDs match.
   Without a `head`, the working tree is analysed as it is on disk,
   untracked files included.
2. **Files and hunks.** `git diff --name-status --find-renames` lists added,
   removed, modified and renamed files; `git diff --unified=0` gives the
   changed line ranges of `.rs` files. Both are limited to the analysed
   directory with `--relative`, so a project inside a larger repository is
   handled. Options such as `--no-ext-diff` and explicit prefixes make the
   output independent of the user's Git configuration.
3. **Symbols.** Symbols are matched by ID. Each function, method, struct,
   enum and trait carries a *fingerprint*: a hash of its tokens with
   whitespace and comments left out (a trait's methods are left out of the
   trait's fingerprint; they have their own).
   * *Modified*: present on both sides with different fingerprints. The
     evidence is the hunk lines that fall inside the symbol. If the
     signature text differs as well, the change is reported as a
     *signature change*, with both versions.
   * *Cosmetic*: the diff touches the symbol's own lines (not just a nested
     symbol's) but the fingerprint is unchanged: only formatting or
     comments changed. These are listed but not treated as changes.
   * *Added* / *removed*: only on one side.
   * *Moved*: a removed and an added symbol of the same kind and name with
     the same fingerprint, when that pairing is unique, e.g. after a file
     rename. Ambiguous pairings stay added and removed.

   Modules are only added or removed; edits to `use` lines and `mod`
   declarations show up through the symbols they affect.
4. **Impact.** Modified symbols are traced on the head graph and removed
   symbols on the base graph, with the same engine and rules as
   [above](#which-edges-propagate-impact) (types and traits expand to their
   methods). Dependents of removed code matter if they still exist: they
   either changed (and are reported as changes) or now resolve to
   something else. The two results are merged per symbol: certain before
   possible, head before base. Symbols the diff itself changed are left
   out, so *downstream* means code that did not change but depends on code
   that did. Each downstream symbol keeps its evidence chain and says
   which revision its line numbers refer to.

The `pr-impact` fixture has a base and a head snapshot covering each case
(a signature change, body changes, a removed file, an added file, a
rename, a comment-only edit), and `tests/diff.rs` compares the result with
the hand-derived `expected.json`. The same diff must come out for
committed branches, for uncommitted changes, and for a project in a
subdirectory of its repository.

Limits of the comparison:

* Attributes are outside an item's syntax node, so adding `#[derive]` or
  `#[inline]` alone does not mark the item as modified.
* `#[cfg]` duplicates get `#N` suffixes in source order; removing one can
  renumber the others and make them look modified.
* Both revisions are analysed in full on every request. Incremental
  analysis is Milestone 8.

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
