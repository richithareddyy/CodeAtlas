//! Test selection, and its evaluation against the observed ground truth of
//! the `test-impact` fixture (`expected-tests.json`, recorded with
//! `codeatlas probe-tests`).

use std::path::{Path, PathBuf};

use codeatlas_analyzer::graph::impact::ImpactOptions;
use codeatlas_analyzer::graph::test_selection::select_tests;
use codeatlas_analyzer::graph::CodeGraph;
use codeatlas_analyzer::ingest::IngestOptions;
use codeatlas_analyzer::model::SymbolId;
use codeatlas_analyzer::test_evaluation::{evaluate_tests, source_hash, TestGroundTruth};
use codeatlas_analyzer::{analyze_source, RepoSource, RepositoryAnalysis};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/test-impact")
}

fn analysis() -> RepositoryAnalysis {
    analyze_source(&RepoSource::Local(fixture_dir()), &IngestOptions::default()).unwrap()
}

fn node(graph: &CodeGraph, id: &str) -> usize {
    graph
        .node(&SymbolId::from_stored(id.to_string()))
        .unwrap_or_else(|| panic!("no symbol {id}"))
}

fn names(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut names: Vec<String> = ids
        .into_iter()
        .map(|id| id.rsplit("::").next().unwrap().to_string())
        .collect();
    names.sort();
    names
}

#[test]
fn selection_separates_direct_and_transitive_tests_and_reports_gaps() {
    let graph = CodeGraph::from_analysis(&analysis());
    let changed = [
        node(&graph, "fn:ledger::pricing::subtotal"),
        node(&graph, "fn:ledger::util::unused_helper"),
        node(&graph, "fn:ledger::pricing::tests::tax_is_eight_percent"),
    ];
    let s = select_tests(&graph, &changed, ImpactOptions::default()).unwrap();
    assert_eq!(s.total_tests, 9);
    assert_eq!(
        names(s.direct.iter().map(|t| t.test.id.to_string())),
        ["subtotal_adds_items"]
    );
    // summary_includes_tax -> summary -> total -> subtotal
    assert_eq!(
        names(s.transitive.iter().map(|t| t.test.id.to_string())),
        ["summary_includes_tax"]
    );
    let summary = &s.transitive[0];
    assert_eq!((summary.depth, summary.calls), (3, 3));
    assert_eq!(
        summary.reaches,
        [SymbolId::from_stored("fn:ledger::pricing::subtotal".into())]
    );
    assert_eq!(
        names(s.untested.iter().map(|u| u.id.to_string())),
        ["unused_helper"]
    );
    assert_eq!(
        names(s.changed_tests.iter().map(|t| t.id.to_string())),
        ["tax_is_eight_percent"]
    );
    assert_eq!(s.to_run().len(), 3);
}

#[test]
fn dispatch_steps_do_not_make_a_test_transitive() {
    let graph = CodeGraph::from_analysis(&analysis());
    // The test calls `balance`, which calls `Store::get`, which may
    // dispatch to the changed implementation: two calls.
    let s = select_tests(
        &graph,
        &[node(
            &graph,
            "method:ledger::store::<MemoryStore as Store>::get",
        )],
        ImpactOptions::default(),
    )
    .unwrap();
    assert!(s.direct.is_empty());
    let balance = s
        .transitive
        .iter()
        .find(|t| t.test.id.as_str() == "fn:ledger_tests::balance_reads_the_memory_store")
        .unwrap();
    assert_eq!((balance.depth, balance.calls), (3, 2));
    // The generic call in `lookup_or_zero` is ambiguous: only possible.
    let with_ambiguous = select_tests(
        &graph,
        &[node(
            &graph,
            "method:ledger::store::<MemoryStore as Store>::get",
        )],
        ImpactOptions {
            include_ambiguous: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        names(
            with_ambiguous
                .possible
                .iter()
                .map(|t| t.test.id.to_string())
        ),
        ["generic_lookup_defaults_to_zero"]
    );
}

#[test]
fn evaluation_against_observed_failures_matches_the_recorded_result() {
    let analysis = analysis();
    let truth: TestGroundTruth = serde_json::from_str(
        &std::fs::read_to_string(fixture_dir().join("expected-tests.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        truth.source_hash,
        source_hash(&analysis),
        "the fixture changed; record the ground truth again with `codeatlas probe-tests`"
    );
    let graph = CodeGraph::from_analysis(&analysis);
    let e = evaluate_tests(&graph, &truth, ImpactOptions::default());
    let s = &e.summary;
    assert_eq!(
        (s.probes, s.skipped, s.tests, s.unmapped_tests),
        (20, 0, 9, 0)
    );
    assert_eq!((s.actual, s.predicted, s.true_positives), (23, 22, 19));
    assert_eq!((s.precision, s.recall), (Some(0.864), Some(0.826)));
    assert_eq!(
        (s.exact, s.not_executed, s.not_executed_and_not_selected),
        (14, 2, 1)
    );

    // Each miss is a known limitation of static selection.
    let missed: Vec<(String, Vec<String>)> = e
        .probes
        .iter()
        .filter(|p| !p.missed.is_empty())
        .map(|p| (p.symbol.clone(), names(p.missed.clone())))
        .collect();
    assert_eq!(
        missed,
        [
            // Only called by `pair`, which is passed as a value.
            (
                "fn:ledger::parse::number".into(),
                vec!["file_store_parses_lines".into()]
            ),
            // Passed to `filter_map` as a value, never called by name.
            (
                "fn:ledger::parse::pair".into(),
                vec!["file_store_parses_lines".into()]
            ),
            // Called inside a `macro_rules!` body.
            (
                "fn:ledger::util::validate".into(),
                vec!["positive_values_are_valid".into()]
            ),
            // Called through a generic parameter (ambiguous).
            (
                "method:ledger::store::<MemoryStore as Store>::get".into(),
                vec!["generic_lookup_defaults_to_zero".into()]
            ),
        ]
    );
    // Extra selections: a branch the test does not take, and the other
    // implementation behind `dyn Store`.
    let extra: Vec<String> = e
        .probes
        .iter()
        .filter(|p| !p.extra.is_empty())
        .map(|p| p.symbol.clone())
        .collect();
    assert_eq!(
        extra,
        [
            "fn:ledger::pricing::bulk_discount",
            "method:ledger::store::<FileStore as Store>::get",
            "method:ledger::store::<MemoryStore as Store>::get",
        ]
    );
}

/// The other fixtures with tests, as recorded: selection finds every test
/// that executed a probed function; the extra selections are a branch not
/// taken (`stripe_call`) and implementations the tests never use behind
/// `dyn Gateway` (Stripe's).
#[test]
fn recorded_results_on_the_other_fixtures() {
    for (name, expected) in [
        ("simple-repo", (6, 8, 9, 8, Some(1.0), Some(0.889))),
        ("change-impact", (11, 13, 16, 13, Some(1.0), Some(0.813))),
    ] {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name);
        let analysis =
            analyze_source(&RepoSource::Local(dir.clone()), &IngestOptions::default()).unwrap();
        let truth: TestGroundTruth = serde_json::from_str(
            &std::fs::read_to_string(dir.join("expected-tests.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(truth.source_hash, source_hash(&analysis), "{name} changed");
        let graph = CodeGraph::from_analysis(&analysis);
        let s = evaluate_tests(&graph, &truth, ImpactOptions::default()).summary;
        assert_eq!(
            (
                s.probes,
                s.actual,
                s.predicted,
                s.true_positives,
                s.recall,
                s.precision
            ),
            expected,
            "{name}"
        );
    }
}
