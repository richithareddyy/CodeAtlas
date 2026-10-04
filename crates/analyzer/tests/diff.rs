//! Git diff impact on the `pr-impact` fixture: `base/` is committed on
//! `main`, `head/` on `feature`, and the result is compared with the
//! hand-written `expected.json`. Requires the `git` executable.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use codeatlas_analyzer::diff::{analyze_diff, ChangeKind, DiffOptions, DiffReport, Side};
use codeatlas_analyzer::error::{AnalyzerError, GitError};
use codeatlas_analyzer::git::FileStatus;
use codeatlas_analyzer::graph::impact::DependencyKind;
use codeatlas_analyzer::RepoSource;
use serde::Deserialize;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pr-impact")
}

#[derive(Deserialize)]
struct Expected {
    files: ExpectedFiles,
    symbols: ExpectedSymbols,
    downstream: BTreeMap<String, u32>,
    tests: Vec<String>,
    modules: Vec<String>,
}

#[derive(Deserialize)]
struct ExpectedFiles {
    added: Vec<String>,
    removed: Vec<String>,
    modified: Vec<String>,
    renamed: Vec<(String, String)>,
}

#[derive(Deserialize)]
struct ExpectedSymbols {
    modified: Vec<String>,
    signature_changed: Vec<String>,
    added: Vec<String>,
    removed: Vec<String>,
    moved: Vec<(String, String)>,
    cosmetic: Vec<String>,
}

fn expected() -> Expected {
    serde_json::from_str(&fs::read_to_string(fixture().join("expected.json")).unwrap()).unwrap()
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
        .status()
        .expect("git must be installed for these tests");
    assert!(status.success(), "git {args:?} failed");
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

/// Replaces the project files in `dir` (keeping `.git`) with `snapshot`.
fn replace_with(dir: &Path, snapshot: &str) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|n| n == ".git") {
            continue;
        }
        if path.is_dir() {
            fs::remove_dir_all(path).unwrap();
        } else {
            fs::remove_file(path).unwrap();
        }
    }
    copy_dir(&fixture().join(snapshot), dir);
}

/// A repository with `base/` on `main` and `head/` on `feature`, checked
/// out at `main`. With `nested`, the project lives in a subdirectory of
/// the repository. Returns the project directory.
fn build_repo(tmp: &Path, nested: bool) -> PathBuf {
    let top = tmp.join("shop");
    let project = if nested {
        top.join("services/pr-shop")
    } else {
        top.clone()
    };
    fs::create_dir_all(&project).unwrap();
    git(&top, &["init", "--quiet", "--initial-branch=main"]);
    if nested {
        fs::write(top.join("README.md"), "monorepo\n").unwrap();
    }
    copy_dir(&fixture().join("base"), &project);
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "--quiet", "-m", "base"]);
    git(&top, &["checkout", "--quiet", "-b", "feature"]);
    replace_with(&project, "head");
    if nested {
        // A change outside the project must not show up.
        fs::write(top.join("README.md"), "monorepo, edited\n").unwrap();
    }
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "--quiet", "-m", "head"]);
    git(&top, &["checkout", "--quiet", "main"]);
    project
}

fn diff(project: &Path, base: &str, head: Option<&str>) -> DiffReport {
    analyze_diff(
        &RepoSource::Local(project.to_path_buf()),
        base,
        head,
        &DiffOptions::default(),
    )
    .unwrap()
}

fn ids(report: &DiffReport, kind: ChangeKind) -> Vec<String> {
    let mut ids: Vec<String> = report
        .symbols
        .iter()
        .filter(|c| c.change == kind)
        .map(|c| c.symbol.id.to_string())
        .collect();
    ids.sort();
    ids
}

fn files(report: &DiffReport, status: FileStatus) -> Vec<String> {
    report
        .files
        .iter()
        .filter(|f| f.status == status)
        .map(|f| f.path.clone())
        .collect()
}

/// Symbol changes as comparable text, evidence lines included.
fn render_symbols(report: &DiffReport) -> Vec<String> {
    report
        .symbols
        .iter()
        .map(|c| {
            let lines: Vec<String> = c
                .lines
                .iter()
                .map(|l| format!("{}-{}", l.start, l.end))
                .collect();
            format!("{:?} {} {}", c.change, c.symbol.id, lines.join(","))
        })
        .collect()
}

fn check_symbols_and_impact(report: &DiffReport, e: &Expected) {
    let s = &e.symbols;
    assert_eq!(ids(report, ChangeKind::Modified), s.modified);
    assert_eq!(ids(report, ChangeKind::Added), s.added);
    assert_eq!(ids(report, ChangeKind::Removed), s.removed);
    let mut moved: Vec<(String, String)> = report
        .symbols
        .iter()
        .filter(|c| c.change == ChangeKind::Moved)
        .map(|c| {
            let from = c.previous.as_ref().unwrap().id.to_string();
            (from, c.symbol.id.to_string())
        })
        .collect();
    moved.sort();
    assert_eq!(moved, s.moved);
    let signatures: Vec<String> = report
        .symbols
        .iter()
        .filter(|c| c.signature.is_some())
        .map(|c| c.symbol.id.to_string())
        .collect();
    assert_eq!(signatures, s.signature_changed);
    let cosmetic: Vec<String> = report.cosmetic.iter().map(|c| c.id.to_string()).collect();
    assert_eq!(cosmetic, s.cosmetic);

    let downstream: BTreeMap<String, u32> = report
        .impact
        .downstream
        .iter()
        .map(|d| (d.affected.symbol.id.to_string(), d.affected.depth))
        .collect();
    assert_eq!(downstream, e.downstream);
    assert!(report
        .impact
        .downstream
        .iter()
        .all(|d| d.revision == Side::Head));
    let tests: Vec<String> = report.impact.tests.iter().map(|t| t.to_string()).collect();
    assert_eq!(tests, e.tests);
    let mut modules: Vec<String> = report
        .impact
        .modules
        .iter()
        .map(|m| m.name.clone())
        .collect();
    modules.sort();
    assert_eq!(modules, e.modules);
}

#[test]
fn diff_between_branches_matches_ground_truth() {
    let tmp = tempfile::tempdir().unwrap();
    let project = build_repo(tmp.path(), false);
    let report = diff(&project, "main", Some("feature"));
    let e = expected();

    assert_eq!(files(&report, FileStatus::Added), e.files.added);
    assert_eq!(files(&report, FileStatus::Removed), e.files.removed);
    assert_eq!(files(&report, FileStatus::Modified), e.files.modified);
    let renamed: Vec<(String, String)> = report
        .files
        .iter()
        .filter(|f| f.status == FileStatus::Renamed)
        .map(|f| (f.old_path.clone().unwrap(), f.path.clone()))
        .collect();
    assert_eq!(renamed, e.files.renamed);
    check_symbols_and_impact(&report, &e);

    // Evidence for a modified symbol: the changed lines and the signatures.
    let authorize = report
        .symbols
        .iter()
        .find(|c| c.symbol.id.as_str().ends_with("PaymentService::authorize"))
        .unwrap();
    let lines: Vec<(u32, u32)> = authorize.lines.iter().map(|l| (l.start, l.end)).collect();
    assert_eq!(lines, [(13, 14)]);
    let signature = authorize.signature.as_ref().unwrap();
    assert_eq!(
        signature.before,
        "pub fn authorize(&self, cents: u64) -> Result<String, String>"
    );
    assert_eq!(
        signature.after,
        "pub fn authorize(&self, amount: u64, currency: &str) -> Result<String, String>"
    );

    // Evidence for a downstream symbol: the call chain with lines.
    let print_test = report
        .impact
        .downstream
        .iter()
        .find(|d| d.affected.symbol.id.as_str() == "fn:shop_tests::test_print_invoice")
        .unwrap();
    let chain: Vec<String> = print_test
        .affected
        .path
        .iter()
        .map(|step| {
            assert_eq!(step.kind, DependencyKind::Calls);
            format!(
                "{} -> {} {}:{:?}",
                step.source, step.target, step.file, step.lines
            )
        })
        .collect();
    assert_eq!(
        chain,
        [
            "fn:shop_tests::test_print_invoice -> fn:pr_shop::invoice::print_invoice tests/shop_tests.rs:[27]",
            "fn:pr_shop::invoice::print_invoice -> fn:pr_shop::invoice::invoice_total src/invoice.rs:[8]",
        ]
    );

    let sum = &report.summary;
    assert_eq!(
        (
            sum.files_changed,
            sum.functions_modified,
            sum.functions_added,
            sum.functions_removed,
            sum.tests_modified,
            sum.signatures_changed,
            sum.moved,
            sum.cosmetic
        ),
        (9, 4, 1, 1, 1, 1, 2, 1)
    );
    assert_eq!(
        (
            sum.downstream_symbols,
            sum.affected_modules,
            sum.affected_files,
            sum.affected_tests
        ),
        (5, 3, 3, 3)
    );
    assert_eq!(report.base.label, "main");
    assert_eq!(report.head.label, "feature");
    assert_eq!(report.head.sha.as_ref().map(String::len), Some(40));
    assert_eq!(report.repository, "shop");
}

#[test]
fn working_tree_changes_give_the_same_symbol_diff() {
    let tmp = tempfile::tempdir().unwrap();
    let project = build_repo(tmp.path(), false);
    let committed = diff(&project, "main", Some("feature"));

    // Same edits, uncommitted on `main`; new files are untracked.
    replace_with(&project, "head");
    let report = diff(&project, "main", None);
    assert_eq!(report.head.label, "working tree");
    assert_eq!(report.head.sha, None);
    assert_eq!(render_symbols(&report), render_symbols(&committed));
    check_symbols_and_impact(&report, &expected());
    // Without staging, Git sees the rename as a deletion and a new file.
    assert_eq!(
        files(&report, FileStatus::Added),
        ["src/currency.rs", "src/reporting.rs"]
    );
    assert_eq!(
        files(&report, FileStatus::Removed),
        ["src/legacy.rs", "src/reports.rs"]
    );
}

#[test]
fn projects_in_a_subdirectory_are_diffed_relative_to_it() {
    let tmp = tempfile::tempdir().unwrap();
    let project = build_repo(tmp.path(), true);
    let report = diff(&project, "main", Some("feature"));
    let e = expected();
    assert_eq!(files(&report, FileStatus::Modified), e.files.modified);
    check_symbols_and_impact(&report, &e);
}

#[test]
fn identical_revisions_and_bad_revisions() {
    let tmp = tempfile::tempdir().unwrap();
    let project = build_repo(tmp.path(), false);
    let same = diff(&project, "feature", Some("feature"));
    assert!(same.files.is_empty() && same.symbols.is_empty());
    assert!(same.impact.downstream.is_empty());

    let error = |base: &str| {
        analyze_diff(
            &RepoSource::Local(project.clone()),
            base,
            None,
            &DiffOptions::default(),
        )
        .unwrap_err()
    };
    assert!(matches!(
        error("--output=/tmp/x"),
        AnalyzerError::Git(GitError::InvalidRevision(_))
    ));
    assert!(matches!(
        error("no-such-branch"),
        AnalyzerError::Git(GitError::UnknownRevision(_))
    ));
}

#[test]
fn directories_outside_git_are_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    copy_dir(&fixture().join("base"), tmp.path());
    let err = analyze_diff(
        &RepoSource::Local(tmp.path().to_path_buf()),
        "main",
        None,
        &DiffOptions::default(),
    )
    .unwrap_err();
    assert!(
        matches!(err, AnalyzerError::Git(GitError::NotARepository { .. })),
        "{err}"
    );
}
