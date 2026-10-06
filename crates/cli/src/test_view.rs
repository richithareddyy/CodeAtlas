//! Text rendering of test selections and their evaluation.

use std::fmt::Write as _;

use codeatlas_analyzer::graph::test_selection::{SelectedTest, TestSelection};
use codeatlas_analyzer::test_evaluation::TestImpactEvaluation;

use crate::views::{short, step_text};

fn ratio(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_string(), |v| format!("{v:.3}"))
}

pub fn evaluation(e: &TestImpactEvaluation) -> String {
    let s = &e.summary;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Test selection compared with observed test failures ({} probes)",
        e.method
    );
    let _ = writeln!(
        out,
        "Repository  {} @ {}",
        e.repository,
        e.commit
            .as_deref()
            .map_or("unknown commit", |c| &c[..c.len().min(10)])
    );
    let _ = writeln!(
        out,
        "Selection   depth <= {}, ambiguous calls {}",
        e.max_depth,
        if e.include_ambiguous {
            "included"
        } else {
            "excluded"
        }
    );
    let _ = writeln!(
        out,
        "Probes      {} evaluated, {} skipped (did not build or did not finish)",
        s.probes, s.skipped
    );
    let _ = writeln!(
        out,
        "Tests       {} in the suite, {} without a symbol (not selectable)",
        s.tests, s.unmapped_tests
    );
    let _ = writeln!(
        out,
        "Precision   {}  ({} of {} selections executed the probed function)",
        ratio(s.precision),
        s.true_positives,
        s.predicted
    );
    let _ = writeln!(
        out,
        "Recall      {}  ({} of {} executing tests were selected); {} counting only tests with a symbol",
        ratio(s.recall),
        s.true_positives,
        s.actual,
        ratio(s.recall_known_tests)
    );
    let _ = writeln!(
        out,
        "Exact       {} of {} probes; {} probed functions ran in no test ({} of them with nothing selected)",
        s.exact, s.probes, s.not_executed, s.not_executed_and_not_selected
    );
    let _ = writeln!(
        out,
        "Share       {:.1}% of the suite selected per probe on average; {:.1}% actually affected",
        s.mean_selected_share * 100.0,
        s.mean_actual_share * 100.0
    );

    let missed: Vec<_> = e.probes.iter().filter(|p| !p.missed.is_empty()).collect();
    if !missed.is_empty() {
        let _ = writeln!(out, "\nMissed (executed the function, not selected)");
        for p in missed {
            let _ = writeln!(out, "  {}", short(&p.symbol));
            for t in &p.missed {
                let _ = writeln!(out, "      {}", short(t));
            }
        }
    }
    let extra: Vec<_> = e.probes.iter().filter(|p| !p.extra.is_empty()).collect();
    if !extra.is_empty() {
        let _ = writeln!(out, "\nSelected but not executed");
        for p in extra {
            let _ = writeln!(out, "  {}", short(&p.symbol));
            for t in &p.extra {
                let _ = writeln!(out, "      {}", short(t));
            }
        }
    }
    let _ = writeln!(out, "\nPer probe (actual, selected, precision, recall)");
    for p in &e.probes {
        let _ = writeln!(
            out,
            "  {:>3} {:>3}  {:>5}  {:>5}  {}",
            p.actual.len(),
            p.predicted.len(),
            ratio(p.precision),
            ratio(p.recall),
            short(&p.symbol)
        );
    }
    out.trim_end().to_string()
}

fn selected_lines(out: &mut String, tests: &[SelectedTest]) {
    for t in tests {
        let _ = writeln!(
            out,
            "  {:<64} {}:{}",
            t.test.qualified_name, t.test.file, t.test.line
        );
        for step in &t.path {
            let _ = writeln!(out, "        {}", step_text(step));
        }
    }
}

pub fn selection(s: &TestSelection) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Tests for   {}",
        s.changed
            .iter()
            .map(|c| short(c.id.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let _ = writeln!(
        out,
        "Selected    {} of {} tests: {} direct, {} transitive{}",
        s.to_run().len(),
        s.total_tests,
        s.direct.len(),
        s.transitive.len(),
        if s.possible.is_empty() {
            String::new()
        } else {
            format!(
                "; {} more possible through ambiguous calls",
                s.possible.len()
            )
        }
    );
    if !s.changed_tests.is_empty() {
        let _ = writeln!(out, "\nChanged tests");
        for t in &s.changed_tests {
            let _ = writeln!(out, "  {:<64} {}:{}", t.qualified_name, t.file, t.line);
        }
    }
    let _ = writeln!(out, "\nDirect (the test calls the changed code)");
    if s.direct.is_empty() {
        let _ = writeln!(out, "  none");
    }
    selected_lines(&mut out, &s.direct);
    let _ = writeln!(out, "\nTransitive (through other code)");
    if s.transitive.is_empty() {
        let _ = writeln!(out, "  none");
    }
    selected_lines(&mut out, &s.transitive);
    if !s.possible.is_empty() {
        let _ = writeln!(out, "\nPossible (through ambiguous calls)");
        selected_lines(&mut out, &s.possible);
    }
    if !s.untested.is_empty() {
        let _ = writeln!(out, "\nNo test reaches");
        for u in &s.untested {
            let _ = writeln!(out, "  {:<64} {}:{}", u.qualified_name, u.file, u.line);
        }
    }
    out.trim_end().to_string()
}
