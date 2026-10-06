//! Re-records the ground truth of the fixtures with tests using
//! `codeatlas probe-tests` and requires it to equal the committed
//! `expected-tests.json` files. Builds and runs the fixture's tests, so it only
//! runs with `CODEATLAS_RUN_PROBES=1` (CI sets it).

use std::path::Path;
use std::process::Command;

use serde_json::Value;

#[test]
fn committed_ground_truth_is_reproducible() {
    if std::env::var("CODEATLAS_RUN_PROBES").as_deref() != Ok("1") {
        eprintln!("skipping: CODEATLAS_RUN_PROBES is not 1");
        return;
    }
    for name in ["test-impact", "change-impact", "simple-repo"] {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name);
        let tmp = tempfile::tempdir().unwrap();
        let output = tmp.path().join("truth.json");
        let status = Command::new(env!("CARGO_BIN_EXE_codeatlas"))
            .args(["probe-tests", "-n", "1000", "-o"])
            .arg(&output)
            .arg(&fixture)
            .env("RUST_LOG", "warn")
            .status()
            .unwrap();
        assert!(status.success(), "{name}");
        let read = |path: &Path| -> Value {
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
        };
        let (recorded, committed) = (read(&output), read(&fixture.join("expected-tests.json")));
        // The toolchain and the enclosing repository's commit may differ.
        for field in [
            "source_hash",
            "method",
            "tests",
            "baseline_failures",
            "probes",
        ] {
            assert_eq!(
                recorded[field], committed[field],
                "{name}: `{field}` differs"
            );
        }
    }
}
