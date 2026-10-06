//! `codeatlas bench` on a small fixture: the report has every section and
//! its numbers are consistent. Indexing and queries are included when
//! Neo4j is configured (as in CI), otherwise `--no-database` is used.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

fn neo4j_configured(workspace: &Path) -> bool {
    std::env::var("CODEATLAS_NEO4J_PASSWORD").is_ok() || workspace.join(".env").is_file()
}

#[test]
fn bench_reports_every_measurement() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let database = neo4j_configured(&workspace);
    let tmp = tempfile::tempdir().unwrap();
    let output = tmp.path().join("bench.json");
    let mut command = Command::new(env!("CARGO_BIN_EXE_codeatlas"));
    command
        .args([
            "bench",
            "--runs",
            "2",
            "--targets",
            "4",
            "--query-runs",
            "1",
        ])
        .arg("--test-truth")
        .arg(workspace.join("fixtures/change-impact/expected-tests.json"))
        .arg("-o")
        .arg(&output)
        .arg(workspace.join("fixtures/change-impact"))
        .current_dir(&workspace)
        .env("RUST_LOG", "warn");
    if !database {
        command.arg("--no-database");
    }
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let r: Value = serde_json::from_str(&std::fs::read_to_string(&output).unwrap()).unwrap();

    assert_eq!(r["schema"], 1);
    assert_eq!(r["repository"]["name"], "change-impact");
    let corpus = &r["corpus"];
    assert_eq!(corpus["rust_files"], 7);
    assert_eq!(corpus["rust_loc"], 85);
    assert_eq!(corpus["relationships"]["calls"], 11);
    assert_eq!(corpus["resolution_rate"], 1.0);

    let analysis = &r["analysis"];
    assert_eq!(analysis["samples"].as_array().unwrap().len(), 2);
    let total = &analysis["total_ms"];
    assert!(total["min"].as_f64().unwrap() <= total["median"].as_f64().unwrap());
    assert!(total["median"].as_f64().unwrap() <= total["max"].as_f64().unwrap());
    if cfg!(unix) {
        assert!(analysis["peak_rss_mb"]["median"].as_f64().unwrap() > 1.0);
    }

    // Test impact against the recorded ground truth.
    assert_eq!(r["test_impact"]["resolved_only"]["recall"], 1.0);

    if database {
        let indexing = &r["indexing"];
        assert_eq!(indexing["full"]["samples"][0]["mode"], "full");
        assert_eq!(indexing["unchanged"]["samples"][0]["mode"], "incremental");
        assert_eq!(indexing["unchanged"]["samples"][0]["files_parsed"], 0);
        // The edit inserts a line at the top of one file: that file only.
        assert_eq!(indexing["one_file"]["samples"][0]["files_parsed"], 1);
        assert!(corpus["stored_nodes"].as_u64().unwrap() > 0);
        let queries = r["queries"].as_array().unwrap();
        assert!(queries.iter().any(|q| q["name"] == "callers, depth 3"));
        assert!(queries
            .iter()
            .all(|q| q["latency_ms"]["n"].as_u64().unwrap() >= 1));
    } else {
        assert!(r["indexing"].is_null());
    }
}
