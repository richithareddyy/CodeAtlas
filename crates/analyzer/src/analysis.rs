//! Repository-level analysis: parses every Rust file and assembles the
//! per-file results into a consistent whole.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::error::{AnalyzerError, Result};
use crate::incremental::{AnalysisCache, CachedFile, ReuseStats};
use crate::ingest::{
    self, DiscoveredFile, IngestOptions, IngestedRepository, RepoSource, RepositoryInfo,
};
use crate::layout::{build_layout, manifest_reader};
use crate::model::{CrateTarget, FileAnalysis, SymbolId, SymbolKind};
use crate::module_tree;
use crate::parser::RustParser;
use crate::resolver::{self, Resolution};
use crate::symbols::{extract_file, FileContext};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryAnalysis {
    pub repository: RepositoryInfo,
    pub crates: Vec<CrateTarget>,
    pub files: Vec<FileAnalysis>,
    pub resolution: Resolution,
    pub stats: AnalysisStats,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AnalysisStats {
    pub files_analyzed: u32,
    pub files_with_syntax_errors: u32,
    pub loc_analyzed: u64,
    pub symbols: BTreeMap<SymbolKind, u32>,
    pub tests: u32,
    pub imports: u32,
    pub impl_blocks: u32,
    pub call_sites: u32,
    pub macro_call_sites: u32,
    pub parse_ms: f64,
    pub resolve_ms: f64,
    pub total_ms: f64,
}

/// Ingests `source` and analyses it.
pub fn analyze_source(source: &RepoSource, options: &IngestOptions) -> Result<RepositoryAnalysis> {
    let ingested = ingest::ingest(source, options)?;
    analyze(&ingested)
}

pub fn analyze(repo: &IngestedRepository) -> Result<RepositoryAnalysis> {
    Ok(analyze_with_cache(repo, None)?.analysis)
}

/// The result of [`analyze_with_cache`].
#[derive(Debug, Clone)]
pub struct Analyzed {
    pub analysis: RepositoryAnalysis,
    /// Per-file results to pass to the next analysis of the repository.
    pub cache: AnalysisCache,
    pub reuse: ReuseStats,
}

/// Analyses `repo`, reusing the per-file results in `previous` for files
/// whose content, crate and module path are unchanged. Cross-file work
/// (module tree, ID de-duplication, resolution) always runs over all
/// files, so the result is the same as a full analysis.
pub fn analyze_with_cache(
    repo: &IngestedRepository,
    previous: Option<&AnalysisCache>,
) -> Result<Analyzed> {
    let started = Instant::now();
    let previous = previous.filter(|c| c.is_current());
    let cached: HashMap<&str, &CachedFile> = previous
        .map(|c| c.files.iter().map(|f| (f.path.as_str(), f)).collect())
        .unwrap_or_default();
    let root = &repo.info.root;
    let rust_files: Vec<String> = repo.analyzable_files().map(|f| f.path.clone()).collect();
    let mut layout = build_layout(
        &repo.info.name.replace('-', "_"),
        &repo.discovery.cargo_manifests,
        &rust_files,
        manifest_reader(root),
    );

    let mut parser = RustParser::new()?;
    let mut parse_ms = 0.0;
    let mut parse = |parser: &mut RustParser, path: &str, src: &str| -> Result<_> {
        let started = Instant::now();
        let tree = parser.parse(src, path)?;
        parse_ms += elapsed_ms(started);
        Ok(tree)
    };
    let mut reuse = ReuseStats {
        files: rust_files.len() as u32,
        ..Default::default()
    };

    // Pass 1: `mod` declarations of every file, from the cache when the
    // content is unchanged. Syntax trees are dropped immediately;
    // re-parsing in pass 2 is cheaper than holding every tree in memory on
    // large repositories.
    let mut sources: Vec<(&DiscoveredFile, Option<String>)> = Vec::with_capacity(rust_files.len());
    let mut declarations = BTreeMap::new();
    for file in repo.analyzable_files() {
        let unchanged = cached
            .get(file.path.as_str())
            .filter(|c| c.content_hash == file.content_hash);
        match unchanged {
            Some(entry) => {
                declarations.insert(file.path.clone(), entry.declarations.clone());
                sources.push((file, None));
            }
            None => {
                let src = fs::read_to_string(&file.abs_path)
                    .map_err(|err| AnalyzerError::io(&file.abs_path, err))?;
                let tree = parse(&mut parser, &file.path, &src)?;
                declarations.insert(
                    file.path.clone(),
                    module_tree::declarations(tree.root_node(), &src),
                );
                reuse.parsed += 1;
                if cached.contains_key(file.path.as_str()) {
                    reuse.changed += 1;
                } else if previous.is_some() {
                    reuse.added += 1;
                }
                sources.push((file, Some(src)));
            }
        }
    }
    if let Some(previous) = previous {
        let current: HashSet<&str> = rust_files.iter().map(String::as_str).collect();
        reuse.removed = previous
            .files
            .iter()
            .filter(|f| !current.contains(f.path.as_str()))
            .count() as u32;
    }
    module_tree::apply(&mut layout, &declarations);

    // Pass 2: extract symbols with the corrected module paths, reusing
    // results whose inputs are unchanged.
    let mut files = Vec::with_capacity(sources.len());
    let mut entries = Vec::with_capacity(sources.len());
    for (file, src) in sources {
        let path = file.path.as_str();
        let module = layout.files.get(path);
        let reusable = cached.get(path).filter(|c| {
            src.is_none()
                && module.is_some_and(|m| {
                    c.crate_name.as_deref() == Some(m.crate_name.as_str())
                        && c.module_path.as_deref() == Some(m.module_path.as_slice())
                })
        });
        let extracted = match (module, reusable.and_then(|c| c.extracted.as_ref())) {
            (None, _) => None,
            (Some(_), Some(extracted)) => {
                reuse.reused += 1;
                Some(extracted.clone())
            }
            (Some(module), None) => {
                let src = match src {
                    Some(src) => src,
                    None => fs::read_to_string(&file.abs_path)
                        .map_err(|err| AnalyzerError::io(&file.abs_path, err))?,
                };
                let tree = parse(&mut parser, path, &src)?;
                let ctx = FileContext {
                    path,
                    crate_name: &module.crate_name,
                    module_path: &module.module_path,
                };
                let analysis = extract_file(&ctx, &src, &tree, &mut parser);
                if analysis.syntax_errors > 0 {
                    tracing::warn!(
                        file = %path,
                        errors = analysis.syntax_errors,
                        "file contains syntax errors; affected regions were skipped"
                    );
                }
                reuse.extracted += 1;
                Some(analysis)
            }
        };
        entries.push(CachedFile {
            path: path.to_string(),
            // The hash of the content actually extracted, not of the
            // content discovery saw, in case the file changed in between.
            content_hash: extracted
                .as_ref()
                .map_or_else(|| file.content_hash.clone(), |e| e.content_hash.clone()),
            declarations: declarations.remove(path).unwrap_or_default(),
            crate_name: module.map(|m| m.crate_name.clone()),
            module_path: module.map(|m| m.module_path.clone()),
            extracted: extracted.clone(),
        });
        files.extend(extracted);
    }

    let cache = AnalysisCache::new(layout.targets.clone(), layout.dependencies.clone(), entries);
    let resolve_started = Instant::now();
    let resolution = assemble(&mut files, &layout.targets, &layout.dependencies);
    let resolve_ms = elapsed_ms(resolve_started);

    let mut stats = compute_stats(&files);
    stats.parse_ms = parse_ms;
    stats.resolve_ms = resolve_ms;
    stats.total_ms = elapsed_ms(started);
    tracing::info!(
        files = stats.files_analyzed,
        call_sites = stats.call_sites,
        resolved_calls = resolution.stats.calls.resolved,
        parsed = reuse.parsed,
        reused = reuse.reused,
        ms = stats.total_ms,
        "analysis complete"
    );

    Ok(Analyzed {
        analysis: RepositoryAnalysis {
            repository: repo.info.clone(),
            crates: layout.targets,
            files,
            resolution,
            stats,
        },
        cache,
        reuse,
    })
}

/// Makes per-file results consistent (unique IDs, linked modules) and
/// resolves references across files.
pub(crate) fn assemble(
    files: &mut [FileAnalysis],
    crates: &[CrateTarget],
    dependencies: &BTreeSet<String>,
) -> Resolution {
    dedupe_ids_across_files(files);
    link_modules(files);
    resolver::resolve(files, crates, dependencies)
}

/// Files are processed in path order, so the first definition keeps the
/// plain ID and later ones get `#N` suffixes deterministically.
fn dedupe_ids_across_files(files: &mut [FileAnalysis]) {
    let mut seen: HashMap<SymbolId, u32> = HashMap::new();
    for file in files.iter_mut() {
        let mut renames = HashMap::new();
        for symbol in &file.symbols {
            let mut candidate = symbol.id.clone();
            let mut n = 1;
            while seen.contains_key(&candidate) {
                n += 1;
                candidate = symbol.id.with_suffix(n);
            }
            seen.insert(candidate.clone(), 1);
            if candidate != symbol.id {
                renames.insert(symbol.id.clone(), candidate);
            }
        }
        if !renames.is_empty() {
            file.remap_ids(&renames);
        }
    }
}

/// Connects each file module to its parent module and applies the
/// visibility and `cfg(test)` status written on the parent's `mod` item.
fn link_modules(files: &mut [FileAnalysis]) {
    let modules_by_qname: HashMap<String, SymbolId> = files
        .iter()
        .flat_map(|f| &f.symbols)
        .filter(|s| s.kind == SymbolKind::Module)
        .map(|s| (s.qualified_name.clone(), s.id.clone()))
        .collect();
    let decls: HashMap<(SymbolId, String), _> = files
        .iter()
        .flat_map(|f| &f.module_decls)
        .map(|d| ((d.parent.clone(), d.name.clone()), d.clone()))
        .collect();

    for file in files.iter_mut() {
        let Some(module_id) = file.module.clone() else {
            continue;
        };
        let Some(symbol) = file.symbols.iter_mut().find(|s| s.id == module_id) else {
            continue;
        };
        let Some((parent_qname, _)) = symbol.qualified_name.rsplit_once("::") else {
            continue;
        };
        let Some(parent_id) = modules_by_qname.get(parent_qname) else {
            continue;
        };
        symbol.parent = Some(parent_id.clone());
        if let Some(decl) = decls.get(&(parent_id.clone(), symbol.name.clone())) {
            symbol.visibility = decl.visibility.clone();
            symbol.cfg_test |= decl.cfg_test;
        }
    }
}

pub(crate) fn compute_stats(files: &[FileAnalysis]) -> AnalysisStats {
    let mut stats = AnalysisStats {
        files_analyzed: files.len() as u32,
        ..Default::default()
    };
    for file in files {
        if file.syntax_errors > 0 {
            stats.files_with_syntax_errors += 1;
        }
        stats.loc_analyzed += u64::from(file.loc);
        for symbol in &file.symbols {
            *stats.symbols.entry(symbol.kind).or_default() += 1;
            if symbol.is_test {
                stats.tests += 1;
            }
        }
        stats.imports += file.imports.len() as u32;
        stats.impl_blocks += file.impls.len() as u32;
        stats.call_sites += file.calls.len() as u32;
        stats.macro_call_sites += file.calls.iter().filter(|c| c.in_macro).count() as u32;
    }
    stats
}

fn elapsed_ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}
