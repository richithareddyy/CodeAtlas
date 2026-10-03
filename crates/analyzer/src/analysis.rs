//! Repository-level analysis: parses every Rust file and assembles the
//! per-file results into a consistent whole.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::error::{AnalyzerError, Result};
use crate::ingest::{self, IngestOptions, IngestedRepository, RepoSource, RepositoryInfo};
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
    let started = Instant::now();
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

    // Pass 1: read every file and collect its `mod` declarations. Syntax
    // trees are dropped immediately; re-parsing in pass 2 is cheaper than
    // holding every tree in memory on large repositories.
    let mut sources = Vec::with_capacity(rust_files.len());
    let mut declarations = BTreeMap::new();
    for file in repo.analyzable_files() {
        let src = fs::read_to_string(&file.abs_path)
            .map_err(|err| AnalyzerError::io(&file.abs_path, err))?;
        let tree = parse(&mut parser, &file.path, &src)?;
        declarations.insert(
            file.path.clone(),
            module_tree::declarations(tree.root_node(), &src),
        );
        sources.push((file.path.as_str(), src));
    }
    module_tree::apply(&mut layout, &declarations);

    // Pass 2: extract symbols with the corrected module paths.
    let mut files = Vec::with_capacity(sources.len());
    for (path, src) in &sources {
        let Some(module) = layout.files.get(*path) else {
            continue;
        };
        let tree = parse(&mut parser, path, src)?;
        let ctx = FileContext {
            path,
            crate_name: &module.crate_name,
            module_path: &module.module_path,
        };
        let analysis = extract_file(&ctx, src, &tree, &mut parser);
        if analysis.syntax_errors > 0 {
            tracing::warn!(
                file = %path,
                errors = analysis.syntax_errors,
                "file contains syntax errors; affected regions were skipped"
            );
        }
        files.push(analysis);
    }
    drop(sources);

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
        ms = stats.total_ms,
        "analysis complete"
    );

    Ok(RepositoryAnalysis {
        repository: repo.info.clone(),
        crates: layout.targets,
        files,
        resolution,
        stats,
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

fn compute_stats(files: &[FileAnalysis]) -> AnalysisStats {
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
