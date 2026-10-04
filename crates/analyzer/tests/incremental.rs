//! Incremental analysis: an analysis that reuses a cache from an older
//! state of the repository must equal a full analysis of the new state,
//! while parsing only what changed.

use std::fs;
use std::path::{Path, PathBuf};

use codeatlas_analyzer::incremental::{AnalysisCache, ReuseStats};
use codeatlas_analyzer::ingest::{ingest, IngestOptions, RepoSource};
use codeatlas_analyzer::{analyze, analyze_with_cache, Analyzed, RepositoryAnalysis};
use serde_json::Value;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

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

/// Replaces the contents of `dir` with `snapshot`.
fn replace_with(dir: &Path, snapshot: &Path) {
    fs::remove_dir_all(dir).unwrap();
    copy_dir(snapshot, dir);
}

fn run(dir: &Path, cache: Option<&AnalysisCache>) -> Analyzed {
    let repo = ingest(
        &RepoSource::Local(dir.to_path_buf()),
        &IngestOptions::default(),
    )
    .unwrap();
    analyze_with_cache(&repo, cache).unwrap()
}

fn full(dir: &Path) -> RepositoryAnalysis {
    let repo = ingest(
        &RepoSource::Local(dir.to_path_buf()),
        &IngestOptions::default(),
    )
    .unwrap();
    analyze(&repo).unwrap()
}

/// The analysis as JSON, without timings and timestamps.
fn normalized(a: &RepositoryAnalysis) -> Value {
    let mut value = serde_json::to_value(a).unwrap();
    for field in ["parse_ms", "resolve_ms", "total_ms"] {
        value["stats"][field] = Value::Null;
    }
    value["repository"]["analyzed_at"] = Value::Null;
    value
}

fn assert_same(incremental: &RepositoryAnalysis, full: &RepositoryAnalysis) {
    let (a, b) = (normalized(incremental), normalized(full));
    if a != b {
        for key in ["crates", "files", "resolution", "stats"] {
            assert_eq!(
                a[key], b[key],
                "incremental and full analyses differ in `{key}`"
            );
        }
        panic!("incremental and full analyses differ");
    }
}

fn reuse(parsed: u32, extracted: u32, reused: u32) -> (u32, u32, u32) {
    (parsed, extracted, reused)
}

fn counts(r: &ReuseStats) -> (u32, u32, u32) {
    (r.parsed, r.extracted, r.reused)
}

#[test]
fn unchanged_repositories_are_not_parsed_again() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("change-impact");
    copy_dir(&fixture("change-impact"), &dir);

    let first = run(&dir, None);
    assert_eq!(counts(&first.reuse), reuse(7, 7, 0));
    let second = run(&dir, Some(&first.cache));
    assert_eq!(counts(&second.reuse), reuse(0, 0, 7));
    assert_eq!(
        (
            second.reuse.changed,
            second.reuse.added,
            second.reuse.removed
        ),
        (0, 0, 0)
    );
    assert_same(&second.analysis, &first.analysis);
    assert_eq!(second.cache, first.cache);
}

#[test]
fn edits_additions_removals_and_renames_match_a_full_analysis() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("pr-shop");
    copy_dir(&fixture("pr-impact/base"), &dir);
    let base = run(&dir, None);

    replace_with(&dir, &fixture("pr-impact/head"));
    let head = run(&dir, Some(&base.cache));
    assert_same(&head.analysis, &full(&dir));
    // 9 files: 6 edited, 2 new (currency.rs, reporting.rs), gateway.rs
    // unchanged; legacy.rs and reports.rs are gone.
    let r = &head.reuse;
    assert_eq!((r.files, r.changed, r.added, r.removed), (9, 6, 2, 2));
    assert_eq!(counts(r), reuse(8, 8, 1));
}

#[test]
fn files_whose_module_path_changes_are_extracted_again() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("change-impact");
    copy_dir(&fixture("change-impact"), &dir);
    let before = run(&dir, None);

    // reports.rs keeps its content but becomes module `summary`.
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
    let after = run(&dir, Some(&before.cache));
    assert_same(&after.analysis, &full(&dir));
    // Only lib.rs is parsed for declarations; reports.rs is extracted again
    // because its module path changed.
    assert_eq!(counts(&after.reuse), reuse(1, 2, 5));
    assert!(after
        .analysis
        .files
        .iter()
        .flat_map(|f| &f.symbols)
        .any(|s| s.id.as_str() == "fn:change_impact::summary::daily_total"));
}

#[test]
fn renaming_the_package_reextracts_its_files_without_parsing_declarations() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("change-impact");
    copy_dir(&fixture("change-impact"), &dir);
    let before = run(&dir, None);

    let manifest = dir.join("Cargo.toml");
    let text = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        text.replace("name = \"change-impact\"", "name = \"shop-core\""),
    )
    .unwrap();
    let after = run(&dir, Some(&before.cache));
    assert_same(&after.analysis, &full(&dir));
    // The six library files get a new crate name; the integration test is
    // its own crate and is reused (its `use change_impact::...` paths are
    // resolved again and now fail, as in a full analysis).
    assert_eq!(counts(&after.reuse), reuse(0, 6, 1));
}

#[test]
fn caches_from_another_analyzer_version_are_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("simple-repo");
    copy_dir(&fixture("simple-repo"), &dir);
    let mut cache = run(&dir, None).cache;
    cache.version += 1;
    assert!(!cache.is_current());
    let again = run(&dir, Some(&cache));
    assert_eq!(again.reuse.reused, 0);
    assert_eq!(again.reuse.parsed, again.reuse.files);
}

#[test]
fn rebuild_reproduces_the_analysis_without_parsing() {
    for name in [
        "simple-repo",
        "cross-module",
        "duplicate-symbols",
        "change-impact",
    ] {
        let first = run(&fixture(name), None);
        let rebuilt = first.cache.rebuild(&first.analysis.repository);
        assert_same(&rebuilt, &first.analysis);
    }
}

#[test]
fn caches_round_trip_through_json() {
    let first = run(&fixture("duplicate-symbols"), None);
    let json = serde_json::to_string(&first.cache).unwrap();
    let cache: AnalysisCache = serde_json::from_str(&json).unwrap();
    assert_eq!(cache, first.cache);
}
