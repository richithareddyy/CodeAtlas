//! End-to-end analysis of the fixture repositories. Expected values were
//! written by reading the fixture sources, not by recording analyzer output.

use std::path::PathBuf;

use codeatlas_analyzer::ingest::IngestOptions;
use codeatlas_analyzer::model::{Symbol, SymbolKind, TargetKind, Visibility};
use codeatlas_analyzer::{analyze_source, RepoSource, RepositoryAnalysis};

fn fixture(name: &str) -> RepositoryAnalysis {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name);
    analyze_source(&RepoSource::Local(path), &IngestOptions::default()).unwrap()
}

fn symbols(a: &RepositoryAnalysis) -> impl Iterator<Item = &Symbol> {
    a.files.iter().flat_map(|f| &f.symbols)
}

fn find<'a>(a: &'a RepositoryAnalysis, id: &str) -> &'a Symbol {
    symbols(a)
        .find(|s| s.id.as_str() == id)
        .unwrap_or_else(|| panic!("missing symbol {id}"))
}

#[test]
fn simple_repo_layout_and_metadata() {
    let a = fixture("simple-repo");
    assert_eq!(a.repository.name, "simple-repo");
    assert_eq!(a.stats.files_analyzed, 6);
    assert_eq!(a.stats.files_with_syntax_errors, 0);

    let files: Vec<_> = a.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        files,
        vec![
            "src/checkout.rs",
            "src/inventory.rs",
            "src/lib.rs",
            "src/payments/gateway.rs",
            "src/payments/mod.rs",
            "tests/checkout_test.rs",
        ]
    );

    let targets: Vec<_> = a.crates.iter().map(|c| (c.name.as_str(), c.kind)).collect();
    assert!(targets.contains(&("simple_repo", TargetKind::Lib)));
    assert!(targets.contains(&("checkout_test", TargetKind::Test)));
}

#[test]
fn simple_repo_symbols() {
    let a = fixture("simple-repo");

    let gateway = find(&a, "mod:simple_repo::payments::gateway");
    assert_eq!(gateway.visibility, Visibility::Private);
    assert_eq!(
        gateway.parent.as_ref().unwrap().as_str(),
        "mod:simple_repo::payments"
    );
    assert_eq!(
        find(&a, "mod:simple_repo::payments").visibility,
        Visibility::Public
    );

    let authorize = find(
        &a,
        "method:simple_repo::payments::PaymentService::authorize",
    );
    assert_eq!(authorize.file, "src/payments/mod.rs");
    assert_eq!(
        (authorize.span.start_line, authorize.span.end_line),
        (17, 22)
    );
    assert_eq!(
        authorize.parent.as_ref().unwrap().as_str(),
        "struct:simple_repo::payments::PaymentService"
    );
    assert_eq!(
        authorize.signature.as_deref(),
        Some("pub fn authorize(&self, amount_cents: u64) -> Result<(), PaymentError>")
    );

    let stripe = find(&a, "fn:simple_repo::payments::gateway::stripe_call");
    assert_eq!(stripe.visibility, Visibility::Crate);

    let tests: Vec<_> = symbols(&a)
        .filter(|s| s.is_test)
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(
        tests,
        vec![
            "fn:simple_repo::payments::tests::rejects_amounts_over_limit",
            "fn:checkout_test::checkout_succeeds_for_small_orders",
        ]
    );
    assert!(find(&a, "mod:simple_repo::payments::tests").cfg_test);
    assert_eq!(a.stats.symbols[&SymbolKind::Struct], 2);
    assert_eq!(a.stats.symbols[&SymbolKind::Enum], 1);
}

#[test]
fn simple_repo_call_sites_and_imports() {
    let a = fixture("simple-repo");
    let calls_from = |caller: &str| -> Vec<String> {
        a.files
            .iter()
            .flat_map(|f| &f.calls)
            .filter(|c| c.caller.as_str() == caller)
            .map(|c| c.callee.display())
            .collect()
    };

    assert_eq!(
        calls_from("fn:simple_repo::checkout::checkout"),
        vec!["inventory::reserve", "payments::process_payment", "Ok"]
    );
    assert_eq!(
        calls_from("fn:simple_repo::payments::process_payment"),
        vec!["PaymentService::new", "service.authorize"]
    );
    assert_eq!(
        calls_from("method:simple_repo::payments::PaymentService::authorize"),
        vec!["Err", "gateway::stripe_call"]
    );

    // The integration test calls `checkout` only inside `assert!`.
    let test_call = a
        .files
        .iter()
        .flat_map(|f| &f.calls)
        .find(|c| c.caller.as_str() == "fn:checkout_test::checkout_succeeds_for_small_orders")
        .unwrap();
    assert!(test_call.in_macro);
    assert_eq!(test_call.line, 9);

    let checkout_imports: Vec<_> = a
        .files
        .iter()
        .find(|f| f.path == "src/checkout.rs")
        .unwrap()
        .imports
        .iter()
        .map(|i| i.path.join("::"))
        .collect();
    assert_eq!(
        checkout_imports,
        vec![
            "crate::inventory",
            "crate::payments",
            "crate::payments::PaymentError"
        ]
    );
}

#[test]
fn duplicate_symbols_receive_distinct_ids() {
    let a = fixture("duplicate-symbols");
    let mut authorize: Vec<_> = symbols(&a)
        .filter(|s| s.name == "authorize")
        .map(|s| s.id.as_str())
        .collect();
    authorize.sort();
    assert_eq!(
        authorize,
        vec![
            "fn:duplicate_symbols::auth::authorize",
            "method:duplicate_symbols::Authorizer::authorize",
            "method:duplicate_symbols::admin::<AdminService as Authorizer>::authorize",
            "method:duplicate_symbols::admin::AdminService::authorize",
            "method:duplicate_symbols::auth::OAuthService::authorize",
            "method:duplicate_symbols::payments::PaymentService::authorize",
        ]
    );

    let fee_lines: Vec<_> = ["", "#2"]
        .iter()
        .map(|suffix| {
            find(
                &a,
                &format!("fn:duplicate_symbols::payments::platform_fee{suffix}"),
            )
            .span
            .start_line
        })
        .collect();
    assert_eq!(fee_lines, vec![10, 15]);

    // Both impl blocks for AdminService are attached to the struct.
    for id in [
        "method:duplicate_symbols::admin::AdminService::authorize",
        "method:duplicate_symbols::admin::<AdminService as Authorizer>::authorize",
    ] {
        assert_eq!(
            find(&a, id).parent.as_ref().unwrap().as_str(),
            "struct:duplicate_symbols::admin::AdminService"
        );
    }
}

#[test]
fn symbol_ids_are_unique_and_stable_across_runs() {
    let first = fixture("duplicate-symbols");
    let second = fixture("duplicate-symbols");
    let ids = |a: &RepositoryAnalysis| symbols(a).map(|s| s.id.clone()).collect::<Vec<_>>();

    let mut unique = ids(&first);
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), ids(&first).len());
    assert_eq!(ids(&first), ids(&second));
}
