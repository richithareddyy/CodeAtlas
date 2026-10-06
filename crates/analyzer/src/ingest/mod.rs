//! Repository ingestion: resolves a local path or Git URL into a working
//! tree, discovers source files and collects repository metadata.
//!
//! Ingestion knows nothing about parsing; it hands a list of files to the
//! analysis pipeline.

pub mod discovery;
pub mod language;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub use discovery::{DiscoveredFile, Discovery, DiscoveryOptions, SkippedFiles};
pub use language::Language;

use crate::error::{AnalyzerError, Result};
use crate::git::{self, Git};

/// Where a repository comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoSource {
    Local(PathBuf),
    Remote(String),
}

impl RepoSource {
    /// Interprets user input: anything that looks like a Git URL is remote,
    /// everything else is treated as a filesystem path.
    pub fn parse(input: &str) -> Self {
        let looks_remote = input.contains("://")
            || (input.starts_with("git@") && input.contains(':'))
            || (input.ends_with(".git") && !Path::new(input).exists());
        if looks_remote {
            RepoSource::Remote(input.to_string())
        } else {
            RepoSource::Local(PathBuf::from(input))
        }
    }
}

#[derive(Debug, Clone)]
pub struct IngestOptions {
    /// Directory where remote repositories are cloned.
    pub clone_dir: PathBuf,
    pub discovery: DiscoveryOptions,
}

impl Default for IngestOptions {
    fn default() -> Self {
        Self {
            clone_dir: default_clone_dir(),
            discovery: DiscoveryOptions::default(),
        }
    }
}

/// `$XDG_CACHE_HOME/codeatlas/repos`, falling back to `~/.cache` and then
/// the system temp directory.
pub fn default_clone_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("codeatlas").join("repos")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageStats {
    pub language: Language,
    pub files: u32,
    pub loc: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryInfo {
    /// Stable identifier derived from the origin URL, or the canonical path
    /// for repositories without a remote.
    pub id: String,
    pub name: String,
    pub root: PathBuf,
    pub origin_url: Option<String>,
    pub branch: Option<String>,
    pub head_sha: Option<String>,
    /// Sorted by LOC, largest first.
    pub languages: Vec<LanguageStats>,
    pub source_files: u32,
    pub loc: u64,
    pub skipped: SkippedFiles,
    pub analyzed_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct IngestedRepository {
    pub info: RepositoryInfo,
    pub discovery: Discovery,
}

impl IngestedRepository {
    /// Files the analyzer can parse.
    pub fn analyzable_files(&self) -> impl Iterator<Item = &DiscoveredFile> {
        self.discovery
            .files
            .iter()
            .filter(|f| f.language.is_analyzable())
    }
}

pub fn ingest(source: &RepoSource, options: &IngestOptions) -> Result<IngestedRepository> {
    let (root, origin_override) = match source {
        RepoSource::Local(path) => (canonical_dir(path)?, None),
        RepoSource::Remote(url) => (clone_or_update(url, &options.clone_dir)?, Some(url)),
    };

    let git = Git::discover(&root)?;
    let (branch, head_sha, origin_url) = match &git {
        Some(git) => (git.current_branch()?, git.head_sha()?, git.origin_url()?),
        None => (None, None, None),
    };
    let origin_url = origin_override
        .map(|url| git::strip_credentials(url))
        .or(origin_url);
    // A project below the top of its Git repository (one package of a
    // monorepo, a fixture) is a repository of its own for CodeAtlas: named
    // after its directory, identified by the origin plus its path.
    let prefix = match &git {
        Some(_) => Git::prefix_of(&root)?.trim_end_matches('/').to_string(),
        None => String::new(),
    };

    let discovery = discovery::discover(&root, &options.discovery)?;
    tracing::info!(
        root = %root.display(),
        files = discovery.files.len(),
        "discovered source files"
    );

    let name = if prefix.is_empty() {
        repository_name(origin_url.as_deref(), &root)
    } else {
        repository_name(None, &root)
    };
    let id_basis = match &origin_url {
        Some(url) if prefix.is_empty() => url.clone(),
        Some(url) => format!("{url}#{prefix}"),
        None => root.to_string_lossy().into_owned(),
    };
    let languages = language_stats(&discovery.files);

    let info = RepositoryInfo {
        id: format!("{:016x}", fnv1a(id_basis.as_bytes())),
        name,
        source_files: discovery.files.len() as u32,
        loc: languages.iter().map(|l| l.loc).sum(),
        languages,
        skipped: discovery.skipped.clone(),
        root,
        origin_url,
        branch,
        head_sha,
        analyzed_at: Utc::now(),
    };
    Ok(IngestedRepository { info, discovery })
}

/// Ingests a snapshot of a repository, such as one revision exported to a
/// temporary directory, under the identity (ID, name, origin) of `like`.
/// Crate names fall back to the repository name, so this keeps symbol IDs
/// identical to an analysis of the repository itself.
pub fn ingest_snapshot(
    root: &Path,
    like: &RepositoryInfo,
    commit: Option<String>,
    options: &DiscoveryOptions,
) -> Result<IngestedRepository> {
    let root = canonical_dir(root)?;
    let discovery = discovery::discover(&root, options)?;
    let languages = language_stats(&discovery.files);
    let info = RepositoryInfo {
        id: like.id.clone(),
        name: like.name.clone(),
        source_files: discovery.files.len() as u32,
        loc: languages.iter().map(|l| l.loc).sum(),
        languages,
        skipped: discovery.skipped.clone(),
        root,
        origin_url: like.origin_url.clone(),
        branch: None,
        head_sha: commit,
        analyzed_at: Utc::now(),
    };
    Ok(IngestedRepository { info, discovery })
}

fn canonical_dir(path: &Path) -> Result<PathBuf> {
    let canonical = fs::canonicalize(path).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => AnalyzerError::NotADirectory(path.to_path_buf()),
        _ => AnalyzerError::io(path, err),
    })?;
    if !canonical.is_dir() {
        return Err(AnalyzerError::NotADirectory(path.to_path_buf()));
    }
    Ok(canonical)
}

fn clone_or_update(url: &str, clone_dir: &Path) -> Result<PathBuf> {
    let dest = clone_dir.join(clone_dir_name(url));
    if dest.join(".git").is_dir() {
        tracing::info!(dest = %dest.display(), "updating existing clone");
        Git::discover(&dest)?
            .ok_or_else(|| AnalyzerError::NotADirectory(dest.clone()))?
            .pull_fast_forward()?;
    } else {
        fs::create_dir_all(clone_dir).map_err(|err| AnalyzerError::io(clone_dir, err))?;
        tracing::info!(url = %git::strip_credentials(url), dest = %dest.display(), "cloning");
        Git::clone(url, &dest)?;
    }
    canonical_dir(&dest)
}

/// `<repo-name>-<hash>`: readable, and unique per URL.
fn clone_dir_name(url: &str) -> String {
    let clean = git::strip_credentials(url);
    let name = name_from_url(&clean).unwrap_or("repository");
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe}-{:08x}", fnv1a(clean.as_bytes()) as u32)
}

fn repository_name(origin_url: Option<&str>, root: &Path) -> String {
    origin_url
        .and_then(name_from_url)
        .map(str::to_string)
        .or_else(|| root.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "repository".to_string())
}

fn name_from_url(url: &str) -> Option<&str> {
    let trimmed = url.trim_end_matches('/');
    let last = trimmed.rsplit(['/', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    (!name.is_empty()).then_some(name)
}

fn language_stats(files: &[DiscoveredFile]) -> Vec<LanguageStats> {
    let mut by_language: BTreeMap<Language, LanguageStats> = BTreeMap::new();
    for file in files {
        let entry = by_language.entry(file.language).or_insert(LanguageStats {
            language: file.language,
            files: 0,
            loc: 0,
        });
        entry.files += 1;
        entry.loc += u64::from(file.loc);
    }
    let mut stats: Vec<_> = by_language.into_values().collect();
    stats.sort_by(|a, b| b.loc.cmp(&a.loc).then(a.language.cmp(&b.language)));
    stats
}

/// 64-bit FNV-1a. Used only for stable, non-cryptographic identifiers.
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    fnv1a_extend(FNV1A_OFFSET, bytes)
}

pub(crate) const FNV1A_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// Continues an FNV-1a hash over more bytes.
pub(crate) fn fnv1a_extend(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sources() {
        assert_eq!(
            RepoSource::parse("https://github.com/org/repo"),
            RepoSource::Remote("https://github.com/org/repo".into())
        );
        assert_eq!(
            RepoSource::parse("git@github.com:org/repo.git"),
            RepoSource::Remote("git@github.com:org/repo.git".into())
        );
        assert_eq!(
            RepoSource::parse("../some/dir"),
            RepoSource::Local(PathBuf::from("../some/dir"))
        );
    }

    #[test]
    fn derives_names_from_urls() {
        assert_eq!(
            name_from_url("https://github.com/org/repo.git"),
            Some("repo")
        );
        assert_eq!(name_from_url("git@github.com:org/tool"), Some("tool"));
        assert_eq!(name_from_url("file:///tmp/fixture/"), Some("fixture"));
    }

    #[test]
    fn clone_dir_names_are_stable_and_ignore_credentials() {
        let a = clone_dir_name("https://user:token@example.com/org/my.repo.git");
        let b = clone_dir_name("https://example.com/org/my.repo.git");
        assert_eq!(a, b);
        assert!(a.starts_with("my_repo-"));
    }

    #[test]
    fn missing_path_is_reported() {
        let err = ingest(
            &RepoSource::Local(PathBuf::from("/definitely/not/here")),
            &IngestOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(err, AnalyzerError::NotADirectory(_)));
    }
}
