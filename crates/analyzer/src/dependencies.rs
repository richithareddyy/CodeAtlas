//! Derived file-level and module-level dependencies.
//!
//! A file (or module) depends on another when code written in it has a
//! resolved CALLS, IMPORTS or IMPLEMENTS edge to a symbol defined in the
//! other. These aggregates feed cycle detection and the architecture view;
//! each keeps the number of underlying edges and their kinds so it can be
//! traced back to the symbol-level evidence.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::model::{Edge, EdgeKind, FileAnalysis, SymbolId, SymbolKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    /// File path or module symbol ID.
    pub from: String,
    pub to: String,
    /// Number of symbol-level edges aggregated into this dependency.
    pub weight: u32,
    pub via: Vec<EdgeKind>,
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
struct Location<'a> {
    file: &'a str,
    module: &'a SymbolId,
}

pub fn derive_dependencies(files: &[FileAnalysis], edges: &[Edge]) -> Dependencies {
    let locations = locate_symbols(files);
    let mut by_file: BTreeMap<(&str, &str), (u32, BTreeSet<EdgeKind>)> = BTreeMap::new();
    let mut by_module: BTreeMap<(&str, &str), (u32, BTreeSet<EdgeKind>)> = BTreeMap::new();

    for edge in edges {
        let (Some(from), Some(to)) = (locations.get(&edge.from), locations.get(&edge.to)) else {
            continue;
        };
        if from.file != to.file {
            let entry = by_file.entry((from.file, to.file)).or_default();
            entry.0 += 1;
            entry.1.insert(edge.kind);
        }
        if from.module != to.module {
            let entry = by_module
                .entry((from.module.as_str(), to.module.as_str()))
                .or_default();
            entry.0 += 1;
            entry.1.insert(edge.kind);
        }
    }

    let collect = |map: BTreeMap<(&str, &str), (u32, BTreeSet<EdgeKind>)>| {
        map.into_iter()
            .map(|((from, to), (weight, via))| Dependency {
                from: from.to_string(),
                to: to.to_string(),
                weight,
                via: via.into_iter().collect(),
            })
            .collect()
    };
    Dependencies {
        files: collect(by_file),
        modules: collect(by_module),
    }
}

fn locate_symbols(files: &[FileAnalysis]) -> HashMap<&SymbolId, Location<'_>> {
    let mut locations = HashMap::new();
    for file in files {
        let modules: Vec<_> = file
            .symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Module)
            .collect();
        for symbol in &file.symbols {
            // Innermost module whose span contains the symbol's first line.
            let module = modules
                .iter()
                .filter(|m| {
                    m.span.start_line <= symbol.span.start_line
                        && symbol.span.start_line <= m.span.end_line
                })
                .max_by_key(|m| m.span.start_line)
                .map(|m| &m.id);
            let module = if symbol.kind == SymbolKind::Module {
                Some(&symbol.id)
            } else {
                module
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
