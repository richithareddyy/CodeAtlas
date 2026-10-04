//! Impact analysis and architecture analyses on the fixtures. Expected
//! results were derived by hand from the fixture sources.

use std::path::PathBuf;

use codeatlas_analyzer::graph::architecture::{cycles, hotspots, layers, Level};
use codeatlas_analyzer::graph::impact::{
    impact_of_file, impact_of_symbol, Confidence, DependencyKind, ImpactError, ImpactOptions,
    ImpactReport,
};
use codeatlas_analyzer::graph::CodeGraph;
use codeatlas_analyzer::ingest::IngestOptions;
use codeatlas_analyzer::model::{SymbolId, SymbolKind};
use codeatlas_analyzer::{analyze_source, RepoSource};

fn graph(name: &str) -> CodeGraph {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name);
    let analysis = analyze_source(&RepoSource::Local(path), &IngestOptions::default()).unwrap();
    CodeGraph::from_analysis(&analysis)
}

fn id(kind: SymbolKind, qualified_name: &str) -> SymbolId {
    SymbolId::new(kind, qualified_name)
}

fn method(q: &str) -> SymbolId {
    id(SymbolKind::Method, &format!("change_impact::{q}"))
}

fn function(q: &str) -> SymbolId {
    id(SymbolKind::Function, q)
}

fn impact(g: &CodeGraph, symbol: &SymbolId) -> ImpactReport {
    impact_of_symbol(g, symbol, ImpactOptions::default()).unwrap()
}

/// `(id, depth)` of certain results.
fn affected(report: &ImpactReport) -> Vec<(String, u32)> {
    report
        .certain()
        .map(|a| (a.symbol.id.to_string(), a.depth))
        .collect()
}

const AUTHORIZE: &str = "method:change_impact::payments::PaymentService::authorize";
const CHECKOUT: &str = "fn:change_impact::checkout::checkout";
const TEST_AUTHORIZE: &str = "fn:payment_tests::test_authorize_valid";
const TEST_CHECKOUT: &str = "fn:payment_tests::test_checkout";

#[test]
fn private_helper_affects_its_call_chain_and_tests_only() {
    let g = graph("change-impact");
    let report = impact(&g, &function("change_impact::payments::validate_amount"));
    assert_eq!(
        affected(&report),
        vec![
            (AUTHORIZE.into(), 1),
            (CHECKOUT.into(), 2),
            (TEST_AUTHORIZE.into(), 2),
            (TEST_CHECKOUT.into(), 3),
        ]
    );
    let tests: Vec<&str> = report.tests.iter().map(|t| t.as_str()).collect();
    assert_eq!(tests, vec![TEST_AUTHORIZE, TEST_CHECKOUT]);

    let files: Vec<(&str, u32)> = report
        .files
        .iter()
        .map(|f| (f.name.as_str(), f.symbols))
        .collect();
    assert_eq!(
        files,
        vec![
            ("tests/payment_tests.rs", 2),
            ("src/checkout.rs", 1),
            ("src/payments.rs", 1)
        ]
    );
    assert_eq!(report.modules.len(), 3);

    // Evidence: test_checkout -> checkout -> authorize -> validate_amount.
    let chain = &report
        .affected
        .iter()
        .find(|a| a.symbol.id.as_str() == TEST_CHECKOUT)
        .unwrap()
        .path;
    let steps: Vec<(&str, &str, DependencyKind, &str, Vec<u32>)> = chain
        .iter()
        .map(|s| {
            (
                s.source.as_str(),
                s.target.as_str(),
                s.kind,
                s.file.as_str(),
                s.lines.clone(),
            )
        })
        .collect();
    assert_eq!(
        steps,
        vec![
            (
                TEST_CHECKOUT,
                CHECKOUT,
                DependencyKind::Calls,
                "tests/payment_tests.rs",
                vec![16]
            ),
            (
                CHECKOUT,
                AUTHORIZE,
                DependencyKind::Calls,
                "src/checkout.rs",
                vec![5]
            ),
            (
                AUTHORIZE,
                "fn:change_impact::payments::validate_amount",
                DependencyKind::Calls,
                "src/payments.rs",
                vec![13]
            ),
        ]
    );
}

#[test]
fn changing_an_implementation_reaches_callers_through_the_trait_only() {
    let g = graph("change-impact");
    let report = impact(&g, &method("gateway::<StripeGateway as Gateway>::charge"));
    let gateway_charge = method("gateway::Gateway::charge").to_string();
    assert_eq!(
        affected(&report),
        vec![
            (gateway_charge.clone(), 1),
            (AUTHORIZE.into(), 2),
            (CHECKOUT.into(), 3),
            (TEST_AUTHORIZE.into(), 3),
            (TEST_CHECKOUT.into(), 4),
        ]
    );
    // The sibling implementation is not affected by this change.
    assert!(report
        .affected
        .iter()
        .all(|a| !a.symbol.id.as_str().contains("FakeGateway")));
    assert_eq!(
        report.affected[0].path[0].kind,
        DependencyKind::DispatchesTo
    );
    assert_eq!(report.affected[0].path[0].file, "src/gateway.rs");
}

#[test]
fn changing_a_trait_method_affects_every_implementation() {
    let g = graph("change-impact");
    let report = impact(&g, &method("gateway::Gateway::charge"));
    assert_eq!(
        affected(&report),
        vec![
            (
                method("gateway::<FakeGateway as Gateway>::charge").to_string(),
                1
            ),
            (
                method("gateway::<StripeGateway as Gateway>::charge").to_string(),
                1
            ),
            (AUTHORIZE.into(), 1),
            (CHECKOUT.into(), 2),
            (TEST_AUTHORIZE.into(), 2),
            (TEST_CHECKOUT.into(), 3),
        ]
    );
    let implementation = report
        .affected
        .iter()
        .find(|a| a.symbol.id.as_str().contains("FakeGateway"))
        .unwrap();
    assert_eq!(implementation.path[0].kind, DependencyKind::Implements);
}

#[test]
fn types_modules_and_files_expand_to_their_members() {
    let g = graph("change-impact");
    let by_type = impact(
        &g,
        &id(
            SymbolKind::Struct,
            "change_impact::payments::PaymentService",
        ),
    );
    let changed: Vec<&str> = by_type.changed.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        changed,
        vec![
            AUTHORIZE,
            "method:change_impact::payments::PaymentService::new",
            "struct:change_impact::payments::PaymentService",
        ]
    );
    assert_eq!(
        affected(&by_type),
        vec![
            (CHECKOUT.into(), 1),
            (TEST_AUTHORIZE.into(), 1),
            (TEST_CHECKOUT.into(), 1)
        ]
    );

    let by_module = impact(&g, &id(SymbolKind::Module, "change_impact::payments"));
    assert_eq!(affected(&by_module), affected(&by_type));
    assert!(by_module
        .changed
        .iter()
        .any(|c| c.id.as_str() == "fn:change_impact::payments::validate_amount"));

    let by_file = impact_of_file(&g, "src/payments.rs", ImpactOptions::default()).unwrap();
    assert_eq!(affected(&by_file), affected(&by_type));

    let unrelated = impact(&g, &function("change_impact::reports::daily_total"));
    assert_eq!(
        affected(&unrelated),
        vec![("fn:payment_tests::test_daily_total".into(), 1)]
    );
}

#[test]
fn score_is_decomposable_into_its_inputs() {
    let g = graph("change-impact");
    let report = impact(&g, &function("change_impact::payments::validate_amount"));
    let values: Vec<(&str, u32)> = report
        .score
        .factors
        .iter()
        .map(|f| (f.name.as_str(), f.value))
        .collect();
    assert_eq!(
        values,
        vec![
            ("direct_dependents", 1),
            ("transitive_dependents", 4),
            ("affected_modules", 3),
            ("affected_tests", 2),
            ("fan_in", 1),
            ("dependency_depth", 3),
        ]
    );
    let sum: f64 = report.score.factors.iter().map(|f| f.contribution).sum();
    assert!((sum - report.score.total).abs() < 0.11);
}

#[test]
fn ambiguous_calls_are_reported_as_possible_only_when_requested() {
    let g = graph("unresolved");
    let email_run = id(SymbolKind::Method, "unresolved::jobs::<Email as Job>::run");

    let strict = impact_of_symbol(&g, &email_run, ImpactOptions::default()).unwrap();
    assert_eq!(
        affected(&strict),
        vec![("method:unresolved::jobs::Job::run".into(), 1)]
    );
    assert!(strict
        .affected
        .iter()
        .all(|a| a.confidence == Confidence::Certain));

    let lenient = impact_of_symbol(
        &g,
        &email_run,
        ImpactOptions {
            include_ambiguous: true,
            ..Default::default()
        },
    )
    .unwrap();
    let possible: Vec<(&str, DependencyKind)> = lenient
        .affected
        .iter()
        .filter(|a| a.confidence == Confidence::Possible)
        .map(|a| (a.symbol.id.as_str(), a.path[0].kind))
        .collect();
    assert_eq!(
        possible,
        vec![
            ("fn:unresolved::jobs::run_all", DependencyKind::MayCall),
            ("fn:unresolved::jobs::run_generic", DependencyKind::MayCall),
        ]
    );
    // Possible results never inflate the score.
    assert_eq!(lenient.score, strict.score);
}

#[test]
fn impact_is_bounded_and_rejects_unknown_symbols() {
    let g = graph("change-impact");
    let validate = function("change_impact::payments::validate_amount");
    let shallow = impact_of_symbol(
        &g,
        &validate,
        ImpactOptions {
            max_depth: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(affected(&shallow), vec![(AUTHORIZE.into(), 1)]);

    let capped = impact_of_symbol(
        &g,
        &validate,
        ImpactOptions {
            max_nodes: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(capped.affected.len(), 1);
    assert!(capped.truncated);

    assert!(matches!(
        impact_of_symbol(&g, &function("nope"), ImpactOptions::default()),
        Err(ImpactError::UnknownSymbol(_))
    ));
}

#[test]
fn detects_module_file_and_function_cycles_with_evidence() {
    let g = graph("circular-dependency");
    let module_cycles = cycles(&g, Level::Module);
    assert_eq!(module_cycles.len(), 1);
    let cycle = &module_cycles[0];
    assert_eq!(
        cycle.members,
        vec![
            "mod:circular_dependency::checkout",
            "mod:circular_dependency::orders",
            "mod:circular_dependency::payments",
        ]
    );
    let hops: Vec<(&str, &str)> = cycle
        .hops
        .iter()
        .map(|h| (h.from.as_str(), h.to.as_str()))
        .collect();
    assert_eq!(
        hops,
        vec![
            (
                "mod:circular_dependency::checkout",
                "mod:circular_dependency::payments"
            ),
            (
                "mod:circular_dependency::payments",
                "mod:circular_dependency::orders"
            ),
            (
                "mod:circular_dependency::orders",
                "mod:circular_dependency::checkout"
            ),
        ]
    );
    // checkout -> payments: `use crate::payments` (line 1) and the call on line 5.
    let first = &cycle.hops[0];
    assert_eq!(first.weight, 2);
    assert_eq!(first.via, vec!["CALLS", "IMPORTS"]);
    let evidence: Vec<(&str, &str, u32)> = first
        .evidence
        .iter()
        .map(|e| (e.relation.as_str(), e.file.as_str(), e.line))
        .collect();
    assert_eq!(
        evidence,
        vec![
            ("CALLS", "src/checkout.rs", 5),
            ("IMPORTS", "src/checkout.rs", 1)
        ]
    );

    let file_cycles = cycles(&g, Level::File);
    assert_eq!(
        file_cycles[0].members,
        vec!["src/checkout.rs", "src/orders.rs", "src/payments.rs"]
    );

    let recursion: Vec<Vec<String>> = cycles(&g, Level::Function)
        .into_iter()
        .map(|c| c.members)
        .collect();
    assert_eq!(
        recursion,
        vec![
            vec![
                "fn:circular_dependency::math::is_even".to_string(),
                "fn:circular_dependency::math::is_odd".to_string(),
            ],
            vec!["fn:circular_dependency::math::factorial".to_string()],
        ]
    );
}

#[test]
fn layers_place_cycles_above_their_dependencies() {
    let g = graph("circular-dependency");
    let module_layers: Vec<(u32, Vec<String>)> = layers(&g, Level::Module)
        .into_iter()
        .map(|l| (l.layer, l.members))
        .collect();
    assert_eq!(
        module_layers,
        vec![
            (0, vec!["mod:circular_dependency::util".to_string()]),
            (
                1,
                vec![
                    "mod:circular_dependency::checkout".to_string(),
                    "mod:circular_dependency::orders".to_string(),
                    "mod:circular_dependency::payments".to_string(),
                ]
            ),
        ]
    );
}

#[test]
fn hotspots_rank_by_betweenness() {
    let g = graph("change-impact");
    let top: Vec<(String, f64, usize)> = hotspots(&g, Level::Function, 3)
        .into_iter()
        .map(|h| (h.name, h.betweenness, h.fan_in))
        .collect();
    assert_eq!(
        top,
        vec![
            (AUTHORIZE.to_string(), 6.0, 2),
            (CHECKOUT.to_string(), 4.0, 1),
            (
                "fn:change_impact::refunds::refund_order".to_string(),
                1.0,
                1
            ),
        ]
    );
}

#[test]
fn dependency_views_at_crate_module_and_file_level() {
    use codeatlas_analyzer::graph::architecture::{dependency_graph, ViewLevel};

    let g = graph("simple-repo");
    let crates = dependency_graph(&g, ViewLevel::Crate);
    let edges: Vec<(&str, &str, u32)> = crates
        .edges
        .iter()
        .map(|e| (e.from.as_str(), e.to.as_str(), e.weight))
        .collect();
    // The integration test imports and calls into the library: one import of
    // `checkout`, one of `Order`, and the call to `checkout`.
    assert_eq!(edges, vec![("checkout_test", "simple_repo", 3)]);
    assert!(crates.nodes.iter().all(|n| !n.in_cycle));

    let modules = dependency_graph(&g, ViewLevel::Module);
    let cyclic: Vec<&str> = modules
        .nodes
        .iter()
        .filter(|n| n.in_cycle)
        .map(|n| n.id.as_str())
        .collect();
    // payments calls gateway; gateway imports PaymentError from payments.
    assert_eq!(
        cyclic,
        vec![
            "mod:simple_repo::payments",
            "mod:simple_repo::payments::gateway"
        ]
    );

    let files = dependency_graph(&g, ViewLevel::File);
    let checkout = files
        .nodes
        .iter()
        .find(|n| n.id == "src/checkout.rs")
        .unwrap();
    assert_eq!((checkout.fan_in, checkout.fan_out), (2, 2));
}
