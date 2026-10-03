//! Architecture-level analyses: circular dependencies, highly connected
//! components and dependency layers, at function, file or module level.
//!
//! * **Function level** uses resolved `CALLS` (cycles are recursion).
//! * **File and module level** use the derived `DEPENDS_ON` aggregates.
//!
//! Every reported relationship carries sample symbol-level evidence with a
//! file and line, so a cycle such as `checkout → payments → orders →
//! checkout` can be checked against the source.

use serde::{Deserialize, Serialize};

use super::algorithms::{
    betweenness, cyclic_components, degrees, dependency_layers, shortest_cycle, Digraph,
};
use super::{CodeGraph, Relation};
use crate::dependencies::Dependency;
use crate::model::SymbolId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Function,
    File,
    Module,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub from: SymbolId,
    pub to: SymbolId,
    pub relation: String,
    pub file: String,
    pub line: u32,
}

/// One `from → to` relationship at the chosen level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hop {
    pub from: String,
    pub to: String,
    /// Underlying symbol-level edges (1 at function level).
    pub weight: u32,
    pub via: Vec<String>,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cycle {
    /// Every member of the strongly connected component, sorted.
    pub members: Vec<String>,
    /// A shortest cycle through the component as consecutive hops; the
    /// last hop returns to the first `from`.
    pub hops: Vec<Hop>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hotspot {
    pub name: String,
    /// Number of distinct dependents (callers / depending files or modules).
    pub fan_in: usize,
    /// Number of distinct dependencies.
    pub fan_out: usize,
    /// Shortest dependency paths between other nodes that pass through
    /// this one (Brandes betweenness, unnormalised).
    pub betweenness: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layer {
    /// 0 = depends on nothing else at this level.
    pub layer: u32,
    pub members: Vec<String>,
}

/// A level-specific projection: node names, digraph and hop details.
struct LevelGraph {
    names: Vec<String>,
    graph: Digraph,
    hops: std::collections::HashMap<(usize, usize), Hop>,
}

impl LevelGraph {
    fn build(code: &CodeGraph, level: Level) -> Self {
        match level {
            Level::Function => Self::functions(code),
            Level::File => Self::aggregated(code, &code.dependencies().files),
            Level::Module => Self::aggregated(code, &code.dependencies().modules),
        }
    }

    fn functions(code: &CodeGraph) -> Self {
        let names = code.symbols().iter().map(|s| s.id.to_string()).collect();
        let mut hops = std::collections::HashMap::new();
        for edge in code
            .edges()
            .iter()
            .filter(|e| e.relation == Relation::Calls)
        {
            let from = code.symbol(edge.from);
            hops.insert(
                (edge.from, edge.to),
                Hop {
                    from: from.id.to_string(),
                    to: code.symbol(edge.to).id.to_string(),
                    weight: 1,
                    via: vec![Relation::Calls.as_str().to_string()],
                    evidence: vec![EvidenceRef {
                        from: from.id.clone(),
                        to: code.symbol(edge.to).id.clone(),
                        relation: Relation::Calls.as_str().to_string(),
                        file: from.file.clone(),
                        line: edge.lines.first().copied().unwrap_or(from.start_line),
                    }],
                },
            );
        }
        Self {
            names,
            graph: code.projection(&[Relation::Calls]),
            hops,
        }
    }

    fn aggregated(code: &CodeGraph, deps: &[Dependency]) -> Self {
        let mut names: Vec<String> = deps
            .iter()
            .flat_map(|d| [d.from.clone(), d.to.clone()])
            .collect();
        names.sort();
        names.dedup();
        let index = |name: &str| names.binary_search_by(|n| n.as_str().cmp(name)).ok();

        let mut hops = std::collections::HashMap::new();
        let mut edges = Vec::new();
        for dep in deps {
            let (Some(from), Some(to)) = (index(&dep.from), index(&dep.to)) else {
                continue;
            };
            edges.push((from, to));
            let evidence = dep
                .evidence
                .iter()
                .map(|e| EvidenceRef {
                    from: e.from.clone(),
                    to: e.to.clone(),
                    relation: Relation::from(e.kind).as_str().to_string(),
                    file: code
                        .node(&e.from)
                        .map(|n| code.symbol(n).file.clone())
                        .unwrap_or_default(),
                    line: e.line,
                })
                .collect();
            hops.insert(
                (from, to),
                Hop {
                    from: dep.from.clone(),
                    to: dep.to.clone(),
                    weight: dep.weight,
                    via: dep
                        .via
                        .iter()
                        .map(|k| Relation::from(*k).as_str().to_string())
                        .collect(),
                    evidence,
                },
            );
        }
        let graph = Digraph::from_edges(names.len(), edges);
        Self { names, graph, hops }
    }
}

/// Circular dependencies, largest first.
pub fn cycles(code: &CodeGraph, level: Level) -> Vec<Cycle> {
    let level_graph = LevelGraph::build(code, level);
    cyclic_components(&level_graph.graph)
        .into_iter()
        .map(|component| {
            let order = shortest_cycle(&level_graph.graph, &component);
            let hops = order
                .iter()
                .enumerate()
                .filter_map(|(i, &from)| {
                    let to = order[(i + 1) % order.len()];
                    level_graph.hops.get(&(from, to)).cloned()
                })
                .collect();
            Cycle {
                members: component
                    .iter()
                    .map(|&n| level_graph.names[n].clone())
                    .collect(),
                hops,
            }
        })
        .collect()
}

/// The `limit` most central nodes by betweenness, then fan-in. Nodes
/// without any dependency relationship are omitted.
pub fn hotspots(code: &CodeGraph, level: Level, limit: usize) -> Vec<Hotspot> {
    let level_graph = LevelGraph::build(code, level);
    let (fan_in, fan_out) = degrees(&level_graph.graph);
    let centrality = betweenness(&level_graph.graph);
    let mut spots: Vec<Hotspot> = (0..level_graph.graph.len())
        .filter(|&n| fan_in[n] + fan_out[n] > 0)
        .map(|n| Hotspot {
            name: level_graph.names[n].clone(),
            fan_in: fan_in[n],
            fan_out: fan_out[n],
            betweenness: (centrality[n] * 1000.0).round() / 1000.0,
        })
        .collect();
    spots.sort_by(|a, b| {
        b.betweenness
            .total_cmp(&a.betweenness)
            .then(b.fan_in.cmp(&a.fan_in))
            .then(a.name.cmp(&b.name))
    });
    spots.truncate(limit);
    spots
}

/// Dependency layers (foundations first). Members of a cycle share a layer.
pub fn layers(code: &CodeGraph, level: Level) -> Vec<Layer> {
    let level_graph = LevelGraph::build(code, level);
    let assignment = dependency_layers(&level_graph.graph);
    let (fan_in, fan_out) = degrees(&level_graph.graph);
    let mut layers: std::collections::BTreeMap<u32, Vec<String>> = Default::default();
    for (node, layer) in assignment.into_iter().enumerate() {
        // Nodes with no dependency relationship at all carry no information.
        if fan_in[node] + fan_out[node] > 0 {
            layers
                .entry(layer)
                .or_default()
                .push(level_graph.names[node].clone());
        }
    }
    layers
        .into_iter()
        .map(|(layer, mut members)| {
            members.sort();
            Layer { layer, members }
        })
        .collect()
}
