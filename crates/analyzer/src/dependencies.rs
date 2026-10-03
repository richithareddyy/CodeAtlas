//! Derived file-level and module-level dependencies.
//!
//! A file (or module) depends on another when code written in it has a
//! resolved CALLS, IMPORTS or IMPLEMENTS edge to a symbol defined in the
//! other. These aggregates feed cycle detection and the architecture view;
//! each keeps the number of underlying edges, their kinds and a few sample
//! edges, so it can be traced back to symbol-level evidence.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::model::{Edge, EdgeKind, FileAnalysis, SymbolId, SymbolKind};

/// Sample edges kept per dependency.
pub const MAX_EVIDENCE: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    /// File path or module symbol ID.
    pub from: String,
    pub to: String,
    /// Number of symbol-level edges aggregated into this dependency.
    pub weight: u32,
    pub via: Vec<EdgeKind>,
    /// Up to [`MAX_EVIDENCE`] of the underlying edges, in edge order.
    pub evidence: Vec<DependencyEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyEvidence {
    pub from: SymbolId,
    pub to: SymbolId,
    pub kind: EdgeKind,
    /// First source line of the underlying edge.
    pub line: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependencies {
    pub files: Vec<Dependency>,
    pub modules: Vec<Dependency>,
}

/// Where a symbol's code is written: its file and the innermost module
/// (file module or inline `mod { }`) lexically containing it. Methods of
/// impl blocks written outside their type's module count towards the module
/// that contains the impl block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location<'a> {
    pub file: &'a str,
    pub module: &'a SymbolId,
}

#[derive(Default)]
struct Aggregate {
    weight: u32,
    via: BTreeSet<EdgeKind>,
    evidence: Vec<DependencyEvidence>,
}

impl Aggregate {
    fn add(&mut self, edge: &Edge) {
        self.weight += 1;
        self.via.insert(edge.kind);
        if self.evidence.len() < MAX_EVIDENCE {
            self.evidence.push(DependencyEvidence {
                from: edge.from.clone(),
                to: edge.to.clone(),
                kind: edge.kind,
                line: edge.lines.first().copied().unwrap_or(0),
            });
        }
    }
}

pub fn derive_dependencies(files: &[FileAnalysis], edges: &[Edge]) -> Dependencies {
    let locations = locate_symbols(files);
    let mut by_file: BTreeMap<(&str, &str), Aggregate> = BTreeMap::new();
    let mut by_module: BTreeMap<(&str, &str), Aggregate> = BTreeMap::new();

    for edge in edges {
        let (Some(from), Some(to)) = (locations.get(&edge.from), locations.get(&edge.to)) else {
            continue;
        };
        if from.file != to.file {
            by_file.entry((from.file, to.file)).or_default().add(edge);
        }
        if from.module != to.module {
            by_module
                .entry((from.module.as_str(), to.module.as_str()))
                .or_default()
                .add(edge);
        }
    }

    let collect = |map: BTreeMap<(&str, &str), Aggregate>| {
        map.into_iter()
            .map(|((from, to), agg)| Dependency {
                from: from.to_string(),
                to: to.to_string(),
                weight: agg.weight,
                via: agg.via.into_iter().collect(),
                evidence: agg.evidence,
            })
            .collect()
    };
    Dependencies {
        files: collect(by_file),
        modules: collect(by_module),
    }
}

/// File and lexical module of every symbol.
pub fn locate_symbols(files: &[FileAnalysis]) -> HashMap<&SymbolId, Location<'_>> {
    let mut locations = HashMap::new();
    for file in files {
        let modules: Vec<_> = file
            .symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Module)
            .collect();
        for symbol in &file.symbols {
            let module = if symbol.kind == SymbolKind::Module {
                Some(&symbol.id)
            } else {
                // Innermost module whose span contains the symbol's first line.
                modules
                    .iter()
                    .filter(|m| {
                        m.span.start_line <= symbol.span.start_line
                            && symbol.span.start_line <= m.span.end_line
                    })
                    .max_by_key(|m| m.span.start_line)
                    .map(|m| &m.id)
            };
            if let Some(module) = module {
                locations.insert(
                    &symbol.id,
                    Location {
                        file: &file.path,
                        module,
                    },
                );
            }
        }
    }
    locations
}
