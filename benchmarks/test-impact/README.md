# Test-impact measurements

How well CodeAtlas's static test selection finds the tests that actually
execute a function, measured on ripgrep. The method is described in
[docs/impact-analysis.md](../../docs/impact-analysis.md#measuring-selection-against-reality).

| File | Contents |
|---|---|
| `ripgrep-3fce3b5-truth.json` | Ground truth: 30 probed functions (seed 1) and the tests that failed when each panicked; 1,195 tests |
| `ripgrep-3fce3b5-results.json` | Evaluation, resolved calls only |
| `ripgrep-3fce3b5-results-ambiguous.json` | Evaluation, ambiguous calls included |

Recorded on ripgrep commit `3fce3b5` with rustc 1.99.0 on an Apple Silicon
Mac (one run; about 2.5 minutes for the 30 probes).

## Reproducing

This builds ripgrep and runs its test suite once per probe.

```bash
git clone https://github.com/BurntSushi/ripgrep && git -C ripgrep checkout 3fce3b5
```

```bash
codeatlas probe-tests ripgrep -n 30 --seed 1 -o truth.json
```

```bash
codeatlas evaluate-tests ripgrep --truth truth.json --include-ambiguous
```

`evaluate-tests` also accepts the committed truth file directly; it warns
if the analysed sources differ from the probed ones.
