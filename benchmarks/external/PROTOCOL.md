# External test-selection evaluation: protocol

Frozen before any probe was run on the repositories below. Changes after
results are seen must be listed under "Deviations" with the reason.

## Question

Can CodeAtlas's static test selection safely omit tests on external Rust
projects? Earlier ripgrep numbers (whole-suite recall 0.047 resolved, 0.228
with ambiguous calls) suggest not. This evaluation measures it on several
repositories against simple baselines.

## Inputs

Pinned in `repos.json`: repository commits, the Docker image (by digest)
and toolchain, probe seeds and sample sizes. Libraries without a committed
`Cargo.lock` get one generated once and committed under `locks/` before
probing; builds use `--locked` and run offline.

## Ground truth (mutation probes)

For each sampled non-test function or method in a library or binary crate,
`panic!` is inserted at the start of its body, `cargo test --workspace
--no-run` builds the default-feature test binaries, and every binary runs.
A test is *failing* for a probe if it fails with the probe and passed in
both baseline runs.

- Baseline runs twice. Tests with different outcomes are *flaky* and
  excluded from all failing sets.
- Per-test times come from libtest's JSON output in the baseline runs
  (enabled with `RUSTC_BOOTSTRAP=1` at run time only; the code under test
  is compiled normally). Binaries with a custom harness are recorded as
  one opaque test using their exit status.
- Probe status: `ok`, `build_failed`, or `incomplete` (crash or timeout).
  Only `ok` probes enter the metrics; the others are counted and reported.
- Not covered by the oracle: doctests; `#[should_panic]` or
  `catch_unwind` tests that tolerate the probe; non-default features.

Isolation: every build and test run happens in a container from the
pinned image, with `--network none`, only a scratch copy of the repository
mounted, and dependencies fetched beforehand in a separate step.

## Strategies compared

1. `run-all`: every test.
2. `changed-package`: tests of the Cargo package that contains the probed
   function, plus workspace packages depending on it (transitively).
3. `file-dependency`: tests in the probed function's file and in files
   that depend on it transitively through CodeAtlas's file-level graph.
4. `name-mention`: tests in any file in which the function's name occurs
   as an identifier token.
5. `codeatlas`: selection over resolved relationships.
6. `codeatlas-ambiguous`: also through ambiguous calls.
7. `codeatlas-conservative`: `codeatlas`, but the full suite whenever the
   rule below reports selection as unsafe.

## Conservative rule (fixed here, heuristic, not a soundness proof)

Selective execution is reported unsafe when any of these holds:

- S1: test code (test, bench or example crates, `#[cfg(test)]` modules)
  contains item-level macro invocations, which can generate tests
  CodeAtlas cannot see.
- S2: a changed callable is a candidate of an ambiguous call.
- S3: an unresolved or dynamic call site names a changed callable.
- S4: a changed callable's name appears in code CodeAtlas does not analyse
  as calls: macro definitions, unparsed macro bodies, or a bare path in
  value position (e.g. passed as an argument).
- S5: a changed method implements a trait defined outside the repository
  (callers may be in the standard library or dependencies).
- S6: the traversal was truncated.

## Metrics (per repository, over `ok` probes with at least one failing test unless stated)

- Whole-suite failing-test recall, micro-averaged (primary; denominator
  is every failing test, including tests CodeAtlas cannot see) and
  per-probe mean.
- Safe-probe rate: share of probes with recall 1.
- Precision, selected-test count and share of the suite.
- Test time saved: 1 − (summed per-test time of the selection ÷ summed
  time of all tests); build time is reported separately and not counted
  as saved.
- Misses split into *invisible* tests (no symbol in the analysis, e.g.
  macro-generated) and *visible-but-missed* tests. Recall over visible
  tests only is reported as secondary, labelled with its denominator.
- Unresolved coverage: repository resolution rate and the ambiguous and
  unresolved call counts; how often each rule S1–S6 fired.
- Probe failures: `build_failed`, `incomplete`, and probes whose function
  no test executed.

## Analysis plan

Results on the primary seed are reported in full. Missed tests are
classified by cause. Any change to CodeAtlas motivated by these results is
evaluated only on the held-out seed, and reported as such.

## Deviations

All made while implementing the harness, before any probe ran on the
repositories above.

1. S2–S5 are checked for the changed callables and for every symbol that
   depends on them (the certain impact set), not only the changed
   callables. Found while writing the fixture test: in `test-impact`,
   `parse::number` is reached only through `parse::pair`, which is passed
   as a value; checking the changed name alone does not flag it.
2. `name-mention` selects at test-binary granularity: all tests of every
   crate in whose files the name occurs as an identifier token. Tests
   generated by macros have no file of their own, so file granularity
   could never select them.
3. S4's "value position" is: a bare path as a call argument, an array or
   tuple element, a `let` initializer, a field initializer value or
   shorthand field, the right side of an assignment, or a return value;
   names bound locally (parameters, `let`, closure parameters, as recorded
   by the analyzer) are not counted.
4. Ground truths without package data (the committed fixture files) are
   compared without `changed-package`.
5. The first `bytes` run recorded no probes: CodeAtlas dropped the
   library target because a bench target (`benches/bytes.rs`) has the
   same name, so no library function was eligible. Fixed in the layout
   (such targets get a kind suffix, as `src/main.rs` next to a library
   already did) and `bytes` probed again with the fixed build. No probe
   of `bytes` had produced a result before. The fix changes no target in
   the other three repositories, and their comparisons were recomputed
   with the fixed build (identical results).
