//! Git diff impact: what changed between two revisions, symbol by symbol,
//! and what else that could affect.
//!
//! 1. Resolve both revisions to commits. Export each into a temporary
//!    directory with `git archive` (the work tree can stand in for the
//!    head) and analyse it like any repository.
//! 2. List changed files and zero-context hunks with `git diff`.
//! 3. [`compare`] the two analyses: added, removed, modified (with
//!    signature changes), moved and cosmetic symbols.
//! 4. Run the impact engine twice: modified symbols on the head graph, and
//!    removed symbols on the base graph (their former dependents may still
//!    exist). Merge the results, leaving out symbols the diff itself
//!    changed, and keep the evidence chain of every downstream symbol.

pub mod compare;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::Instant;

use serde::{Deserialize, Serialize};

pub use compare::{ChangeKind, LineRange, SignatureChange, SymbolChange};

use crate::analysis::{analyze, RepositoryAnalysis};
use crate::error::{AnalyzerError, Result};
use crate::git::{FileChange, FileStatus, Git, Hunk};
use crate::graph::impact::{
    analyze_impact, expand_change, AffectedGroup, AffectedSymbol, Confidence, ImpactOptions,
    SymbolRef,
};
use crate::graph::CodeGraph;
use crate::ingest::{ingest, ingest_snapshot, IngestOptions, RepoSource};
use crate::model::{Symbol, SymbolId, SymbolKind};

#[derive(Debug, Clone, Default)]
pub struct DiffOptions {
    pub impact: ImpactOptions,
    /// Clone directory (for URLs) and file discovery.
    pub ingest: IngestOptions,
}

/// One side of the comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionInfo {
    /// As given (`main`, `HEAD~3`, a SHA), or `working tree`.
    pub label: String,
    /// `None` for the working tree.
    pub sha: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    pub status: FileStatus,
    pub path: String,
    pub old_path: Option<String>,
    /// A `.rs` file, so its symbols were compared.
    pub rust: bool,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Head,
    Base,
}

/// A symbol the diff did not change but that depends on one it did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownstreamSymbol {
    #[serde(flatten)]
    pub affected: AffectedSymbol,
    /// Graph the evidence chain comes from: the head revision, or the base
    /// revision for dependents of removed symbols. Line numbers refer to
    /// that revision.
    pub revision: Side,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffImpact {
    pub max_depth: u32,
    pub include_ambiguous: bool,
    /// Sorted by confidence, depth, then ID.
    pub downstream: Vec<DownstreamSymbol>,
    /// Certain results only, as in an impact report.
    pub files: Vec<AffectedGroup>,
    pub modules: Vec<AffectedGroup>,
    pub tests: Vec<SymbolId>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffSummary {
    pub files_changed: u32,
    pub files_added: u32,
    pub files_removed: u32,
    pub files_modified: u32,
    pub files_renamed: u32,
    /// Functions and methods, tests excluded.
    pub functions_added: u32,
    pub functions_removed: u32,
    pub functions_modified: u32,
    pub tests_added: u32,
    pub tests_removed: u32,
    pub tests_modified: u32,
    /// Structs, enums and traits.
    pub types_added: u32,
    pub types_removed: u32,
    pub types_modified: u32,
    pub signatures_changed: u32,
    pub moved: u32,
    pub cosmetic: u32,
    /// Certain downstream symbols.
    pub downstream_symbols: u32,
    /// Reached only through ambiguous calls (when requested).
    pub possible_symbols: u32,
    pub affected_modules: u32,
    pub affected_files: u32,
    pub affected_tests: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiffStats {
    pub base_analysis_ms: f64,
    pub head_analysis_ms: f64,
    pub total_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffReport {
    pub repository: String,
    pub base: RevisionInfo,
    pub head: RevisionInfo,
    pub files: Vec<ChangedFile>,
    pub symbols: Vec<SymbolChange>,
    /// Touched by the diff, but only whitespace or comments changed.
    pub cosmetic: Vec<SymbolRef>,
    pub impact: DiffImpact,
    pub summary: DiffSummary,
    pub stats: DiffStats,
}

/// Compares `base` with `head` (a revision, or the working tree when
/// `None`) for the repository at `source`.
pub fn analyze_diff(
    source: &RepoSource,
    base: &str,
    head: Option<&str>,
    options: &DiffOptions,
) -> Result<DiffReport> {
    let started = Instant::now();
    let current = ingest(source, &options.ingest)?;
    let root = current.info.root.clone();
    let git = Git::discover(&root)?
        .ok_or_else(|| crate::error::GitError::NotARepository { path: root.clone() })?;
    let prefix = Git::prefix_of(&root)?;
    let base_sha = git.resolve_commit(base)?;
    let head_sha = head.map(|h| git.resolve_commit(h)).transpose()?;

    let scratch = tempfile::Builder::new()
        .prefix("codeatlas-diff-")
        .tempdir()
        .map_err(|err| AnalyzerError::io(std::env::temp_dir(), err))?;
    let snapshot = |side: &str, sha: &str| -> Result<RepositoryAnalysis> {
        // Keep the repository's directory name: it is part of its identity.
        let dir = scratch.path().join(side).join(&current.info.name);
        fs::create_dir_all(&dir).map_err(|err| AnalyzerError::io(&dir, err))?;
        git.export(sha, &prefix, &dir)?;
        let ingested = ingest_snapshot(
            &dir,
            &current.info,
            Some(sha.to_string()),
            &options.ingest.discovery,
        )?;
        analyze(&ingested)
    };

    let timer = Instant::now();
    let base_analysis = snapshot("base", &base_sha)?;
    let base_ms = elapsed_ms(timer);
    let timer = Instant::now();
    let head_analysis = match &head_sha {
        Some(sha) => snapshot("head", sha)?,
        None => analyze(&current)?,
    };
    let head_ms = elapsed_ms(timer);

    let mut file_changes = git.changed_files(&base_sha, head_sha.as_deref(), &prefix)?;
    if head_sha.is_none() {
        let tracked: HashSet<String> = file_changes.iter().map(|c| c.path.clone()).collect();
        for path in git.untracked_files(&prefix)? {
            if !tracked.contains(&path) {
                file_changes.push(FileChange {
                    status: FileStatus::Added,
                    path,
                    old_path: None,
                });
            }
        }
        file_changes.sort_by(|a, b| a.path.cmp(&b.path));
    }
    let hunks = git.rust_hunks(&base_sha, head_sha.as_deref(), &prefix)?;

    let mut report = build_report(
        &base_analysis,
        &head_analysis,
        file_changes,
        hunks,
        options.impact,
    );
    report.repository = current.info.name.clone();
    report.base = RevisionInfo {
        label: base.to_string(),
        sha: Some(base_sha),
    };
    report.head = RevisionInfo {
        label: head.unwrap_or("working tree").to_string(),
        sha: head_sha,
    };
    report.stats = DiffStats {
        base_analysis_ms: base_ms,
        head_analysis_ms: head_ms,
        total_ms: elapsed_ms(started),
    };
    Ok(report)
}

/// Everything after the analyses: symbol comparison, impact and summary.
pub fn build_report(
    base: &RepositoryAnalysis,
    head: &RepositoryAnalysis,
    file_changes: Vec<FileChange>,
    hunks: BTreeMap<String, Vec<Hunk>>,
    options: ImpactOptions,
) -> DiffReport {
    let base_symbols: Vec<&Symbol> = base.files.iter().flat_map(|f| &f.symbols).collect();
    let head_symbols: Vec<&Symbol> = head.files.iter().flat_map(|f| &f.symbols).collect();
    let comparison = compare::compare(&base_symbols, &head_symbols, &hunks);

    let head_graph = CodeGraph::from_analysis(head);
    let base_graph = CodeGraph::from_analysis(base);
    let impact = diff_impact(&head_graph, &base_graph, &comparison.changes, options);

    let files: Vec<ChangedFile> = file_changes
        .into_iter()
        .map(|c| ChangedFile {
            rust: c.path.ends_with(".rs")
                || c.old_path.as_ref().is_some_and(|p| p.ends_with(".rs")),
            hunks: hunks.get(&c.path).cloned().unwrap_or_default(),
            status: c.status,
            path: c.path,
            old_path: c.old_path,
        })
        .collect();
    let summary = summarize(&files, &comparison, &impact);
    DiffReport {
        repository: head.repository.name.clone(),
        base: RevisionInfo {
            label: String::new(),
            sha: base.repository.head_sha.clone(),
        },
        head: RevisionInfo {
            label: String::new(),
            sha: head.repository.head_sha.clone(),
        },
        files,
        symbols: comparison.changes,
        cosmetic: comparison.cosmetic,
        impact,
        summary,
        stats: DiffStats::default(),
    }
}

fn diff_impact(
    head: &CodeGraph,
    base: &CodeGraph,
    changes: &[SymbolChange],
    options: ImpactOptions,
) -> DiffImpact {
    let changed: HashSet<&SymbolId> = changes
        .iter()
        .flat_map(|c| std::iter::once(&c.symbol.id).chain(c.previous.as_ref().map(|p| &p.id)))
        .collect();
    let seeds = |graph: &CodeGraph, kind: ChangeKind| -> Vec<usize> {
        changes
            .iter()
            // A removed module's contents are removed or moved themselves.
            .filter(|c| c.change == kind && c.symbol.kind != SymbolKind::Module)
            .filter_map(|c| graph.node(&c.symbol.id))
            .flat_map(|node| expand_change(graph, node))
            .collect()
    };

    // Best entry per symbol: certain before possible, head before base.
    let mut best: HashMap<SymbolId, DownstreamSymbol> = HashMap::new();
    let mut truncated = false;
    for (graph, side, kind) in [
        (head, Side::Head, ChangeKind::Modified),
        (base, Side::Base, ChangeKind::Removed),
    ] {
        let Ok(report) = analyze_impact(graph, &seeds(graph, kind), options) else {
            continue; // nothing of this kind changed
        };
        truncated |= report.truncated;
        for affected in report.affected {
            let id = affected.symbol.id.clone();
            // Changed symbols are reported as changes, and dependents of
            // removed code only matter if they still exist.
            if changed.contains(&id) || head.node(&id).is_none() {
                continue;
            }
            let candidate = DownstreamSymbol {
                affected,
                revision: side,
            };
            let rank = |d: &DownstreamSymbol| (d.affected.confidence, d.revision);
            match best.get(&id) {
                Some(existing) if rank(existing) <= rank(&candidate) => {}
                _ => {
                    best.insert(id, candidate);
                }
            }
        }
    }
    let mut downstream: Vec<DownstreamSymbol> = best.into_values().collect();
    downstream.sort_by(|a, b| {
        let key = |d: &DownstreamSymbol| (d.affected.confidence, d.affected.depth);
        key(a)
            .cmp(&key(b))
            .then_with(|| a.affected.symbol.id.cmp(&b.affected.symbol.id))
    });

    let certain: Vec<&DownstreamSymbol> = downstream
        .iter()
        .filter(|d| d.affected.confidence == Confidence::Certain)
        .collect();
    let group = |key: &dyn Fn(usize) -> Option<String>| -> Vec<AffectedGroup> {
        let mut groups: BTreeMap<String, (u32, u32)> = BTreeMap::new();
        for d in &certain {
            let Some(name) = head.node(&d.affected.symbol.id).and_then(key) else {
                continue;
            };
            let entry = groups.entry(name).or_default();
            entry.0 += 1;
            entry.1 += u32::from(d.affected.symbol.is_test);
        }
        let mut groups: Vec<AffectedGroup> = groups
            .into_iter()
            .map(|(name, (symbols, tests))| AffectedGroup {
                name,
                symbols,
                tests,
            })
            .collect();
        groups.sort_by(|a, b| b.symbols.cmp(&a.symbols).then(a.name.cmp(&b.name)));
        groups
    };
    let files = group(&|n| Some(head.symbol(n).file.clone()));
    let modules = group(&|n| head.symbol(n).module.as_ref().map(|m| m.to_string()));
    let tests = certain
        .iter()
        .filter(|d| d.affected.symbol.is_test)
        .map(|d| d.affected.symbol.id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    DiffImpact {
        max_depth: options.max_depth,
        include_ambiguous: options.include_ambiguous,
        downstream,
        files,
        modules,
        tests,
        truncated,
    }
}

fn summarize(
    files: &[ChangedFile],
    comparison: &compare::Comparison,
    impact: &DiffImpact,
) -> DiffSummary {
    let mut s = DiffSummary {
        files_changed: files.len() as u32,
        cosmetic: comparison.cosmetic.len() as u32,
        affected_modules: impact.modules.len() as u32,
        affected_files: impact.files.len() as u32,
        affected_tests: impact.tests.len() as u32,
        ..Default::default()
    };
    for f in files {
        match f.status {
            FileStatus::Added => s.files_added += 1,
            FileStatus::Removed => s.files_removed += 1,
            FileStatus::Modified => s.files_modified += 1,
            FileStatus::Renamed => s.files_renamed += 1,
        }
    }
    for c in &comparison.changes {
        let kind = c.symbol.kind;
        let counter = match (kind, c.symbol.is_test, c.change) {
            (_, _, ChangeKind::Moved) => &mut s.moved,
            (SymbolKind::Module, ..) => continue,
            (k, false, change) if k.is_callable() => match change {
                ChangeKind::Added => &mut s.functions_added,
                ChangeKind::Removed => &mut s.functions_removed,
                _ => &mut s.functions_modified,
            },
            (k, true, change) if k.is_callable() => match change {
                ChangeKind::Added => &mut s.tests_added,
                ChangeKind::Removed => &mut s.tests_removed,
                _ => &mut s.tests_modified,
            },
            (_, _, ChangeKind::Added) => &mut s.types_added,
            (_, _, ChangeKind::Removed) => &mut s.types_removed,
            _ => &mut s.types_modified,
        };
        *counter += 1;
        if c.signature.is_some() {
            s.signatures_changed += 1;
        }
    }
    for d in &impact.downstream {
        match d.affected.confidence {
            Confidence::Certain => s.downstream_symbols += 1,
            Confidence::Possible => s.possible_symbols += 1,
        }
    }
    s
}

fn elapsed_ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// Convenience for callers that already hold a path.
pub fn analyze_diff_at(
    root: &Path,
    base: &str,
    head: Option<&str>,
    options: &DiffOptions,
) -> Result<DiffReport> {
    analyze_diff(&RepoSource::Local(root.to_path_buf()), base, head, options)
}
