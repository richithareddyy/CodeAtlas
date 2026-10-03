//! Resolution quality on the fixture repositories, measured against the
//! hand-written ground truth in `fixtures/<name>/expected.json`.

use std::path::PathBuf;

use codeatlas_analyzer::evaluation::{evaluate, Evaluation, GroundTruth, SetComparison};
use codeatlas_analyzer::ingest::IngestOptions;
use codeatlas_analyzer::model::{EdgeKind, ResolutionMethod};
use codeatlas_analyzer::{analyze_source, RepoSource, RepositoryAnalysis};

fn fixture_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn analyze(name: &str) -> RepositoryAnalysis {
    analyze_source(
        &RepoSource::Local(fixture_dir(name)),
        &IngestOptions::default(),
    )
    .unwrap()
}

fn evaluate_fixture(name: &str) -> (RepositoryAnalysis, Evaluation) {
    let analysis = analyze(name);
    let truth: GroundTruth = serde_json::from_str(
        &std::fs::read_to_string(fixture_dir(name).join("expected.json")).unwrap(),
    )
    .unwrap();
    let evaluation = evaluate(&analysis.resolution, &truth);
    (analysis, evaluation)
}

fn describe(label: &str, c: &SetComparison) -> String {
    let mut out = format!("{label}: {} expected, {} actual\n", c.expected, c.actual);
    for m in &c.missing {
        out.push_str(&format!("  missing    {m}\n"));
    }
    for u in &c.unexpected {
        out.push_str(&format!("  unexpected {u}\n"));
    }
    out
}

fn assert_exact(name: &str) -> RepositoryAnalysis {
    let (analysis, e) = evaluate_fixture(name);
    assert!(
        e.is_exact(),
        "{name} differs from ground truth:\n{}{}{}{}",
        describe("calls", &e.calls),
        describe("implements", &e.implements),
        describe("ambiguous", &e.ambiguous),
        describe("unresolved", &e.unresolved),
    );
    analysis
}

fn via(analysis: &RepositoryAnalysis, from: &str, to: &str) -> ResolutionMethod {
    analysis
        .resolution
        .edges
        .iter()
        .find(|e| e.kind == EdgeKind::Calls && e.from.as_str() == from && e.to.as_str() == to)
        .unwrap_or_else(|| panic!("no edge {from} -> {to}"))
        .via
}

#[test]
fn simple_repo_matches_ground_truth() {
    let a = assert_exact("simple-repo");
    let stats = &a.resolution.stats.calls;
    assert_eq!(stats.resolution_rate, Some(1.0));
    assert_eq!(stats.total, 14);
    // Ok x2, Err x3, is_ok
    assert_eq!(stats.external, 6);
    assert_eq!(
        via(
            &a,
            "fn:simple_repo::payments::process_payment",
            "method:simple_repo::payments::PaymentService::authorize"
        ),
        ResolutionMethod::ReceiverType
    );
    assert_eq!(
        via(
            &a,
            "fn:checkout_test::checkout_succeeds_for_small_orders",
            "fn:simple_repo::checkout::checkout"
        ),
        ResolutionMethod::Import
    );
}

#[test]
fn duplicate_symbols_match_ground_truth() {
    let a = assert_exact("duplicate-symbols");
    let ambiguous = &a.resolution.ambiguous_calls[0];
    assert_eq!(
        ambiguous
            .candidates
            .iter()
            .map(|c| c.as_str())
            .collect::<Vec<_>>(),
        vec![
            "fn:duplicate_symbols::payments::platform_fee",
            "fn:duplicate_symbols::payments::platform_fee#2"
        ]
    );
    // Inherent methods take priority over the trait method of the same name.
    assert_eq!(
        via(
            &a,
            "method:duplicate_symbols::admin::<AdminService as Authorizer>::authorize",
            "method:duplicate_symbols::admin::AdminService::authorize"
        ),
        ResolutionMethod::Scope
    );
}

#[test]
fn cross_module_matches_ground_truth() {
    let a = assert_exact("cross-module");
    // The impl block in services/billing.rs is re-homed under model::Invoice.
    let discounted = a
        .files
        .iter()
        .flat_map(|f| &f.symbols)
        .find(|s| s.id.as_str() == "method:cross_module::model::Invoice::discounted")
        .expect("re-homed method");
    assert_eq!(discounted.file, "src/services/billing.rs");
    assert_eq!(
        discounted.parent.as_ref().unwrap().as_str(),
        "struct:cross_module::model::Invoice"
    );
    assert_eq!(
        via(
            &a,
            "method:cross_module::services::billing::Billing::issue",
            "method:cross_module::storage::memory::<MemoryStore as Store>::save"
        ),
        ResolutionMethod::SelfType
    );
    assert_eq!(
        via(
            &a,
            "fn:cross_module::services::reporting::summarize",
            "method:cross_module::storage::Store::exists"
        ),
        ResolutionMethod::ReceiverType
    );
    assert_eq!(a.resolution.stats.imports.unresolved, 0);
}

#[test]
fn unresolved_fixture_matches_ground_truth() {
    let a = assert_exact("unresolved");
    let stats = &a.resolution.stats.calls;
    assert_eq!(stats.resolved, 0);
    // A macro-generated function is invisible to the parser, so its call is
    // classified as external. This documents the limitation.
    assert!(stats.external >= 1);
}

#[test]
fn resolution_is_deterministic() {
    let first = analyze("cross-module");
    let second = analyze("cross-module");
    assert_eq!(first.resolution.edges, second.resolution.edges);
    assert_eq!(
        first.resolution.ambiguous_calls,
        second.resolution.ambiguous_calls
    );
}
