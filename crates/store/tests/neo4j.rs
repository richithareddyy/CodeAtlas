//! Integration tests against a real Neo4j instance.
//!
//! They run when `CODEATLAS_NEO4J_PASSWORD` is set (directly or in the
//! workspace `.env`) and are skipped otherwise. If Neo4j is configured but
//! unreachable, they fail. Each test indexes under its own repository ID and
//! deletes it afterwards, so tests can run in parallel against one database.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use codeatlas_analyzer::ingest::IngestOptions;
use codeatlas_analyzer::model::{EdgeKind, SymbolKind};
use codeatlas_analyzer::{analyze_source, RepoSource, RepositoryAnalysis};
use codeatlas_store::{Direction, GraphStore, QueryLimits, Relation, StoreConfig, StoreError};

static NEXT_ID: AtomicU32 = AtomicU32::new(0);

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

async fn store() -> Option<GraphStore> {
    let _ = dotenvy::from_path(workspace_root().join(".env"));
    let Ok(config) = StoreConfig::from_env() else {
        eprintln!("skipping: CODEATLAS_NEO4J_PASSWORD not set");
        return None;
    };
    let store = GraphStore::connect(&config)
        .await
        .expect("Neo4j is configured but not reachable; is `docker compose up -d` running?");
    store.ensure_schema().await.expect("schema");
    remove_stale_test_repositories(&store).await;
    Some(store)
}

/// A failed assertion skips a test's cleanup, so each run removes test
/// repositories left behind by earlier processes (never its own).
async fn remove_stale_test_repositories(store: &GraphStore) {
    let own = format!("-{}-", std::process::id());
    for repo in store.repositories().await.expect("list repositories") {
        if repo.id.starts_with("test-") && !repo.id.contains(&own) {
            store
                .delete_repository(&repo.id)
                .await
                .expect("delete stale");
        }
    }
}

/// Analyses a fixture under a repository ID unique to this test run.
fn fixture(name: &str) -> RepositoryAnalysis {
    let path = workspace_root().join("fixtures").join(name);
    let mut analysis =
        analyze_source(&RepoSource::Local(path), &IngestOptions::default()).expect("analysis");
    analysis.repository.id = format!(
        "test-{name}-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    );
    analysis
}

/// Indexes a fixture, runs `body`, and always deletes the repository.
macro_rules! with_indexed {
    ($fixture:expr, |$store:ident, $analysis:ident, $repo:ident| $body:block) => {{
        let Some($store) = store().await else { return };
        let $analysis = fixture($fixture);
        let $repo = $analysis.repository.id.clone();
        $store.index(&$analysis).await.expect("index");
        let result = async { $body }.await;
        $store.delete_repository(&$repo).await.expect("cleanup");
        result
    }};
}

fn ids(nodes: &[codeatlas_store::TraversalNode]) -> Vec<(&str, u32)> {
    let mut ids: Vec<_> = nodes
        .iter()
        .map(|n| (n.symbol.id.as_str(), n.depth))
        .collect();
    ids.sort();
    ids
}

const AUTHORIZE: &str = "method:simple_repo::payments::PaymentService::authorize";
const STRIPE: &str = "fn:simple_repo::payments::gateway::stripe_call";

#[tokio::test]
async fn stored_graph_matches_the_analysis() {
    with_indexed!("simple-repo", |store, analysis, repo| {
        let stats = store.graph_stats(&repo).await.unwrap();
        let symbols: usize = analysis.files.iter().map(|f| f.symbols.len()).sum();
        let tests = analysis
            .files
            .iter()
            .flat_map(|f| &f.symbols)
            .filter(|s| s.is_test)
            .count();
        let methods = analysis
            .files
            .iter()
            .flat_map(|f| &f.symbols)
            .filter(|s| s.kind == SymbolKind::Method)
            .count();
        let edges = |kind| {
            analysis
                .resolution
                .edges
                .iter()
                .filter(|e| e.kind == kind)
                .count() as i64
        };
        assert_eq!(stats.labels["Repository"], 1);
        assert_eq!(stats.labels["File"], analysis.files.len() as i64);
        assert_eq!(stats.labels["Crate"], analysis.crates.len() as i64);
        assert_eq!(stats.labels["Symbol"], symbols as i64);
        assert_eq!(stats.labels["Method"], methods as i64);
        assert_eq!(stats.labels["Test"], tests as i64);
        assert_eq!(stats.relationships["CALLS"], edges(EdgeKind::Calls));
        assert_eq!(stats.relationships["IMPORTS"], edges(EdgeKind::Imports));
        assert_eq!(stats.relationships["DEPENDS_ON"] as usize, {
            let deps = codeatlas_analyzer::dependencies::derive_dependencies(
                &analysis.files,
                &analysis.resolution.edges,
            );
            deps.files.len() + deps.modules.len()
        });

        let repository = store.repository(&repo).await.unwrap();
        assert_eq!(repository.name, "simple-repo");
        assert_eq!(repository.source_files, 6);
    });
}

#[tokio::test]
async fn reindexing_replaces_instead_of_duplicating() {
    with_indexed!("simple-repo", |store, analysis, repo| {
        let before = store.graph_stats(&repo).await.unwrap();
        store.index(&analysis).await.unwrap();
        let after = store.graph_stats(&repo).await.unwrap();
        assert_eq!(before, after);
    });
}

#[tokio::test]
async fn callers_and_callees_with_depth() {
    with_indexed!("simple-repo", |store, _analysis, repo| {
        let direct = store.callers(&repo, AUTHORIZE, 1).await.unwrap();
        assert_eq!(
            ids(&direct.nodes),
            vec![
                ("fn:simple_repo::payments::process_payment", 1),
                (
                    "fn:simple_repo::payments::tests::rejects_amounts_over_limit",
                    1
                ),
            ]
        );
        assert!(!direct.truncated);

        let transitive = store.callers(&repo, AUTHORIZE, 3).await.unwrap();
        assert_eq!(
            ids(&transitive.nodes),
            vec![
                ("fn:checkout_test::checkout_succeeds_for_small_orders", 3),
                ("fn:simple_repo::checkout::checkout", 2),
                ("fn:simple_repo::payments::process_payment", 1),
                (
                    "fn:simple_repo::payments::tests::rejects_amounts_over_limit",
                    1
                ),
            ]
        );
        // Evidence: checkout reaches authorize through process_payment, with lines.
        let path = transitive.path_to("fn:simple_repo::checkout::checkout");
        let hops: Vec<_> = path
            .iter()
            .map(|e| (e.from.as_str(), e.to.as_str(), e.lines.clone()))
            .collect();
        assert_eq!(
            hops,
            vec![
                (
                    "fn:simple_repo::payments::process_payment",
                    AUTHORIZE,
                    vec![27]
                ),
                (
                    "fn:simple_repo::checkout::checkout",
                    "fn:simple_repo::payments::process_payment",
                    vec![11]
                ),
            ]
        );

        let callees = store
            .callees(&repo, "fn:simple_repo::checkout::checkout", 10)
            .await
            .unwrap();
        assert_eq!(
            ids(&callees.nodes),
            vec![
                ("fn:simple_repo::inventory::reserve", 1),
                (STRIPE, 3),
                (AUTHORIZE, 2),
                ("method:simple_repo::payments::PaymentService::new", 2),
                ("fn:simple_repo::payments::process_payment", 1),
            ]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
        );
    });
}

#[tokio::test]
async fn related_tests_come_with_call_chains() {
    with_indexed!("simple-repo", |store, _analysis, repo| {
        let tests = store.related_tests(&repo, STRIPE, 5).await.unwrap();
        let summary: Vec<_> = tests
            .iter()
            .map(|t| {
                let chain: Vec<&str> = t.path.iter().map(|e| e.from.as_str()).collect();
                (t.test.id.as_str(), t.depth, chain)
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (
                    "fn:simple_repo::payments::tests::rejects_amounts_over_limit",
                    2,
                    vec![
                        "fn:simple_repo::payments::tests::rejects_amounts_over_limit",
                        AUTHORIZE
                    ]
                ),
                (
                    "fn:checkout_test::checkout_succeeds_for_small_orders",
                    4,
                    vec![
                        "fn:checkout_test::checkout_succeeds_for_small_orders",
                        "fn:simple_repo::checkout::checkout",
                        "fn:simple_repo::payments::process_payment",
                        AUTHORIZE
                    ]
                ),
            ]
        );
    });
}

#[tokio::test]
async fn shortest_dependency_path() {
    with_indexed!("simple-repo", |store, _analysis, repo| {
        let path = store
            .shortest_path(&repo, "fn:simple_repo::checkout::checkout", STRIPE, 10)
            .await
            .unwrap()
            .expect("path exists");
        let names: Vec<_> = path.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["checkout", "process_payment", "authorize", "stripe_call"]
        );
        assert!(path.edges.iter().all(|e| e.kind == "CALLS"));
        assert_eq!(path.edges[2].lines, vec![21]);

        // `checkout` is both a module and a function.
        assert!(matches!(
            store.find_symbol(&repo, "checkout").await,
            Err(StoreError::Ambiguous(_))
        ));
        let callable = store
            .find_symbol_preferring(&repo, "checkout", &["function", "method"])
            .await
            .unwrap();
        assert_eq!(callable.id, "fn:simple_repo::checkout::checkout");

        // Edges are directed: there is no path back up the call chain.
        let none = store
            .shortest_path(&repo, STRIPE, "fn:simple_repo::checkout::checkout", 10)
            .await
            .unwrap();
        assert!(none.is_none());
    });
}

#[tokio::test]
async fn file_and_module_dependencies() {
    with_indexed!("simple-repo", |store, _analysis, repo| {
        let dependents = store
            .file_dependencies(&repo, "src/payments/gateway.rs", Direction::Dependents)
            .await
            .unwrap();
        let dependents: Vec<_> = dependents
            .iter()
            .map(|d| (d.target.as_str(), d.weight))
            .collect();
        assert_eq!(dependents, vec![("src/payments/mod.rs", 1)]);

        let dependencies = store
            .file_dependencies(&repo, "src/checkout.rs", Direction::Dependencies)
            .await
            .unwrap();
        let dependencies: Vec<_> = dependencies
            .iter()
            .map(|d| (d.target.as_str(), d.weight))
            .collect();
        assert_eq!(
            dependencies,
            vec![("src/payments/mod.rs", 3), ("src/inventory.rs", 2)]
        );

        let modules = store
            .module_dependencies(&repo, "mod:simple_repo::payments", Direction::Dependents)
            .await
            .unwrap();
        let modules: Vec<&str> = modules.iter().map(|d| d.target.as_str()).collect();
        assert!(modules.contains(&"mod:simple_repo::checkout"));
        assert!(modules.contains(&"mod:simple_repo::payments::tests"));

        let missing = store
            .file_dependencies(&repo, "src/nope.rs", Direction::Dependents)
            .await;
        assert!(matches!(missing, Err(StoreError::NotFound(_))));
    });
}

#[tokio::test]
async fn search_and_symbol_lookup() {
    with_indexed!("duplicate-symbols", |store, _analysis, repo| {
        // Prefix matching also finds the `Authorizer` trait; exact name
        // matches rank first.
        let hits = store.search(&repo, "authorize", None, 20).await.unwrap();
        let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, [vec!["authorize"; 6], vec!["Authorizer"]].concat());

        let only_functions = store
            .search(&repo, "authorize", Some(&["function".to_string()]), 20)
            .await
            .unwrap();
        assert_eq!(only_functions.len(), 1);

        // Pages: 7 hits split 4 + 3, no overlap, `has_more` only on the first.
        let first = store
            .search_page(&repo, "authorize", None, 4, 0)
            .await
            .unwrap();
        let second = store
            .search_page(&repo, "authorize", None, 4, 4)
            .await
            .unwrap();
        assert_eq!((first.symbols.len(), first.has_more), (4, true));
        assert_eq!((second.symbols.len(), second.has_more), (3, false));
        let mut paged: Vec<&str> = first
            .symbols
            .iter()
            .chain(&second.symbols)
            .map(|s| s.id.as_str())
            .collect();
        paged.sort();
        paged.dedup();
        assert_eq!(paged.len(), 7);

        let prefix = store.search(&repo, "OAuthServ", None, 20).await.unwrap();
        assert_eq!(prefix[0].id, "struct:duplicate_symbols::auth::OAuthService");

        let by_suffix = store
            .find_symbol(&repo, "OAuthService::authorize")
            .await
            .unwrap();
        assert_eq!(
            by_suffix.id,
            "method:duplicate_symbols::auth::OAuthService::authorize"
        );
        let ambiguous = store.find_symbol(&repo, "authorize").await;
        assert!(matches!(ambiguous, Err(StoreError::Ambiguous(_))));
        let missing = store.find_symbol(&repo, "does_not_exist").await;
        assert!(matches!(missing, Err(StoreError::NotFound(_))));
    });
}

#[tokio::test]
async fn ambiguous_and_unresolved_calls_are_stored_explicitly() {
    with_indexed!("unresolved", |store, _analysis, repo| {
        let candidates = store
            .traverse(
                &repo,
                "fn:unresolved::jobs::run_all",
                Direction::Dependencies,
                &[Relation::CallsCandidate],
                1,
            )
            .await
            .unwrap();
        assert_eq!(candidates.nodes.len(), 3);
        assert!(candidates.edges.iter().all(|e| e.kind == "CALLS_CANDIDATE"));
        // Resolved-call traversal does not include candidates.
        let calls = store
            .callees(&repo, "fn:unresolved::jobs::run_all", 1)
            .await
            .unwrap();
        assert!(calls.nodes.is_empty());

        let legacy = store
            .symbol(&repo, "fn:unresolved::legacy::run")
            .await
            .unwrap();
        assert_eq!(
            legacy.unresolved_calls,
            vec![
                "27|normalize|name_not_in_scope",
                "28|crate::util::missing|path_segment_not_found"
            ]
        );
    });
}

#[tokio::test]
async fn limits_bound_every_traversal() {
    let Some(store) = store().await else { return };
    let store = store.with_limits(QueryLimits {
        max_depth: 3,
        max_nodes: 1,
        max_results: 5,
    });
    let analysis = fixture("simple-repo");
    let repo = analysis.repository.id.clone();
    store.index(&analysis).await.unwrap();

    let too_deep = store.callers(&repo, AUTHORIZE, 4).await;
    assert!(matches!(too_deep, Err(StoreError::InvalidArgument(_))));
    let zero = store.callers(&repo, AUTHORIZE, 0).await;
    assert!(matches!(zero, Err(StoreError::InvalidArgument(_))));

    let capped = store.callers(&repo, AUTHORIZE, 3).await.unwrap();
    assert_eq!(capped.nodes.len(), 1);
    assert!(capped.truncated);

    store.delete_repository(&repo).await.unwrap();
}

#[tokio::test]
async fn deleting_a_repository_removes_its_graph() {
    let Some(store) = store().await else { return };
    let analysis = fixture("cross-module");
    let repo = analysis.repository.id.clone();
    store.index(&analysis).await.unwrap();
    assert!(store.delete_repository(&repo).await.unwrap() > 0);
    let stats = store.graph_stats(&repo).await.unwrap();
    assert!(stats.labels.is_empty() && stats.relationships.is_empty());
    assert!(matches!(
        store.repository(&repo).await,
        Err(StoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn loaded_graph_equals_the_analysis_graph() {
    use codeatlas_analyzer::graph::architecture::{cycles, Level};
    use codeatlas_analyzer::graph::impact::{impact_of_symbol, ImpactOptions};
    use codeatlas_analyzer::graph::CodeGraph;
    use codeatlas_analyzer::model::SymbolId;

    let Some(store) = store().await else { return };
    for name in [
        "simple-repo",
        "duplicate-symbols",
        "cross-module",
        "unresolved",
        "change-impact",
        "circular-dependency",
    ] {
        let analysis = fixture(name);
        let repo = analysis.repository.id.clone();
        store.index(&analysis).await.unwrap();
        let loaded = store.load_graph(&repo).await.unwrap();
        let direct = CodeGraph::from_analysis(&analysis);
        store.delete_repository(&repo).await.unwrap();
        assert!(
            loaded == direct,
            "{name}: stored graph differs from analysis graph"
        );

        if name == "change-impact" {
            let symbol =
                SymbolId::from_stored("fn:change_impact::payments::validate_amount".to_string());
            let options = ImpactOptions::default();
            assert_eq!(
                impact_of_symbol(&loaded, &symbol, options).unwrap(),
                impact_of_symbol(&direct, &symbol, options).unwrap()
            );
        }
        if name == "circular-dependency" {
            assert_eq!(
                cycles(&loaded, Level::Module),
                cycles(&direct, Level::Module)
            );
            assert_eq!(cycles(&loaded, Level::Module).len(), 1);
        }
    }
}

#[tokio::test]
async fn loading_an_unknown_repository_fails() {
    let Some(store) = store().await else { return };
    assert!(matches!(
        store.load_graph("does-not-exist").await,
        Err(StoreError::NotFound(_))
    ));
}

/// Opt-in check on a real repository: `CODEATLAS_EQUIVALENCE_REPO=<path>`.
#[tokio::test]
async fn loaded_graph_equals_analysis_graph_on_a_real_repository() {
    let Ok(path) = std::env::var("CODEATLAS_EQUIVALENCE_REPO") else {
        eprintln!("skipping: CODEATLAS_EQUIVALENCE_REPO not set");
        return;
    };
    let Some(store) = store().await else { return };
    let mut analysis =
        analyze_source(&RepoSource::Local(path.into()), &IngestOptions::default()).unwrap();
    analysis.repository.id = format!("test-equivalence-{}", std::process::id());
    let repo = analysis.repository.id.clone();
    store.index(&analysis).await.unwrap();
    let loaded = store.load_graph(&repo).await.unwrap();
    store.delete_repository(&repo).await.unwrap();
    let direct = codeatlas_analyzer::graph::CodeGraph::from_analysis(&analysis);
    assert_eq!(loaded.len(), direct.len());
    assert_eq!(loaded.edges().len(), direct.edges().len());
    assert!(loaded == direct, "stored graph differs from analysis graph");
}

#[tokio::test]
async fn graphs_written_by_another_format_version_are_rejected() {
    with_indexed!("simple-repo", |store, _analysis, repo| {
        assert!(store.load_graph(&repo).await.is_ok());
        store.set_format_version_for_tests(&repo, 1).await.unwrap();
        assert!(matches!(
            store.load_graph(&repo).await,
            Err(StoreError::OutdatedIndex { found: 1, .. })
        ));
    });
}

#[tokio::test]
async fn crates_and_children_form_the_explorer_tree() {
    with_indexed!("simple-repo", |store, _analysis, repo| {
        let crates = store.crates(&repo).await.unwrap();
        let summary: Vec<(&str, &str, Option<&str>)> = crates
            .iter()
            .map(|c| (c.name.as_str(), c.kind.as_str(), c.root_module.as_deref()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("simple_repo", "lib", Some("mod:simple_repo")),
                ("checkout_test", "test", Some("mod:checkout_test")),
            ]
        );

        let names = |symbols: Vec<codeatlas_store::SymbolNode>| -> Vec<String> {
            symbols.into_iter().map(|s| s.name).collect()
        };
        assert_eq!(
            names(store.children(&repo, "mod:simple_repo").await.unwrap()),
            vec!["checkout", "inventory", "payments"]
        );
        assert_eq!(
            names(
                store
                    .children(&repo, "mod:simple_repo::payments")
                    .await
                    .unwrap()
            ),
            vec![
                "gateway",
                "tests",
                "PaymentService",
                "PaymentError",
                "process_payment"
            ]
        );
        assert_eq!(
            names(
                store
                    .children(&repo, "struct:simple_repo::payments::PaymentService")
                    .await
                    .unwrap()
            ),
            vec!["authorize", "new"]
        );
    });
}

// ---- Incremental indexing ----------------------------------------------------

mod incremental {
    use std::fs;
    use std::path::Path;

    use codeatlas_analyzer::analyze;
    use codeatlas_analyzer::ingest::{ingest, IngestOptions, IngestedRepository};
    use codeatlas_analyzer::RepoSource;
    use codeatlas_store::{prepare, DeltaStats, FullReason, IndexMode, IndexReport, StateDir};

    use super::{store, workspace_root, NEXT_ID};
    use std::sync::atomic::Ordering;

    fn copy_dir(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    fn replace_with(dir: &Path, fixture: &str) {
        fs::remove_dir_all(dir).unwrap();
        copy_dir(&workspace_root().join("fixtures").join(fixture), dir);
    }

    fn test_id(name: &str) -> String {
        format!(
            "test-inc-{name}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn ingested(dir: &Path, id: &str) -> IngestedRepository {
        let mut repo = ingest(
            &RepoSource::Local(dir.to_path_buf()),
            &IngestOptions::default(),
        )
        .unwrap();
        repo.info.id = id.to_string();
        repo
    }

    async fn index(
        store: &codeatlas_store::GraphStore,
        dir: &Path,
        id: &str,
        states: &StateDir,
        full: bool,
    ) -> IndexReport {
        let prepared = prepare(&ingested(dir, id), states).unwrap();
        store.index_prepared(prepared, states, full).await.unwrap()
    }

    /// The stored graph of `id` equals a full index of `dir`.
    async fn assert_matches_full_index(store: &codeatlas_store::GraphStore, dir: &Path, id: &str) {
        let reference = test_id("reference");
        let mut analysis = analyze(&ingested(dir, &reference)).unwrap();
        analysis.repository.id = reference.clone();
        store.index(&analysis).await.unwrap();
        let (incremental, full) = (
            store.dump_for_tests(id).await.unwrap(),
            store.dump_for_tests(&reference).await.unwrap(),
        );
        store.delete_repository(&reference).await.unwrap();
        // The comparison must cover real content: symbols, calls, files.
        for prefix in [
            "node File",
            "node Function",
            "rel CALLS",
            "rel DEFINES",
            "rel DEPENDS_ON",
        ] {
            assert!(
                full.iter().any(|l| l.starts_with(prefix)),
                "dump has no `{prefix}` lines"
            );
        }
        if incremental != full {
            let only_inc: Vec<_> = incremental.iter().filter(|l| !full.contains(l)).collect();
            let only_full: Vec<_> = full.iter().filter(|l| !incremental.contains(l)).collect();
            panic!(
                "stored graphs differ\nonly incremental: {only_inc:#?}\nonly full: {only_full:#?}"
            );
        }
    }

    #[tokio::test]
    async fn incremental_index_matches_a_full_index() {
        let Some(store) = store().await else { return };
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("pr-shop");
        copy_dir(&workspace_root().join("fixtures/pr-impact/base"), &dir);
        let states = StateDir::new(tmp.path().join("state"));
        let id = test_id("pr-shop");

        let first = index(&store, &dir, &id, &states, false).await;
        assert_eq!(first.mode, IndexMode::Full(FullReason::NotIndexed));
        assert!(first.state_saved);

        // Nothing changed: nothing parsed, nothing written but the
        // repository node.
        let again = index(&store, &dir, &id, &states, false).await;
        assert_eq!(again.mode, IndexMode::Incremental);
        assert_eq!(again.reuse.parsed, 0);
        assert_eq!(again.delta, DeltaStats::default());
        assert_matches_full_index(&store, &dir, &id).await;

        // Edits, a new file, a removed file and a rename.
        replace_with(&dir, "pr-impact/head");
        let head = index(&store, &dir, &id, &states, false).await;
        assert_eq!(head.mode, IndexMode::Incremental);
        assert_eq!((head.reuse.parsed, head.reuse.reused), (8, 1));
        assert!(head.delta.nodes_added > 0 && head.delta.nodes_removed > 0);
        assert!(head.delta.relationships_removed > 0 && head.delta.relationships_changed > 0);
        assert_eq!(
            (head.summary.nodes, head.summary.relationships),
            (
                first.summary.nodes + head.delta.nodes_added - head.delta.nodes_removed,
                first.summary.relationships + head.delta.relationships_added
                    - head.delta.relationships_removed
            )
        );
        assert_matches_full_index(&store, &dir, &id).await;

        // And back.
        replace_with(&dir, "pr-impact/base");
        let back = index(&store, &dir, &id, &states, false).await;
        assert_eq!(back.mode, IndexMode::Incremental);
        assert_matches_full_index(&store, &dir, &id).await;

        store.delete_repository(&id).await.unwrap();
    }

    #[tokio::test]
    async fn module_moves_and_test_attributes_are_applied_incrementally() {
        let Some(store) = store().await else { return };
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("change-impact");
        copy_dir(&workspace_root().join("fixtures/change-impact"), &dir);
        let states = StateDir::new(tmp.path().join("state"));
        let id = test_id("change-impact");
        index(&store, &dir, &id, &states, false).await;

        // `reports` becomes `summary` (same file); one test loses `#[test]`
        // and a helper gains it.
        let lib = dir.join("src/lib.rs");
        let text = fs::read_to_string(&lib).unwrap();
        fs::write(
            &lib,
            text.replace(
                "pub mod reports;",
                "#[path = \"reports.rs\"]\npub mod summary;",
            ),
        )
        .unwrap();
        let tests = dir.join("tests/payment_tests.rs");
        let text = fs::read_to_string(&tests).unwrap();
        let text = text
            .replace("#[test]\nfn test_refund()", "fn test_refund()")
            .replace(
                "use change_impact::reports::daily_total;",
                "use change_impact::summary::daily_total;",
            );
        fs::write(&tests, text).unwrap();

        let report = index(&store, &dir, &id, &states, false).await;
        assert_eq!(report.mode, IndexMode::Incremental);
        assert!(report.delta.nodes_changed > 0);
        assert_matches_full_index(&store, &dir, &id).await;
        store.delete_repository(&id).await.unwrap();
    }

    /// Opt-in check on a real repository: `CODEATLAS_EQUIVALENCE_REPO=<path>`
    /// and `CODEATLAS_EQUIVALENCE_BASE=<revision>` (default `HEAD~10`).
    /// Both revisions are exported with `git archive`; the repository's
    /// checkout is not touched.
    #[tokio::test]
    async fn incremental_index_matches_a_full_index_on_a_real_repository() {
        let Ok(path) = std::env::var("CODEATLAS_EQUIVALENCE_REPO") else {
            eprintln!("skipping: CODEATLAS_EQUIVALENCE_REPO not set");
            return;
        };
        let base = std::env::var("CODEATLAS_EQUIVALENCE_BASE").unwrap_or_else(|_| "HEAD~10".into());
        let Some(store) = store().await else { return };
        let git = codeatlas_analyzer::git::Git::discover(Path::new(&path))
            .unwrap()
            .unwrap();
        let prefix = codeatlas_analyzer::git::Git::prefix_of(Path::new(&path)).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("repo");
        let export = |revision: &str| {
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let sha = git.resolve_commit(revision).unwrap();
            git.export(&sha, &prefix, &dir).unwrap();
        };
        let states = StateDir::new(tmp.path().join("state"));
        let id = test_id("real");

        export(&base);
        index(&store, &dir, &id, &states, false).await;
        export("HEAD");
        let forward = index(&store, &dir, &id, &states, false).await;
        assert_eq!(forward.mode, IndexMode::Incremental);
        assert_matches_full_index(&store, &dir, &id).await;
        export(&base);
        let back = index(&store, &dir, &id, &states, false).await;
        assert_eq!(back.mode, IndexMode::Incremental);
        assert_matches_full_index(&store, &dir, &id).await;
        eprintln!(
            "forward: {:?} {:?}; back: {:?} {:?}",
            forward.reuse, forward.delta, back.reuse, back.delta
        );
        store.delete_repository(&id).await.unwrap();
    }

    #[tokio::test]
    async fn full_writes_when_the_state_does_not_describe_the_stored_graph() {
        let Some(store) = store().await else { return };
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("simple-repo");
        copy_dir(&workspace_root().join("fixtures/simple-repo"), &dir);
        let states = StateDir::new(tmp.path().join("state"));
        let id = test_id("simple-repo");
        index(&store, &dir, &id, &states, false).await;

        // Another writer replaced the graph: its token differs.
        let mut analysis = analyze(&ingested(&dir, &id)).unwrap();
        analysis.repository.id = id.clone();
        store.index(&analysis).await.unwrap();
        let report = index(&store, &dir, &id, &states, false).await;
        assert_eq!(report.mode, IndexMode::Full(FullReason::IndexChanged));
        // The state saved by that full write is usable again; the analysis
        // still reused every file.
        assert_eq!(report.reuse.parsed, 0);
        let report = index(&store, &dir, &id, &states, false).await;
        assert_eq!(report.mode, IndexMode::Incremental);

        let report = index(&store, &dir, &id, &states, true).await;
        assert_eq!(report.mode, IndexMode::Full(FullReason::Requested));

        states.remove(&id).unwrap();
        let report = index(&store, &dir, &id, &states, false).await;
        assert_eq!(report.mode, IndexMode::Full(FullReason::NoState));
        assert_eq!(report.reuse.reused, 0);
        assert_matches_full_index(&store, &dir, &id).await;
        store.delete_repository(&id).await.unwrap();
    }
}
