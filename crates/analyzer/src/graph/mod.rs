//! In-memory code graph and the analyses that run on it.
//!
//! [`CodeGraph`] holds symbols and their resolved (and, separately,
//! ambiguous) relationships. It is built either directly from a
//! [`RepositoryAnalysis`] or from the stored graph (the store crate), and
//! both paths produce identical graphs. [`algorithms`] is generic graph
//! code; [`impact`] and [`architecture`] give it code semantics.

pub mod algorithms;
pub mod architecture;
pub mod impact;
pub mod test_selection;

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::analysis::RepositoryAnalysis;
use crate::dependencies::{derive_dependencies, locate_symbols, Dependencies};
use crate::model::{EdgeKind, ResolutionMethod, SymbolId, SymbolKind};
use algorithms::Digraph;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphSymbol {
    pub id: SymbolId,
    pub kind: SymbolKind,
    pub name: String,
    pub qualified_name: String,
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub parent: Option<SymbolId>,
    /// Module the symbol's code is written in (see `dependencies::Location`).
    pub module: Option<SymbolId>,
    pub is_test: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Relation {
    Calls,
    /// One candidate of an ambiguous call.
    CallsCandidate,
    Imports,
    Implements,
}

impl Relation {
    pub fn as_str(self) -> &'static str {
        match self {
            Relation::Calls => "CALLS",
            Relation::CallsCandidate => "CALLS_CANDIDATE",
            Relation::Imports => "IMPORTS",
            Relation::Implements => "IMPLEMENTS",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "CALLS" => Relation::Calls,
            "CALLS_CANDIDATE" => Relation::CallsCandidate,
            "IMPORTS" => Relation::Imports,
            "IMPLEMENTS" => Relation::Implements,
            _ => return None,
        })
    }
}

impl From<EdgeKind> for Relation {
    fn from(kind: EdgeKind) -> Self {
        match kind {
            EdgeKind::Calls => Relation::Calls,
            EdgeKind::Imports => Relation::Imports,
            EdgeKind::Implements => Relation::Implements,
        }
    }
}

/// An edge between two symbols, by ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolEdge {
    pub from: SymbolId,
    pub to: SymbolId,
    pub relation: Relation,
    /// How a resolved edge was resolved; `None` for candidates.
    pub resolution: Option<ResolutionMethod>,
    /// Source lines in `from`'s file.
    pub lines: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedEdge {
    pub from: usize,
    pub to: usize,
    pub relation: Relation,
    pub resolution: Option<ResolutionMethod>,
    pub lines: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodeGraph {
    symbols: Vec<GraphSymbol>,
    index: HashMap<SymbolId, usize>,
    edges: Vec<IndexedEdge>,
    outgoing: Vec<Vec<usize>>,
    incoming: Vec<Vec<usize>>,
    children: Vec<Vec<usize>>,
    dependencies: Dependencies,
}

impl CodeGraph {
    /// Builds a graph. Symbols and edges are put in a canonical order, so
    /// graphs built from the same facts compare equal regardless of input
    /// order. Edges with unknown endpoints are dropped.
    pub fn new(
        mut symbols: Vec<GraphSymbol>,
        edges: Vec<SymbolEdge>,
        dependencies: Dependencies,
    ) -> Self {
        symbols.sort_by(|a, b| a.id.cmp(&b.id));
        let index: HashMap<SymbolId, usize> = symbols
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id.clone(), i))
            .collect();

        let mut edges: Vec<IndexedEdge> = edges
            .into_iter()
            .filter_map(|e| {
                let mut lines = e.lines;
                lines.sort_unstable();
                lines.dedup();
                Some(IndexedEdge {
                    from: *index.get(&e.from)?,
                    to: *index.get(&e.to)?,
                    relation: e.relation,
                    resolution: e.resolution,
                    lines,
                })
            })
            .collect();
        edges.sort_by_key(|e| (e.from, e.to, e.relation));
        edges.dedup_by(|a, b| (a.from, a.to, a.relation) == (b.from, b.to, b.relation));

        let n = symbols.len();
        let mut outgoing = vec![Vec::new(); n];
        let mut incoming = vec![Vec::new(); n];
        for (i, edge) in edges.iter().enumerate() {
            outgoing[edge.from].push(i);
            incoming[edge.to].push(i);
        }
        let mut children = vec![Vec::new(); n];
        for (i, symbol) in symbols.iter().enumerate() {
            if let Some(parent) = symbol.parent.as_ref().and_then(|p| index.get(p)) {
                children[*parent].push(i);
            }
        }

        let mut dependencies = dependencies;
        dependencies
            .files
            .sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
        dependencies
            .modules
            .sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));

        Self {
            symbols,
            index,
            edges,
            outgoing,
            incoming,
            children,
            dependencies,
        }
    }

    pub fn from_analysis(analysis: &RepositoryAnalysis) -> Self {
        let locations = locate_symbols(&analysis.files);
        let symbols = analysis
            .files
            .iter()
            .flat_map(|f| &f.symbols)
            .map(|s| GraphSymbol {
                id: s.id.clone(),
                kind: s.kind,
                name: s.name.clone(),
                qualified_name: s.qualified_name.clone(),
                file: s.file.clone(),
                start_line: s.span.start_line,
                end_line: s.span.end_line,
                parent: s.parent.clone(),
                module: locations.get(&s.id).map(|l| l.module.clone()),
                is_test: s.is_test,
            })
            .collect();

        let mut edges: Vec<SymbolEdge> = analysis
            .resolution
            .edges
            .iter()
            .map(|e| SymbolEdge {
                from: e.from.clone(),
                to: e.to.clone(),
                relation: e.kind.into(),
                resolution: Some(e.via),
                lines: e.lines.clone(),
            })
            .collect();
        let mut candidates: BTreeMap<(&SymbolId, &SymbolId), Vec<u32>> = BTreeMap::new();
        for call in &analysis.resolution.ambiguous_calls {
            for candidate in &call.candidates {
                candidates
                    .entry((&call.caller, candidate))
                    .or_default()
                    .push(call.line);
            }
        }
        edges.extend(
            candidates
                .into_iter()
                .map(|((from, to), lines)| SymbolEdge {
                    from: from.clone(),
                    to: to.clone(),
                    relation: Relation::CallsCandidate,
                    resolution: None,
                    lines,
                }),
        );

        let dependencies = derive_dependencies(&analysis.files, &analysis.resolution.edges);
        Self::new(symbols, edges, dependencies)
    }

    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }

    pub fn symbols(&self) -> &[GraphSymbol] {
        &self.symbols
    }

    pub fn symbol(&self, node: usize) -> &GraphSymbol {
        &self.symbols[node]
    }

    pub fn node(&self, id: &SymbolId) -> Option<usize> {
        self.index.get(id).copied()
    }

    pub fn edges(&self) -> &[IndexedEdge] {
        &self.edges
    }

    pub fn edge(&self, index: usize) -> &IndexedEdge {
        &self.edges[index]
    }

    /// Indices of edges leaving `node`.
    pub fn outgoing(&self, node: usize) -> &[usize] {
        &self.outgoing[node]
    }

    /// Indices of edges entering `node`.
    pub fn incoming(&self, node: usize) -> &[usize] {
        &self.incoming[node]
    }

    /// Symbols whose parent is `node` (methods of a type, items of a module).
    pub fn children(&self, node: usize) -> &[usize] {
        &self.children[node]
    }

    pub fn dependencies(&self) -> &Dependencies {
        &self.dependencies
    }

    /// Symbol-level digraph over the given relations.
    pub fn projection(&self, relations: &[Relation]) -> Digraph {
        Digraph::from_edges(
            self.len(),
            self.edges
                .iter()
                .filter(|e| relations.contains(&e.relation))
                .map(|e| (e.from, e.to)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(id: &str) -> GraphSymbol {
        GraphSymbol {
            id: SymbolId::new(SymbolKind::Function, id),
            kind: SymbolKind::Function,
            name: id.into(),
            qualified_name: id.into(),
            file: "f.rs".into(),
            start_line: 1,
            end_line: 1,
            parent: None,
            module: None,
            is_test: false,
        }
    }

    fn edge(from: &str, to: &str, lines: Vec<u32>) -> SymbolEdge {
        SymbolEdge {
            from: SymbolId::new(SymbolKind::Function, from),
            to: SymbolId::new(SymbolKind::Function, to),
            relation: Relation::Calls,
            resolution: Some(ResolutionMethod::Scope),
            lines,
        }
    }

    #[test]
    fn construction_is_order_independent_and_drops_dangling_edges() {
        let a = CodeGraph::new(
            vec![symbol("a"), symbol("b")],
            vec![edge("a", "b", vec![3, 1]), edge("a", "missing", vec![1])],
            Dependencies::default(),
        );
        let b = CodeGraph::new(
            vec![symbol("b"), symbol("a")],
            vec![edge("a", "b", vec![1, 3])],
            Dependencies::default(),
        );
        assert_eq!(a, b);
        assert_eq!(a.edges().len(), 1);
        let from = a.node(&SymbolId::new(SymbolKind::Function, "a")).unwrap();
        assert_eq!(a.outgoing(from).len(), 1);
    }
}
