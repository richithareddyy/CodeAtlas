//! Thin, typed wrapper over the `git` command-line tool.
//!
//! The CLI is used instead of libgit2 so that cloning honours the user's
//! credential helpers and SSH configuration, and so behaviour matches what the
//! user sees in their own terminal.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::GitError;

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

    let output = cmd.output().map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => GitError::NotInstalled,
        _ => GitError::Spawn(err),
    })?;

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
    fn leaves_scp_style_urls_untouched() {
        assert_eq!(
            strip_credentials("git@github.com:org/repo.git"),
            "git@github.com:org/repo.git"
        );
    }
}
