//! Ingestion against real Git repositories created in temporary directories.
//! Requires the `git` executable.

use std::fs;
use std::path::Path;
use std::process::Command;

use codeatlas_analyzer::ingest::{ingest, IngestOptions, RepoSource};

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

fn init_repo(dir: &Path) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn a() {}\n\npub fn b() {}\n").unwrap();
    fs::write(dir.join("notes.py"), "print('x')\n").unwrap();
    git(dir, &["init", "--quiet", "--initial-branch=trunk"]);
    git(dir, &["add", "."]);
    git(dir, &["commit", "--quiet", "-m", "init"]);
}

fn head(dir: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn collects_git_metadata_for_local_repositories() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("demo-repo");
    init_repo(&repo);

    let ingested = ingest(&RepoSource::Local(repo.clone()), &IngestOptions::default()).unwrap();
    let info = &ingested.info;
    assert_eq!(info.name, "demo-repo");
    assert_eq!(info.branch.as_deref(), Some("trunk"));
    assert_eq!(info.head_sha.as_deref(), Some(head(&repo).as_str()));
    assert_eq!(info.source_files, 2);
    assert_eq!(info.loc, 3);
    let languages: Vec<_> = info
        .languages
        .iter()
        .map(|l| (format!("{:?}", l.language), l.files))
        .collect();
    assert_eq!(
        languages,
        vec![("Rust".to_string(), 1), ("Python".to_string(), 1)]
    );
}

#[test]
fn reports_detached_head_and_non_git_directories() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("r");
    init_repo(&repo);
    let sha = head(&repo);
    git(&repo, &["checkout", "--quiet", "--detach", &sha]);

    let info = ingest(&RepoSource::Local(repo), &IngestOptions::default())
        .unwrap()
        .info;
    assert_eq!(info.branch, None);
    assert_eq!(info.head_sha, Some(sha));

    let plain = tmp.path().join("plain");
    fs::create_dir_all(&plain).unwrap();
    fs::write(plain.join("x.rs"), "fn x() {}\n").unwrap();
    let info = ingest(&RepoSource::Local(plain), &IngestOptions::default())
        .unwrap()
        .info;
    assert_eq!(
        (info.branch, info.head_sha, info.origin_url),
        (None, None, None)
    );
}

#[test]
fn clones_remote_url_then_updates_existing_clone() {
    let tmp = tempfile::tempdir().unwrap();
    let upstream = tmp.path().join("upstream");
    init_repo(&upstream);
    let url = format!("file://{}", upstream.display());
    let options = IngestOptions {
        clone_dir: tmp.path().join("clones"),
        ..Default::default()
    };

    let first = ingest(&RepoSource::Remote(url.clone()), &options).unwrap();
    assert!(first
        .info
        .root
        .starts_with(fs::canonicalize(&options.clone_dir).unwrap()));
    assert_eq!(first.info.name, "upstream");
    assert_eq!(first.info.origin_url.as_deref(), Some(url.as_str()));
    assert_eq!(
        first.info.head_sha.as_deref(),
        Some(head(&upstream).as_str())
    );

    fs::write(upstream.join("src/new.rs"), "pub fn c() {}\n").unwrap();
    git(&upstream, &["add", "."]);
    git(&upstream, &["commit", "--quiet", "-m", "add new"]);

    let second = ingest(&RepoSource::Remote(url), &options).unwrap();
    assert_eq!(second.info.root, first.info.root);
    assert_eq!(
        second.info.head_sha.as_deref(),
        Some(head(&upstream).as_str())
    );
    assert_eq!(second.info.source_files, 3);
}

#[test]
fn reports_clone_failures() {
    let tmp = tempfile::tempdir().unwrap();
    let options = IngestOptions {
        clone_dir: tmp.path().join("clones"),
        ..Default::default()
    };
    let missing = format!("file://{}/nope", tmp.path().display());
    let err = ingest(&RepoSource::Remote(missing), &options).unwrap_err();
    assert!(err.to_string().contains("git clone"), "{err}");
}

#[test]
fn projects_inside_a_repository_with_an_origin_get_their_own_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("monorepo");
    init_repo(&repo);
    let project = repo.join("services/billing");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"billing\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(project.join("src/lib.rs"), "pub fn bill() {}\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "billing"]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://example.com/org/monorepo.git",
        ],
    );

    let top = ingest(&RepoSource::Local(repo.clone()), &IngestOptions::default()).unwrap();
    let sub = ingest(&RepoSource::Local(project), &IngestOptions::default()).unwrap();
    assert_eq!(top.info.name, "monorepo");
    assert_eq!(sub.info.name, "billing");
    assert_ne!(top.info.id, sub.info.id);
    // Both still report the repository's origin and commit.
    assert_eq!(sub.info.origin_url, top.info.origin_url);
    assert_eq!(sub.info.head_sha, top.info.head_sha);
    // The ID does not depend on where the clone lives.
    let elsewhere = tmp.path().join("copy");
    git(
        tmp.path(),
        &["clone", "--quiet", repo.to_str().unwrap(), "copy"],
    );
    git(
        &elsewhere,
        &[
            "remote",
            "set-url",
            "origin",
            "https://example.com/org/monorepo.git",
        ],
    );
    let copy = ingest(
        &RepoSource::Local(elsewhere.join("services/billing")),
        &IngestOptions::default(),
    )
    .unwrap();
    assert_eq!(copy.info.id, sub.info.id);
}
