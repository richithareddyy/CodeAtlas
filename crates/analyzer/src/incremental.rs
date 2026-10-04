//! Reusing per-file results between analyses of the same repository.
//!
//! An analysis has per-file stages (reading `mod` declarations, extracting
//! symbols) and cross-file stages (the module tree, ID de-duplication and
//! resolution). [`AnalysisCache`] keeps the per-file results; the next
//! analysis reuses a file's results when its content hash, crate and module
//! path are unchanged and parses only the rest. The cross-file stages are
//! always re-run over every file: a change in one file can change what
//! names mean in another (glob imports, re-exports, shadowing), and on
//! ripgrep they take about 20 ms against about 150 ms of parsing.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::analysis::{assemble, compute_stats, RepositoryAnalysis};
use crate::ingest::RepositoryInfo;
use crate::model::{CrateTarget, FileAnalysis};
use crate::module_tree::ModDecl;

/// Version of the cached per-file data. Bump it whenever extraction or the
/// model types change, so that results of an older analyzer are not reused.
pub const CACHE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalysisCache {
    pub version: u32,
    /// `CARGO_PKG_VERSION` of the analyzer that wrote the cache.
    pub analyzer: String,
    pub crates: Vec<CrateTarget>,
    pub dependencies: BTreeSet<String>,
    /// In analysis order.
    pub files: Vec<CachedFile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CachedFile {
    pub path: String,
    pub content_hash: String,
    pub declarations: Vec<ModDecl>,
    /// The other inputs of extraction. `None` for files outside every
    /// crate's module tree, which are not extracted.
    pub crate_name: Option<String>,
    pub module_path: Option<Vec<String>>,
    /// Extraction output before cross-file assembly.
    pub extracted: Option<FileAnalysis>,
}

/// What an analysis reused from its cache.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReuseStats {
    /// Rust files in the repository.
    pub files: u32,
    /// Files whose content differs from the cache, or that are new or gone.
    pub changed: u32,
    pub added: u32,
    pub removed: u32,
    /// Files read and parsed for their `mod` declarations.
    pub parsed: u32,
    /// Files whose symbols were extracted (changed content, or a new crate
    /// or module path).
    pub extracted: u32,
    /// Files whose extraction results were reused.
    pub reused: u32,
}

impl AnalysisCache {
    pub(crate) fn new(
        crates: Vec<CrateTarget>,
        dependencies: BTreeSet<String>,
        files: Vec<CachedFile>,
    ) -> Self {
        Self {
            version: CACHE_VERSION,
            analyzer: env!("CARGO_PKG_VERSION").to_string(),
            crates,
            dependencies,
            files,
        }
    }

    /// Written by this version of the analyzer, so its results can be
    /// reused.
    pub fn is_current(&self) -> bool {
        self.version == CACHE_VERSION && self.analyzer == env!("CARGO_PKG_VERSION")
    }

    /// Rebuilds the analysis this cache was produced with (timings aside),
    /// without parsing. `repository` describes the repository it belongs to.
    pub fn rebuild(&self, repository: &RepositoryInfo) -> RepositoryAnalysis {
        let mut files: Vec<FileAnalysis> = self
            .files
            .iter()
            .filter_map(|f| f.extracted.clone())
            .collect();
        let resolution = assemble(&mut files, &self.crates, &self.dependencies);
        let stats = compute_stats(&files);
        RepositoryAnalysis {
            repository: repository.clone(),
            crates: self.crates.clone(),
            files,
            resolution,
            stats,
        }
    }
}
