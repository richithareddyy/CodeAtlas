//! End-to-end API tests: HTTP requests through the axum router against a
//! real Neo4j. They run when `CODEATLAS_NEO4J_PASSWORD` is set (directly or
//! in the workspace `.env`) and are skipped otherwise. Each test indexes
//! fixtures under its own repository IDs and removes them afterwards.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use codeatlas_analyzer::ingest::IngestOptions;
use codeatlas_analyzer::{analyze_source, RepoSource};
use codeatlas_server::{router, AppState, ServerConfig};
use codeatlas_store::{GraphStore, StoreConfig};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

static NEXT_ID: AtomicU32 = AtomicU32::new(0);

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn config(allow_indexing: bool) -> ServerConfig {
    ServerConfig {
        addr: "127.0.0.1:0".parse().unwrap(),
        cors_origins: vec!["http://localhost:5173".into()],
        allow_indexing,
        graphiql: true,
        clone_dir: std::env::temp_dir().join("codeatlas-test-clones"),
    }
}

struct Api {
    app: Router,
    state: Arc<AppState>,
    repos: Vec<String>,
}

impl Api {
    /// Connects, indexes `fixtures` under fresh IDs, and builds the router.
    async fn start(fixtures: &[&str], allow_indexing: bool) -> Option<Self> {
        let _ = dotenvy::from_path(workspace_root().join(".env"));
        let Ok(store_config) = StoreConfig::from_env() else {
            eprintln!("skipping: CODEATLAS_NEO4J_PASSWORD not set");
            return None;
        };
        let store = GraphStore::connect(&store_config)
            .await
            .expect("Neo4j is configured but not reachable; is `docker compose up -d` running?");
        store.ensure_schema().await.unwrap();
        // A failed assertion skips cleanup; remove test repositories left by
        // earlier processes (never this one's, which may run in parallel).
        let own = format!("-{}-", std::process::id());
        for stale in store.repositories().await.unwrap() {
            if stale.id.starts_with("test-") && !stale.id.contains(&own) {
                store.delete_repository(&stale.id).await.unwrap();
            }
        }
        let mut repos = Vec::new();
        for name in fixtures {
            let path = workspace_root().join("fixtures").join(name);
            let mut analysis =
                analyze_source(&RepoSource::Local(path), &IngestOptions::default()).unwrap();
            analysis.repository.id = format!(
                "test-api-{name}-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            );
            store.index(&analysis).await.unwrap();
            repos.push(analysis.repository.id);
        }
        let config = config(allow_indexing);
        let state = AppState::new(store, allow_indexing, config.clone_dir.clone());
        Some(Self {
            app: router(state.clone(), &config),
            state,
            repos,
        })
    }

    async fn finish(self) {
        for repo in &self.repos {
            self.state.store.delete_repository(repo).await.unwrap();
        }
    }

    async fn request(&self, request: Request<Body>) -> (StatusCode, Value) {
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, body)
    }

    /// Runs a GraphQL operation and returns the whole response body.
    async fn graphql(&self, query: &str, variables: Value) -> Value {
        let request = Request::post("/graphql")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "query": query, "variables": variables }).to_string(),
            ))
            .unwrap();
        let (status, body) = self.request(request).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    /// Runs a query that must succeed and returns `data`.
    async fn data(&self, query: &str, variables: Value) -> Value {
        let body = self.graphql(query, variables).await;
        assert!(body.get("errors").is_none(), "unexpected errors: {body}");
        body["data"].clone()
    }

    /// Runs a query that must fail and returns the first error's code.
    async fn error_code(&self, query: &str, variables: Value) -> String {
        let body = self.graphql(query, variables).await;
        body["errors"][0]["extensions"]["code"]
            .as_str()
            .unwrap_or_else(|| panic!("expected a coded error: {body}"))
            .to_string()
    }
}

fn strings(value: &Value, field: &str) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v[field].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn health_and_cors() {
    let Some(api) = Api::start(&[], true).await else {
        return;
    };
    let (status, body) = api
        .request(Request::get("/health").body(Body::empty()).unwrap())
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["neo4j"], "ok");

    let preflight = |origin: &str| {
        Request::builder()
            .method("OPTIONS")
            .uri("/graphql")
            .header(header::ORIGIN, origin)
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
            .body(Body::empty())
            .unwrap()
    };
    let allowed = api
        .app
        .clone()
        .oneshot(preflight("http://localhost:5173"))
        .await
        .unwrap();
    assert_eq!(
        allowed.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
        "http://localhost:5173"
    );
    let other = api
        .app
        .clone()
        .oneshot(preflight("https://example.com"))
        .await
        .unwrap();
    assert!(other
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        .is_none());
    api.finish().await;
}

#[tokio::test]
async fn repositories_symbols_and_paginated_search() {
    let Some(api) = Api::start(&["duplicate-symbols"], true).await else {
        return;
    };
    let repo = &api.repos[0];

    let data = api
        .data(
            "query($id: ID!) { repository(id: $id) { name sourceFiles formatVersion } }",
            json!({ "id": repo }),
        )
        .await;
    assert_eq!(data["repository"]["name"], "duplicate-symbols");
    assert_eq!(data["repository"]["formatVersion"], 2);

    let missing = api
        .data("{ repository(id: \"nope\") { id } }", json!({}))
        .await;
    assert!(missing["repository"].is_null());

    let page = "query($repo: ID!, $after: String) { searchSymbols(repoId: $repo, query: \"authorize\", first: 4, after: $after) { \
                pageInfo { hasNextPage hasPreviousPage endCursor } edges { cursor node { id name kind } } } }";
    let first = api.data(page, json!({ "repo": repo, "after": null })).await;
    let first = &first["searchSymbols"];
    assert_eq!(first["edges"].as_array().unwrap().len(), 4);
    assert_eq!(first["pageInfo"]["hasNextPage"], true);
    assert_eq!(first["pageInfo"]["hasPreviousPage"], false);

    let after = first["pageInfo"]["endCursor"].clone();
    let second = api
        .data(page, json!({ "repo": repo, "after": after }))
        .await;
    let second = &second["searchSymbols"];
    assert_eq!(second["edges"].as_array().unwrap().len(), 3);
    assert_eq!(second["pageInfo"]["hasNextPage"], false);
    assert_eq!(second["edges"][2]["node"]["name"], "Authorizer");

    let traits = api
        .data(
            "query($repo: ID!) { searchSymbols(repoId: $repo, query: \"author\", kinds: [TRAIT]) { edges { node { kind qualifiedName } } } }",
            json!({ "repo": repo }),
        )
        .await;
    assert_eq!(traits["searchSymbols"]["edges"][0]["node"]["kind"], "TRAIT");

    let symbol = api
        .data(
            "query($repo: ID!) { symbol(repoId: $repo, id: \"fn:duplicate_symbols::payments::charge\") { name startLine signature isTest } }",
            json!({ "repo": repo }),
        )
        .await;
    assert_eq!(symbol["symbol"]["startLine"], 19);
    api.finish().await;
}

#[tokio::test]
async fn neighborhoods_paths_and_source() {
    let Some(api) = Api::start(&["simple-repo"], true).await else {
        return;
    };
    let repo = &api.repos[0];
    let vars = json!({ "repo": repo });

    let data = api
        .data(
            "query($repo: ID!) { dependents(repoId: $repo, symbolId: \"method:simple_repo::payments::PaymentService::authorize\", depth: 3) { \
             truncated nodes { depth symbol { id } via { from to relation resolution lines } } } }",
            vars.clone(),
        )
        .await;
    let nodes = data["dependents"]["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 4);
    let checkout = nodes
        .iter()
        .find(|n| n["symbol"]["id"] == "fn:simple_repo::checkout::checkout")
        .unwrap();
    assert_eq!(checkout["depth"], 2);
    assert_eq!(checkout["via"]["relation"], "CALLS");
    assert_eq!(checkout["via"]["lines"], json!([11]));

    let path = api
        .data(
            "query($repo: ID!) { dependencyPath(repoId: $repo, from: \"fn:simple_repo::checkout::checkout\", to: \"fn:simple_repo::payments::gateway::stripe_call\") { nodes { name } } }",
            vars.clone(),
        )
        .await;
    assert_eq!(
        strings(&path["dependencyPath"]["nodes"], "name"),
        vec!["checkout", "process_payment", "authorize", "stripe_call"]
    );

    let deps = api
        .data(
            "query($repo: ID!) { fileDependencies(repoId: $repo, path: \"src/checkout.rs\", direction: DEPENDENCIES) { target weight } }",
            vars.clone(),
        )
        .await;
    assert_eq!(deps["fileDependencies"][0]["target"], "src/payments/mod.rs");
    assert_eq!(deps["fileDependencies"][0]["weight"], 3);

    let source = api
        .data(
            "query($repo: ID!) { source(repoId: $repo, file: \"src/payments/gateway.rs\", startLine: 3, endLine: 4) { lines totalLines } }",
            vars.clone(),
        )
        .await;
    assert_eq!(
        source["source"]["lines"][0],
        "pub(crate) fn stripe_call(amount_cents: u64) -> Result<(), PaymentError> {"
    );
    assert_eq!(source["source"]["totalLines"], 9);
    api.finish().await;
}

#[tokio::test]
async fn impact_affected_tests_and_architecture() {
    let Some(api) = Api::start(&["change-impact", "circular-dependency"], true).await else {
        return;
    };
    let (impact_repo, cyclic_repo) = (&api.repos[0], &api.repos[1]);

    let data = api
        .data(
            "query($repo: ID!) { impact(repoId: $repo, symbolId: \"fn:change_impact::payments::validate_amount\") { \
             directCount indirectCount tests score { total level factors { name value } } \
             affected { depth confidence symbol { id } path { source target kind file lines } } } }",
            json!({ "repo": impact_repo }),
        )
        .await;
    let impact = &data["impact"];
    assert_eq!(
        (
            impact["directCount"].as_i64(),
            impact["indirectCount"].as_i64()
        ),
        (Some(1), Some(3))
    );
    assert_eq!(impact["score"]["total"], 41.2);
    assert_eq!(impact["score"]["level"], "MEDIUM");
    assert_eq!(
        impact["tests"],
        json!([
            "fn:payment_tests::test_authorize_valid",
            "fn:payment_tests::test_checkout"
        ])
    );
    let deepest = &impact["affected"][3];
    assert_eq!(deepest["symbol"]["id"], "fn:payment_tests::test_checkout");
    assert_eq!(deepest["path"].as_array().unwrap().len(), 3);
    assert_eq!(deepest["path"][0]["kind"], "CALLS");
    assert_eq!(deepest["path"][0]["lines"], json!([16]));

    let tests = api
        .data(
            "query($repo: ID!) { affectedTests(repoId: $repo, symbolId: \"method:change_impact::gateway::<StripeGateway as Gateway>::charge\") { \
             depth test { id } path { kind } } }",
            json!({ "repo": impact_repo }),
        )
        .await;
    let tests = tests["affectedTests"].as_array().unwrap();
    assert_eq!(tests.len(), 2);
    assert!(tests.iter().all(|t| t["path"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["kind"] == "DISPATCHES_TO")));

    let by_file = api
        .data(
            "query($repo: ID!) { impact(repoId: $repo, file: \"src/reports.rs\") { tests } }",
            json!({ "repo": impact_repo }),
        )
        .await;
    assert_eq!(
        by_file["impact"]["tests"],
        json!(["fn:payment_tests::test_daily_total"])
    );

    let cycles = api
        .data(
            "query($repo: ID!) { circularDependencies(repoId: $repo) { members hops { from to evidence { file line } } } \
             functions: circularDependencies(repoId: $repo, level: FUNCTION) { members } \
             layers(repoId: $repo) { layer members } }",
            json!({ "repo": cyclic_repo }),
        )
        .await;
    assert_eq!(
        cycles["circularDependencies"][0]["members"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        cycles["circularDependencies"][0]["hops"][0]["evidence"][0],
        json!({ "file": "src/checkout.rs", "line": 5 })
    );
    assert_eq!(cycles["functions"].as_array().unwrap().len(), 2);
    assert_eq!(
        cycles["layers"][0]["members"],
        json!(["mod:circular_dependency::util"])
    );

    let hotspots = api
        .data(
            "query($repo: ID!) { hotspots(repoId: $repo, first: 1) { name betweenness fanIn } }",
            json!({ "repo": impact_repo }),
        )
        .await;
    assert_eq!(
        hotspots["hotspots"][0],
        json!({ "name": "method:change_impact::payments::PaymentService::authorize", "betweenness": 6.0, "fanIn": 2 })
    );
    api.finish().await;
}

#[tokio::test]
async fn errors_carry_codes() {
    let Some(api) = Api::start(&["simple-repo"], false).await else {
        return;
    };
    let repo = api.repos[0].clone();
    let vars = json!({ "repo": repo });

    assert_eq!(
        api.error_code(
            "{ dependents(repoId: \"nope\", symbolId: \"x\") { truncated } }",
            json!({})
        )
        .await,
        "NOT_FOUND"
    );
    assert_eq!(
        api.error_code(
            "query($repo: ID!) { dependents(repoId: $repo, symbolId: \"fn:simple_repo::checkout::checkout\", depth: 0) { truncated } }",
            vars.clone()
        )
        .await,
        "BAD_USER_INPUT"
    );
    assert_eq!(
        api.error_code(
            "query($repo: ID!) { impact(repoId: $repo, symbolId: \"fn:simple_repo::checkout::checkout\", file: \"src/lib.rs\") { truncated } }",
            vars.clone()
        )
        .await,
        "BAD_USER_INPUT"
    );
    assert_eq!(
        api.error_code(
            "query($repo: ID!) { impact(repoId: $repo, symbolId: \"fn:missing\") { truncated } }",
            vars.clone()
        )
        .await,
        "NOT_FOUND"
    );
    assert_eq!(
        api.error_code(
            "query($repo: ID!) { source(repoId: $repo, file: \"../Cargo.toml\", startLine: 1, endLine: 2) { lines } }",
            vars.clone()
        )
        .await,
        "BAD_USER_INPUT"
    );
    assert_eq!(
        api.error_code(
            "mutation { indexRepository(source: \".\") { nodes } }",
            json!({})
        )
        .await,
        "FORBIDDEN"
    );

    api.state
        .store
        .set_format_version_for_tests(&repo, 1)
        .await
        .unwrap();
    assert_eq!(
        api.error_code(
            "query($repo: ID!) { circularDependencies(repoId: $repo) { members } }",
            vars
        )
        .await,
        "OUTDATED_INDEX"
    );
    api.finish().await;
}

/// Copies a fixture to a temporary directory, so the indexed repository
/// gets an ID of its own.
fn copy_fixture(name: &str) -> tempfile::TempDir {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    copy(&workspace_root().join("fixtures").join(name), dir.path());
    dir
}

#[tokio::test]
async fn index_reindex_and_remove_through_mutations() {
    let Some(api) = Api::start(&[], true).await else {
        return;
    };
    let copy = copy_fixture("change-impact");
    let source = copy.path().to_string_lossy().into_owned();

    let mutation = "mutation($source: String!) { indexRepository(source: $source) { \
                    nodes relationships filesAnalyzed resolutionRate repository { id name } } }";
    let indexed = api.data(mutation, json!({ "source": source })).await;
    let result = &indexed["indexRepository"];
    assert_eq!(result["nodes"], 38);
    assert_eq!(result["filesAnalyzed"], 7);
    let repo = result["repository"]["id"].as_str().unwrap().to_string();

    // A query loads and caches the graph.
    let impact = "query($repo: ID!) { impact(repoId: $repo, symbolId: \"fn:change_impact::reports::daily_total\") { tests } }";
    let before = api.data(impact, json!({ "repo": repo })).await;
    assert_eq!(
        before["impact"]["tests"],
        json!(["fn:payment_tests::test_daily_total"])
    );
    assert_eq!(api.state.graphs.len().await, 1);

    // Change the source and re-index: the cached graph must not be reused.
    std::fs::write(
        copy.path().join("tests/payment_tests.rs"),
        "use change_impact::reports::daily_total;\n\n#[test]\nfn test_renamed_total() {\n    assert_eq!(daily_total(&[2]), 2);\n}\n",
    )
    .unwrap();
    api.data(mutation, json!({ "source": source })).await;
    let after = api.data(impact, json!({ "repo": repo })).await;
    assert_eq!(
        after["impact"]["tests"],
        json!(["fn:payment_tests::test_renamed_total"])
    );

    let removed = api
        .data(
            "mutation($id: ID!) { removeRepository(id: $id) }",
            json!({ "id": repo }),
        )
        .await;
    assert_eq!(removed["removeRepository"], true);
    let again = api
        .data(
            "mutation($id: ID!) { removeRepository(id: $id) }",
            json!({ "id": repo }),
        )
        .await;
    assert_eq!(again["removeRepository"], false);
    api.finish().await;
}

#[tokio::test]
async fn explorer_tree_and_architecture_graph() {
    let Some(api) = Api::start(&["simple-repo"], true).await else {
        return;
    };
    let vars = json!({ "repo": api.repos[0] });
    let data = api
        .data(
            "query($repo: ID!) { crates(repoId: $repo) { name kind rootModule } \
             children(repoId: $repo, id: \"mod:simple_repo::payments\") { name kind } \
             architectureGraph(repoId: $repo, level: CRATE) { nodes { id fanIn fanOut inCycle } edges { from to weight } } \
             modules: architectureGraph(repoId: $repo) { nodes { id inCycle } } }",
            vars,
        )
        .await;
    assert_eq!(
        data["crates"][0],
        json!({ "name": "simple_repo", "kind": "lib", "rootModule": "mod:simple_repo" })
    );
    assert_eq!(
        strings(&data["children"], "name"),
        vec![
            "gateway",
            "tests",
            "PaymentService",
            "PaymentError",
            "process_payment"
        ]
    );
    assert_eq!(
        data["architectureGraph"]["edges"],
        json!([{ "from": "checkout_test", "to": "simple_repo", "weight": 3 }])
    );
    let cyclic: Vec<&Value> = data["modules"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["inCycle"] == true)
        .collect();
    assert_eq!(cyclic.len(), 2);
    api.finish().await;
}
