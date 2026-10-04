//! Thin, typed wrapper over the `git` command-line tool.
//!
//! The CLI is used instead of libgit2 so that cloning honours the user's
//! credential helpers and SSH configuration, and so behaviour matches what the
//! user sees in their own terminal.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::error::GitError;

/// Options that make diff output independent of the user's Git
/// configuration (external diff drivers, colour, prefixes, rename limits).
const DIFF_FLAGS: &[&str] = &[
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--find-renames",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Removed,
    Modified,
    Renamed,
}

/// A file changed between two revisions. Paths are relative to the
/// analysed directory and use `/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub status: FileStatus,
    pub path: String,
    /// Previous path of a renamed file.
    pub old_path: Option<String>,
}

/// One `@@ -old_start,old_lines +new_start,new_lines @@` hunk of a
/// zero-context diff. A pure deletion has `new_lines == 0` and
/// `new_start` is the line after which the deleted lines stood.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    Branch,
    RemoteBranch,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRef {
    /// Short name (`main`, `origin/feature`, `v1.2.0`).
    pub name: String,
    pub kind: RefKind,
    /// Commit the ref points to (tags are peeled).
    pub sha: String,
    /// Committer date, ISO 8601.
    pub date: String,
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitInfo {
    pub sha: String,
    pub date: String,
    pub subject: String,
}

#[derive(Debug, Clone)]
pub struct Git {
    root: PathBuf,
}

impl Git {
    /// Opens the repository containing `path`, or returns `None` if `path`
    /// is not inside a Git work tree.
    pub fn discover(path: &Path) -> Result<Option<Self>, GitError> {
        match run(Some(path), &["rev-parse", "--show-toplevel"]) {
            Ok(out) => Ok(Some(Self {
                root: PathBuf::from(out.trim()),
            })),
            Err(GitError::CommandFailed { .. }) => Ok(None),
            Err(err) => Err(err),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Commit SHA of `HEAD`, or `None` for a repository without commits.
    pub fn head_sha(&self) -> Result<Option<String>, GitError> {
        self.optional(&["rev-parse", "--verify", "--quiet", "HEAD"])
    }

    /// Current branch name, or `None` when `HEAD` is detached.
    pub fn current_branch(&self) -> Result<Option<String>, GitError> {
        self.optional(&["symbolic-ref", "--short", "--quiet", "HEAD"])
    }

    /// URL of the `origin` remote with any embedded credentials removed.
    pub fn origin_url(&self) -> Result<Option<String>, GitError> {
        Ok(self
            .optional(&["config", "--get", "remote.origin.url"])?
            .map(|url| strip_credentials(&url)))
    }

    /// Clones `url` into `dest`. A blobless partial clone keeps full history
    /// (needed for diff analysis) while deferring blob downloads.
    pub fn clone(url: &str, dest: &Path) -> Result<Self, GitError> {
        let dest_str = dest.to_string_lossy();
        run(
            None,
            &[
                "clone",
                "--filter=blob:none",
                "--quiet",
                "--",
                url,
                &dest_str,
            ],
        )?;
        Ok(Self {
            root: dest.to_path_buf(),
        })
    }

    pub fn pull_fast_forward(&self) -> Result<(), GitError> {
        self.run(&["pull", "--ff-only", "--quiet"]).map(drop)
    }

    /// Path of `dir` relative to the top of the work tree, with a trailing
    /// `/` (empty for the top itself).
    pub fn prefix_of(dir: &Path) -> Result<String, GitError> {
        match run(Some(dir), &["rev-parse", "--show-prefix"]) {
            Ok(out) => Ok(out.trim().to_string()),
            Err(GitError::CommandFailed { .. }) => Err(GitError::NotARepository {
                path: dir.to_path_buf(),
            }),
            Err(err) => Err(err),
        }
    }

    /// Resolves a branch, tag, SHA or expression such as `HEAD~2` to a
    /// commit SHA. Anything that could be read as an option is rejected.
    pub fn resolve_commit(&self, revision: &str) -> Result<String, GitError> {
        let revision = revision.trim();
        if revision.is_empty() || revision.starts_with('-') || revision.contains('\0') {
            return Err(GitError::InvalidRevision(revision.to_string()));
        }
        let spec = format!("{revision}^{{commit}}");
        self.optional(&[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &spec,
        ])?
        .ok_or_else(|| GitError::UnknownRevision(revision.to_string()))
    }

    /// Files changed between `base` and `head` (or the work tree when
    /// `head` is `None`), limited to `prefix` and relative to it. Untracked
    /// files are not included; see [`Git::untracked_files`].
    pub fn changed_files(
        &self,
        base: &str,
        head: Option<&str>,
        prefix: &str,
    ) -> Result<Vec<FileChange>, GitError> {
        let mut args = vec!["diff", "--name-status", "-z"];
        args.extend_from_slice(DIFF_FLAGS);
        let relative = relative_flag(prefix);
        args.push(&relative);
        args.push(base);
        args.extend(head);
        let out = self.run(&args)?;
        Ok(parse_name_status(&out))
    }

    /// Untracked, non-ignored files under `prefix`, relative to it.
    pub fn untracked_files(&self, prefix: &str) -> Result<Vec<String>, GitError> {
        let dir = self.root.join(prefix);
        let out = run(
            Some(&dir),
            &[
                "ls-files",
                "--others",
                "--exclude-standard",
                "-z",
                "--",
                ".",
            ],
        )?;
        Ok(out
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Zero-context hunks of changed `.rs` files, keyed by path (the new
    /// path, or the old one for deleted files).
    pub fn rust_hunks(
        &self,
        base: &str,
        head: Option<&str>,
        prefix: &str,
    ) -> Result<BTreeMap<String, Vec<Hunk>>, GitError> {
        let mut args = vec!["-c", "core.quotePath=false", "diff", "--unified=0"];
        args.extend_from_slice(DIFF_FLAGS);
        let relative = relative_flag(prefix);
        args.push(&relative);
        args.push(base);
        args.extend(head);
        args.extend(["--", "*.rs"]);
        Ok(parse_hunks(&self.run(&args)?))
    }

    /// Writes the files of `commit` under `prefix` into `dest`, which must
    /// exist. Uses `git archive`, so the repository and its work tree are
    /// not touched.
    pub fn export(&self, commit: &str, prefix: &str, dest: &Path) -> Result<(), GitError> {
        let tree = match prefix.trim_end_matches('/') {
            "" => commit.to_string(),
            dir => format!("{commit}:{dir}"),
        };
        let failed = |message: String| GitError::Export {
            revision: commit.to_string(),
            message,
        };
        let mut archive = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(["archive", "--format=tar", &tree])
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(spawn_error)?;
        let stdout = archive
            .stdout
            .take()
            .ok_or_else(|| failed("no archive output".into()))?;
        let tar = Command::new("tar")
            .arg("-x")
            .arg("-f")
            .arg("-")
            .arg("-C")
            .arg(dest)
            .stdin(stdout)
            .output()
            .map_err(|err| failed(format!("failed to run tar: {err}")))?;
        let archive = archive.wait_with_output().map_err(GitError::Spawn)?;
        if !archive.status.success() {
            return Err(failed(
                String::from_utf8_lossy(&archive.stderr).trim().to_string(),
            ));
        }
        if !tar.status.success() {
            return Err(failed(
                String::from_utf8_lossy(&tar.stderr).trim().to_string(),
            ));
        }
        Ok(())
    }

    /// Local branches, remote-tracking branches and tags, most recently
    /// committed first.
    pub fn refs(&self, limit: usize) -> Result<Vec<GitRef>, GitError> {
        let count = format!("--count={limit}");
        let out = self.run(&[
            "for-each-ref",
            "--sort=-creatordate",
            &count,
            "--format=%(refname)%00%(objectname)%00%(*objectname)%00%(creatordate:iso-strict)%00%(contents:subject)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ])?;
        Ok(out
            .lines()
            .filter_map(|line| {
                let mut fields = line.split('\0');
                let full = fields.next()?;
                let object = fields.next()?;
                let peeled = fields.next()?;
                let date = fields.next()?.to_string();
                let subject = fields.next().unwrap_or_default().to_string();
                let (kind, name) = if let Some(name) = full.strip_prefix("refs/heads/") {
                    (RefKind::Branch, name)
                } else if let Some(name) = full.strip_prefix("refs/remotes/") {
                    (RefKind::RemoteBranch, name)
                } else {
                    (RefKind::Tag, full.strip_prefix("refs/tags/")?)
                };
                // `origin/HEAD` is a symbolic ref to another branch.
                if kind == RefKind::RemoteBranch && name.ends_with("/HEAD") {
                    return None;
                }
                Some(GitRef {
                    name: name.to_string(),
                    kind,
                    sha: if peeled.is_empty() { object } else { peeled }.to_string(),
                    date,
                    subject,
                })
            })
            .collect())
    }

    /// The most recent commits reachable from `HEAD`.
    pub fn recent_commits(&self, limit: usize) -> Result<Vec<CommitInfo>, GitError> {
        let count = format!("--max-count={limit}");
        let out = match self.run(&["log", &count, "--format=%H%x00%cI%x00%s", "HEAD", "--"]) {
            Ok(out) => out,
            // No commits yet.
            Err(GitError::CommandFailed { .. }) => return Ok(Vec::new()),
            Err(err) => return Err(err),
        };
        Ok(out
            .lines()
            .filter_map(|line| {
                let mut fields = line.splitn(3, '\0');
                Some(CommitInfo {
                    sha: fields.next()?.to_string(),
                    date: fields.next()?.to_string(),
                    subject: fields.next().unwrap_or_default().to_string(),
                })
            })
            .collect())
    }

    fn run(&self, args: &[&str]) -> Result<String, GitError> {
        run(Some(&self.root), args)
    }

    /// Runs a command whose failure means "no value" rather than an error.
    fn optional(&self, args: &[&str]) -> Result<Option<String>, GitError> {
        match self.run(args) {
            Ok(out) => {
                let out = out.trim();
                Ok((!out.is_empty()).then(|| out.to_string()))
            }
            Err(GitError::CommandFailed { .. }) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

fn run(cwd: Option<&Path>, args: &[&str]) -> Result<String, GitError> {
    let mut cmd = Command::new("git");
    if let Some(dir) = cwd {
        cmd.arg("-C").arg(dir);
    }
    // Never block on an interactive credential prompt.
    cmd.args(args).env("GIT_TERMINAL_PROMPT", "0");

    let output = cmd.output().map_err(spawn_error)?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(GitError::CommandFailed {
            command: args.join(" "),
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

fn spawn_error(err: std::io::Error) -> GitError {
    match err.kind() {
        std::io::ErrorKind::NotFound => GitError::NotInstalled,
        _ => GitError::Spawn(err),
    }
}

fn relative_flag(prefix: &str) -> String {
    match prefix.trim_end_matches('/') {
        "" => "--relative".to_string(),
        dir => format!("--relative={dir}"),
    }
}

/// Parses `git diff --name-status -z`.
fn parse_name_status(out: &str) -> Vec<FileChange> {
    let mut fields = out.split('\0').filter(|f| !f.is_empty());
    let mut changes = Vec::new();
    while let Some(status) = fields.next() {
        let Some(path) = fields.next() else { break };
        let change = match status.as_bytes()[0] {
            b'A' | b'C' => FileChange {
                status: FileStatus::Added,
                // A copy has the source first; the new file is the second path.
                path: if status.starts_with('C') {
                    fields.next().unwrap_or(path)
                } else {
                    path
                }
                .to_string(),
                old_path: None,
            },
            b'D' => FileChange {
                status: FileStatus::Removed,
                path: path.to_string(),
                old_path: None,
            },
            b'R' => {
                let Some(new_path) = fields.next() else { break };
                FileChange {
                    status: FileStatus::Renamed,
                    path: new_path.to_string(),
                    old_path: Some(path.to_string()),
                }
            }
            // M, T (type change), U (unmerged)
            _ => FileChange {
                status: FileStatus::Modified,
                path: path.to_string(),
                old_path: None,
            },
        };
        changes.push(change);
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    changes
}

/// Parses the hunk headers of a `--unified=0` diff.
fn parse_hunks(out: &str) -> BTreeMap<String, Vec<Hunk>> {
    let mut hunks: BTreeMap<String, Vec<Hunk>> = BTreeMap::new();
    let mut old_path: Option<String> = None;
    let mut current: Option<String> = None;
    for line in out.lines() {
        if line.starts_with("diff --git ") {
            old_path = None;
            current = None;
        } else if let Some(path) = line.strip_prefix("--- ") {
            old_path = diff_path(path, "a/");
        } else if let Some(path) = line.strip_prefix("+++ ") {
            current = diff_path(path, "b/").or_else(|| old_path.clone());
        } else if let (Some(path), Some(header)) = (&current, line.strip_prefix("@@ -")) {
            if let Some(hunk) = parse_hunk_header(header) {
                hunks.entry(path.clone()).or_default().push(hunk);
            }
        }
    }
    hunks
}

/// `a/src/lib.rs` → `src/lib.rs`; `/dev/null` → `None`. Handles Git's
/// C-style quoting of unusual file names.
fn diff_path(raw: &str, side: &str) -> Option<String> {
    let raw = raw.trim_end_matches('\t');
    if raw == "/dev/null" {
        return None;
    }
    let unquoted = match raw.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        Some(inner) => {
            let mut out = String::with_capacity(inner.len());
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    match chars.next() {
                        Some('t') => out.push('\t'),
                        Some('n') => out.push('\n'),
                        Some(other) => out.push(other),
                        None => {}
                    }
                } else {
                    out.push(c);
                }
            }
            out
        }
        None => raw.to_string(),
    };
    Some(unquoted.strip_prefix(side).unwrap_or(&unquoted).to_string())
}

/// `12,3 +12,4 @@ fn x()` (after the leading `@@ -`).
fn parse_hunk_header(header: &str) -> Option<Hunk> {
    let (ranges, _) = header.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |text: &str| -> Option<(u32, u32)> {
        match text.split_once(',') {
            Some((start, len)) => Some((start.parse().ok()?, len.parse().ok()?)),
            None => Some((text.parse().ok()?, 1)),
        }
    };
    let (old_start, old_lines) = range(old)?;
    let (new_start, new_lines) = range(new)?;
    Some(Hunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
    })
}

/// Removes `user:password@` from URLs so tokens are never persisted.
pub fn strip_credentials(url: &str) -> String {
    if let Some(scheme_end) = url.find("://") {
        let rest = &url[scheme_end + 3..];
        let authority_end = rest.find('/').unwrap_or(rest.len());
        if let Some(at) = rest[..authority_end].rfind('@') {
            return format!("{}{}", &url[..scheme_end + 3], &rest[at + 1..]);
        }
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_credentials_from_https_urls() {
        assert_eq!(
            strip_credentials("https://user:ghp_secret@github.com/org/repo.git"),
            "https://github.com/org/repo.git"
        );
        assert_eq!(
            strip_credentials("https://github.com/org/repo.git"),
            "https://github.com/org/repo.git"
        );
    }

    #[test]
    fn parses_name_status_with_renames_and_copies() {
        let out = "M\0src/lib.rs\0R087\0src/old.rs\0src/new.rs\0A\0src/a.rs\0D\0src/gone.rs\0C100\0src/x.rs\0src/y.rs\0";
        let changes = parse_name_status(out);
        let render: Vec<String> = changes
            .iter()
            .map(|c| format!("{:?} {} {:?}", c.status, c.path, c.old_path))
            .collect();
        assert_eq!(
            render,
            [
                "Added src/a.rs None",
                "Removed src/gone.rs None",
                "Modified src/lib.rs None",
                "Renamed src/new.rs Some(\"src/old.rs\")",
                "Added src/y.rs None",
            ]
        );
    }

    #[test]
    fn parses_zero_context_hunks() {
        let out = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1..2 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -3 +3,2 @@ fn a() {
-x
+y
+z
@@ -10,2 +10,0 @@
diff --git a/src/gone.rs b/src/gone.rs
--- a/src/gone.rs
+++ /dev/null
@@ -1,4 +0,0 @@
diff --git \"a/src/sp ace\\t.rs\" \"b/src/sp ace\\t.rs\"
--- \"a/src/sp ace\\t.rs\"
+++ \"b/src/sp ace\\t.rs\"
@@ -0,0 +1 @@
";
        let hunks = parse_hunks(out);
        let h = |o, ol, n, nl| Hunk {
            old_start: o,
            old_lines: ol,
            new_start: n,
            new_lines: nl,
        };
        assert_eq!(hunks["src/lib.rs"], [h(3, 1, 3, 2), h(10, 2, 10, 0)]);
        assert_eq!(hunks["src/gone.rs"], [h(1, 4, 0, 0)]);
        assert_eq!(hunks["src/sp ace\t.rs"], [h(0, 0, 1, 1)]);
    }

    #[test]
    fn leaves_scp_style_urls_untouched() {
        assert_eq!(
            strip_credentials("git@github.com:org/repo.git"),
            "git@github.com:org/repo.git"
        );
    }
}
