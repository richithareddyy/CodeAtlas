//! Incremental indexing.
//!
//! After each write, the repository's per-file analysis results are saved
//! in an [`IndexState`] file together with a token that is also stored on
//! the `Repository` node. The next index run:
//!
//! 1. finds the files whose content changed (discovery hashes every file),
//!    re-parses only those, and re-runs the cross-file stages (module tree,
//!    resolution) over all files ([`analyze_with_cache`]);
//! 2. rebuilds the previous analysis from the saved state, without parsing,
//!    and turns both into [`GraphSnapshot`]s;
//! 3. if the token on the `Repository` node matches the state, writes only
//!    the [`GraphDelta`] between them in one transaction; otherwise (no
//!    state, another writer, an older format) writes the full graph.
//!
//! The result is the graph a full index would write; the integration tests
//! compare the two.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use codeatlas_analyzer::incremental::{AnalysisCache, ReuseStats};
use codeatlas_analyzer::ingest::IngestedRepository;
use codeatlas_analyzer::{analyze_with_cache, RepositoryAnalysis};
use serde::{Deserialize, Serialize};

use crate::snapshot::{GraphDelta, GraphSnapshot, Prop};
use crate::writer::{self, IndexSummary, GRAPH_FORMAT_VERSION};
use crate::{GraphStore, Result};

/// Version of the state file layout.
const STATE_VERSION: u32 = 1;

/// What the previous index run left behind for the next one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexState {
    pub version: u32,
    pub repo_id: String,
    /// Equal to `index_token` on the `Repository` node written with this
    /// state; a different token means the stored graph was written by
    /// another run and the state no longer describes it.
    pub index_token: String,
    pub analysis: AnalysisCache,
}

/// Directory of state files, one per repository.
#[derive(Debug, Clone)]
pub struct StateDir {
    dir: PathBuf,
}

impl StateDir {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// `$XDG_CACHE_HOME/codeatlas/index`, falling back to `~/.cache` and
    /// then the system temp directory.
    pub fn default_dir() -> PathBuf {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
            .unwrap_or_else(std::env::temp_dir)
            .join("codeatlas")
            .join("index")
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, repo_id: &str) -> PathBuf {
        let safe: String = repo_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.dir.join(format!("{safe}.json"))
    }

    /// The saved state, if there is a readable one for `repo_id`. Damaged
    /// or foreign files are ignored (the next write replaces them).
    pub fn load(&self, repo_id: &str) -> Option<IndexState> {
        let path = self.path(repo_id);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "cannot read index state");
                return None;
            }
        };
        match serde_json::from_str::<IndexState>(&text) {
            Ok(state) if state.version == STATE_VERSION && state.repo_id == repo_id => Some(state),
            Ok(_) => None,
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "ignoring unreadable index state");
                None
            }
        }
    }

    /// Writes the state atomically (temporary file, then rename).
    pub fn save(&self, state: &IndexState) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.path(&state.repo_id);
        let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
        fs::write(&tmp, serde_json::to_vec(state)?)?;
        fs::rename(&tmp, &path)
    }

    pub fn remove(&self, repo_id: &str) -> std::io::Result<()> {
        match fs::remove_file(self.path(repo_id)) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        }
    }
}

/// Why a run wrote the full graph instead of a delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FullReason {
    /// Requested by the caller.
    Requested,
    /// The repository was not indexed yet.
    NotIndexed,
    /// No usable state file (missing, unreadable, or from another version).
    NoState,
    /// The stored graph was written by another run (or has another format).
    IndexChanged,
}

impl FullReason {
    pub fn describe(self) -> &'static str {
        match self {
            FullReason::Requested => "full index requested",
            FullReason::NotIndexed => "first index of this repository",
            FullReason::NoState => "no saved state from a previous index",
            FullReason::IndexChanged => "the stored graph was written by another run",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode", content = "reason")]
pub enum IndexMode {
    Full(FullReason),
    Incremental,
}

/// Size of the write.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeltaStats {
    pub nodes_added: usize,
    pub nodes_removed: usize,
    pub nodes_changed: usize,
    pub relationships_added: usize,
    pub relationships_removed: usize,
    pub relationships_changed: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexReport {
    pub summary: IndexSummary,
    pub mode: IndexMode,
    /// For full writes, everything counts as added.
    pub delta: DeltaStats,
    pub reuse: ReuseStats,
    /// Analysis, including reuse and rebuilding the previous snapshot.
    pub analysis_ms: f64,
    /// Whether the state for the next run was saved.
    pub state_saved: bool,
}

/// An analysis ready to be written.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub analysis: RepositoryAnalysis,
    pub reuse: ReuseStats,
    pub analysis_ms: f64,
    cache: AnalysisCache,
    snapshot: GraphSnapshot,
    previous: Option<(IndexState, GraphSnapshot)>,
}

/// Analyses `repo`, reusing what the saved state allows, and computes the
/// graph to write and the previously written one. CPU-bound: call it from
/// a blocking context.
pub fn prepare(
    repo: &IngestedRepository,
    states: &StateDir,
) -> codeatlas_analyzer::Result<Prepared> {
    let started = Instant::now();
    let state = states
        .load(&repo.info.id)
        .filter(|s| s.analysis.is_current());
    let analyzed = analyze_with_cache(repo, state.as_ref().map(|s| &s.analysis))?;
    // Token and timestamp are set when writing.
    let snapshot = GraphSnapshot::new(&analyzed.analysis, "", "");
    let previous = state.map(|state| {
        let old = state.analysis.rebuild(&analyzed.analysis.repository);
        let old_snapshot = GraphSnapshot::new(&old, "", "");
        (state, old_snapshot)
    });
    Ok(Prepared {
        analysis: analyzed.analysis,
        reuse: analyzed.reuse,
        analysis_ms: started.elapsed().as_secs_f64() * 1000.0,
        cache: analyzed.cache,
        snapshot,
        previous,
    })
}

impl GraphStore {
    /// Writes a prepared analysis: as a delta when the stored graph is the
    /// one the saved state describes, in full otherwise or when
    /// `force_full`. Saves the state for the next run.
    pub async fn index_prepared(
        &self,
        prepared: Prepared,
        states: &StateDir,
        force_full: bool,
    ) -> Result<IndexReport> {
        let Prepared {
            analysis,
            reuse,
            analysis_ms,
            cache,
            mut snapshot,
            previous,
        } = prepared;
        let repo = analysis.repository.id.clone();
        let token = new_token();
        snapshot
            .repository
            .insert("index_token", Prop::Str(token.clone()));
        snapshot
            .repository
            .insert("indexed_at", Prop::Str(chrono::Utc::now().to_rfc3339()));

        let stored = self.stored_token(&repo).await?;
        let mode = match (&previous, &stored) {
            _ if force_full => IndexMode::Full(FullReason::Requested),
            (_, None) => IndexMode::Full(FullReason::NotIndexed),
            (None, Some(_)) => IndexMode::Full(FullReason::NoState),
            (Some((state, _)), Some((token, version)))
                if token.as_deref() == Some(state.index_token.as_str())
                    && *version == GRAPH_FORMAT_VERSION =>
            {
                IndexMode::Incremental
            }
            (Some(_), Some(_)) => IndexMode::Full(FullReason::IndexChanged),
        };

        let (summary, delta) = match (&mode, &previous) {
            (IndexMode::Incremental, Some((_, old))) => {
                let delta = GraphDelta::between(old, &snapshot);
                let summary =
                    writer::write_delta(&self.graph, &repo, old, &snapshot, &delta).await?;
                let stats = DeltaStats {
                    nodes_added: delta.nodes_added(),
                    nodes_removed: delta.nodes_removed(),
                    nodes_changed: delta.nodes_changed(),
                    relationships_added: delta.relationships.added.len(),
                    relationships_removed: delta.relationships.removed.len(),
                    relationships_changed: delta.relationships.changed.len(),
                };
                (summary, stats)
            }
            _ => {
                let summary = writer::write_full(&self.graph, &repo, &snapshot).await?;
                let stats = DeltaStats {
                    nodes_added: summary.nodes,
                    relationships_added: summary.relationships,
                    ..Default::default()
                };
                (summary, stats)
            }
        };

        let state = IndexState {
            version: STATE_VERSION,
            repo_id: repo.clone(),
            index_token: token,
            analysis: cache,
        };
        let state_saved = match states.save(&state) {
            Ok(()) => true,
            Err(err) => {
                // The graph is written; the next run indexes in full.
                tracing::warn!(dir = %states.dir().display(), %err, "cannot save index state");
                false
            }
        };
        Ok(IndexReport {
            summary,
            mode,
            delta,
            reuse,
            analysis_ms,
            state_saved,
        })
    }

    /// `(index_token, format_version)` of a stored repository, `None` when
    /// it is not indexed.
    async fn stored_token(&self, repo: &str) -> Result<Option<(Option<String>, i64)>> {
        let mut rows = self
            .graph
            .execute(
                neo4rs::query(
                    "MATCH (r:Repository {id: $repo}) \
                     RETURN r.index_token AS token, r.format_version AS version",
                )
                .param("repo", repo),
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Some((
                row.get::<Option<String>>("token")?,
                row.get::<Option<i64>>("version")?.unwrap_or(0),
            ))),
            None => Ok(None),
        }
    }
}

/// Unique per write: time, process and a counter.
pub(crate) fn new_token() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!(
        "{nanos:x}-{:x}-{:x}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_files_are_named_safely_and_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let states = StateDir::new(tmp.path());
        assert!(states.load("abc").is_none());
        assert_eq!(states.path("../x/y").file_name().unwrap(), "___x_y.json");

        let repo = codeatlas_analyzer::ingest::ingest(
            &codeatlas_analyzer::RepoSource::Local(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/simple-repo"),
            ),
            &Default::default(),
        )
        .unwrap();
        let prepared = prepare(&repo, &states).unwrap();
        let state = IndexState {
            version: STATE_VERSION,
            repo_id: "abc".into(),
            index_token: new_token(),
            analysis: prepared.cache,
        };
        states.save(&state).unwrap();
        assert_eq!(states.load("abc"), Some(state.clone()));
        // A state saved under another ID is not used.
        assert!(states.load("abd").is_none());
        fs::write(states.path("abc"), "{ not json").unwrap();
        assert!(states.load("abc").is_none());
        states.remove("abc").unwrap();
        states.remove("abc").unwrap();
    }

    #[test]
    fn tokens_are_unique() {
        assert_ne!(new_token(), new_token());
    }
}
