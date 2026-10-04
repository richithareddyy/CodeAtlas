//! Change-impact analysis: given changed symbols, which other symbols could
//! be affected, and why.
//!
//! The answer is a reverse traversal of the dependency graph:
//!
//! * **Calls.** If `B` is affected and `A` calls `B`, `A` is affected.
//! * **Dispatch.** If an implementation `<T as Tr>::m` is affected, calls
//!   to the trait method `Tr::m` may run it, so `Tr::m` (and through it its
//!   callers) is affected.
//! * **Implementations.** If a trait method declaration is *changed*, every
//!   implementation of it must follow. This rule applies only to changed
//!   symbols, so one implementation never makes its siblings affected.
//! * **Ambiguous calls** (optional) work like calls, but everything reached
//!   only through them is reported as *possible*, never *certain*.
//!
//! Every affected symbol carries the shortest chain of these facts back to
//! a changed symbol, with files and lines. The traversal is bounded by depth
//! and node count, and reports truncation.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::algorithms::bfs;
use super::{CodeGraph, Relation};
use crate::model::{ResolutionMethod, SymbolId, SymbolKind};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImpactOptions {
    pub max_depth: u32,
    pub include_ambiguous: bool,
    pub max_nodes: usize,
}

impl Default for ImpactOptions {
    fn default() -> Self {
        Self {
            max_depth: 8,
            include_ambiguous: false,
            max_nodes: 5_000,
        }
    }
}

/// How `source` depends on `target` in one step of an evidence chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyKind {
    /// `source` calls `target`.
    Calls,
    /// Calls to the trait method `source` may dispatch to `target`, an
    /// implementation of it.
    DispatchesTo,
    /// `source` implements the trait method `target`.
    Implements,
    /// `source` has an ambiguous call for which `target` is a candidate.
    MayCall,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceStep {
    /// The dependent side (further from the change).
    pub source: SymbolId,
    /// The side closer to the change.
    pub target: SymbolId,
    pub kind: DependencyKind,
    /// File holding the evidence: the caller for calls, the implementation
    /// for dispatch and implementation steps.
    pub file: String,
    pub lines: Vec<u32>,
    pub resolution: Option<ResolutionMethod>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Every step is a resolved call, dispatch or implementation.
    Certain,
    /// The chain includes an ambiguous call.
    Possible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolRef {
    pub id: SymbolId,
    pub kind: SymbolKind,
    pub qualified_name: String,
    pub file: String,
    pub line: u32,
    pub is_test: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffectedSymbol {
    pub symbol: SymbolRef,
    pub depth: u32,
    pub confidence: Confidence,
    /// Chain from this symbol to a changed symbol: `path[0].source` is this
    /// symbol, the last step's `target` is a changed symbol.
    pub path: Vec<EvidenceStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffectedGroup {
    /// File path or module ID.
    pub name: String,
    pub symbols: u32,
    pub tests: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImpactReport {
    /// Symbols treated as changed (the requested symbols plus, for types,
    /// traits, modules and files, the members they contain).
    pub changed: Vec<SymbolRef>,
    pub max_depth: u32,
    pub include_ambiguous: bool,
    /// Sorted by confidence, depth, then ID.
    pub affected: Vec<AffectedSymbol>,
    /// Certain results only.
    pub files: Vec<AffectedGroup>,
    pub modules: Vec<AffectedGroup>,
    pub tests: Vec<SymbolId>,
    pub truncated: bool,
    pub score: ImpactScore,
}

impl ImpactReport {
    pub fn certain(&self) -> impl Iterator<Item = &AffectedSymbol> {
        self.affected
            .iter()
            .filter(|a| a.confidence == Confidence::Certain)
    }

    pub fn direct(&self) -> impl Iterator<Item = &AffectedSymbol> {
        self.certain().filter(|a| a.depth == 1)
    }

    pub fn indirect(&self) -> impl Iterator<Item = &AffectedSymbol> {
        self.certain().filter(|a| a.depth > 1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImpactError {
    UnknownSymbol(String),
    UnknownFile(String),
    NothingChanged,
}

impl std::fmt::Display for ImpactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImpactError::UnknownSymbol(id) => write!(f, "unknown symbol `{id}`"),
            ImpactError::UnknownFile(path) => write!(f, "no symbols in file `{path}`"),
            ImpactError::NothingChanged => write!(f, "no changed symbols given"),
        }
    }
}

impl std::error::Error for ImpactError {}

/// Impact of changing one symbol. Types and traits expand to their
/// methods; modules expand to everything written in them.
pub fn impact_of_symbol(
    graph: &CodeGraph,
    id: &SymbolId,
    options: ImpactOptions,
) -> Result<ImpactReport, ImpactError> {
    let node = graph
        .node(id)
        .ok_or_else(|| ImpactError::UnknownSymbol(id.to_string()))?;
    analyze_impact(graph, &expand_change(graph, node), options)
}

/// Impact of changing every symbol defined in a file.
pub fn impact_of_file(
    graph: &CodeGraph,
    path: &str,
    options: ImpactOptions,
) -> Result<ImpactReport, ImpactError> {
    let seeds: Vec<usize> = (0..graph.len())
        .filter(|&n| graph.symbol(n).file == path)
        .collect();
    if seeds.is_empty() {
        return Err(ImpactError::UnknownFile(path.to_string()));
    }
    analyze_impact(graph, &seeds, options)
}

/// Impact of a set of changed symbols (by node index), e.g. from a diff.
pub fn analyze_impact(
    graph: &CodeGraph,
    changed: &[usize],
    options: ImpactOptions,
) -> Result<ImpactReport, ImpactError> {
    let mut seeds: Vec<usize> = changed.to_vec();
    seeds.sort_unstable();
    seeds.dedup();
    if seeds.is_empty() {
        return Err(ImpactError::NothingChanged);
    }
    let is_seed = {
        let mut flags = vec![false; graph.len()];
        for &s in &seeds {
            flags[s] = true;
        }
        flags
    };

    let certain = traverse(graph, &seeds, &is_seed, options, false);
    let mut affected: Vec<AffectedSymbol> = certain
        .iter()
        .map(|(node, (depth, path))| make_affected(graph, *node, depth, Confidence::Certain, path))
        .collect();
    let mut truncated = certain.truncated;

    if options.include_ambiguous {
        let all = traverse(graph, &seeds, &is_seed, options, true);
        truncated |= all.truncated;
        for (node, (depth, path)) in all.iter() {
            if !certain.contains(*node) {
                affected.push(make_affected(
                    graph,
                    *node,
                    depth,
                    Confidence::Possible,
                    path,
                ));
            }
        }
    }
    affected.sort_by(|a, b| {
        (a.confidence, a.depth, &a.symbol.id).cmp(&(b.confidence, b.depth, &b.symbol.id))
    });

    let certain_results: Vec<&AffectedSymbol> = affected
        .iter()
        .filter(|a| a.confidence == Confidence::Certain)
        .collect();
    let group = |key: &dyn Fn(&AffectedSymbol) -> Option<String>| -> Vec<AffectedGroup> {
        let mut groups: BTreeMap<String, (u32, u32)> = BTreeMap::new();
        for a in &certain_results {
            if let Some(name) = key(a) {
                let entry = groups.entry(name).or_default();
                entry.0 += 1;
                entry.1 += u32::from(a.symbol.is_test);
            }
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
    let files = group(&|a| Some(a.symbol.file.clone()));
    let modules = group(&|a| {
        let node = graph.node(&a.symbol.id)?;
        graph.symbol(node).module.as_ref().map(|m| m.to_string())
    });
    let tests: Vec<SymbolId> = certain_results
        .iter()
        .filter(|a| a.symbol.is_test)
        .map(|a| a.symbol.id.clone())
        .collect();

    let fan_in = seeds
        .iter()
        .flat_map(|&s| graph.incoming(s))
        .map(|&e| graph.edge(e))
        .filter(|e| e.relation == Relation::Calls && !is_seed[e.from])
        .map(|e| e.from)
        .collect::<BTreeSet<_>>()
        .len() as u32;
    let score = ImpactScore::compute(ScoreInputs {
        direct_dependents: certain_results.iter().filter(|a| a.depth == 1).count() as u32,
        transitive_dependents: certain_results.len() as u32,
        affected_modules: modules.len() as u32,
        affected_tests: tests.len() as u32,
        fan_in,
        dependency_depth: certain_results.iter().map(|a| a.depth).max().unwrap_or(0),
    });

    Ok(ImpactReport {
        changed: seeds.iter().map(|&s| symbol_ref(graph, s)).collect(),
        max_depth: options.max_depth,
        include_ambiguous: options.include_ambiguous,
        affected,
        files,
        modules,
        tests,
        truncated,
        score,
    })
}

/// The symbols a change to `node` amounts to: a changed type or trait
/// changes its methods; a changed module changes everything written in it
/// (including nested modules).
pub fn expand_change(graph: &CodeGraph, node: usize) -> Vec<usize> {
    let symbol = graph.symbol(node);
    match symbol.kind {
        SymbolKind::Function | SymbolKind::Method => vec![node],
        SymbolKind::Struct | SymbolKind::Enum | SymbolKind::Trait => {
            let mut seeds = vec![node];
            seeds.extend(
                graph
                    .children(node)
                    .iter()
                    .copied()
                    .filter(|&c| graph.symbol(c).kind == SymbolKind::Method),
            );
            seeds
        }
        SymbolKind::Module => {
            let mut modules = BTreeSet::from([symbol.id.clone()]);
            let mut frontier = vec![node];
            while let Some(m) = frontier.pop() {
                for &c in graph.children(m) {
                    if graph.symbol(c).kind == SymbolKind::Module
                        && modules.insert(graph.symbol(c).id.clone())
                    {
                        frontier.push(c);
                    }
                }
            }
            (0..graph.len())
                .filter(|&n| {
                    let s = graph.symbol(n);
                    modules.contains(&s.id)
                        || s.module.as_ref().is_some_and(|m| modules.contains(m))
                })
                .collect()
        }
    }
}

struct Reached {
    depth: Vec<Option<u32>>,
    order: Vec<usize>,
    paths: Vec<Vec<EvidenceStep>>,
    truncated: bool,
}

impl Reached {
    fn contains(&self, node: usize) -> bool {
        self.depth[node].is_some_and(|d| d > 0)
    }

    fn iter(&self) -> impl Iterator<Item = (&usize, (u32, &Vec<EvidenceStep>))> {
        self.order
            .iter()
            .zip(&self.paths)
            .map(move |(node, path)| (node, (self.depth[*node].unwrap_or(0), path)))
    }
}

fn traverse(
    graph: &CodeGraph,
    seeds: &[usize],
    is_seed: &[bool],
    options: ImpactOptions,
    include_ambiguous: bool,
) -> Reached {
    let tree = bfs(
        graph.len(),
        seeds,
        options.max_depth,
        options.max_nodes,
        |node, _depth| dependents(graph, node, is_seed[node], include_ambiguous),
    );
    let paths = tree
        .order
        .iter()
        .map(|&node| {
            // BFS paths run from a seed outwards; evidence reads from the
            // affected symbol back towards the change.
            let mut steps: Vec<EvidenceStep> = tree
                .path_to(node)
                .into_iter()
                .map(|(_, _, step)| step)
                .collect();
            steps.reverse();
            steps
        })
        .collect();
    Reached {
        depth: tree.depth,
        order: tree.order,
        paths,
        truncated: tree.truncated,
    }
}

/// Symbols that depend on `node` in one step, with the evidence.
fn dependents(
    graph: &CodeGraph,
    node: usize,
    is_seed: bool,
    include_ambiguous: bool,
) -> Vec<(usize, EvidenceStep)> {
    let mut out = Vec::new();
    let step = |source: usize, target: usize, kind, edge: &super::IndexedEdge, file_of: usize| {
        EvidenceStep {
            source: graph.symbol(source).id.clone(),
            target: graph.symbol(target).id.clone(),
            kind,
            file: graph.symbol(file_of).file.clone(),
            lines: edge.lines.clone(),
            resolution: edge.resolution,
        }
    };
    for &e in graph.incoming(node) {
        let edge = graph.edge(e);
        match edge.relation {
            Relation::Calls => {
                out.push((
                    edge.from,
                    step(edge.from, node, DependencyKind::Calls, edge, edge.from),
                ));
            }
            Relation::CallsCandidate if include_ambiguous => {
                out.push((
                    edge.from,
                    step(edge.from, node, DependencyKind::MayCall, edge, edge.from),
                ));
            }
            // A changed trait method forces its implementations to change.
            Relation::Implements if is_seed && graph.symbol(node).kind == SymbolKind::Method => {
                out.push((
                    edge.from,
                    step(edge.from, node, DependencyKind::Implements, edge, edge.from),
                ));
            }
            _ => {}
        }
    }
    // An affected implementation affects calls dispatched through its trait method.
    for &e in graph.outgoing(node) {
        let edge = graph.edge(e);
        if edge.relation == Relation::Implements && graph.symbol(edge.to).kind == SymbolKind::Method
        {
            out.push((
                edge.to,
                step(edge.to, node, DependencyKind::DispatchesTo, edge, node),
            ));
        }
    }
    out
}

fn symbol_ref(graph: &CodeGraph, node: usize) -> SymbolRef {
    let s = graph.symbol(node);
    SymbolRef {
        id: s.id.clone(),
        kind: s.kind,
        qualified_name: s.qualified_name.clone(),
        file: s.file.clone(),
        line: s.start_line,
        is_test: s.is_test,
    }
}

fn make_affected(
    graph: &CodeGraph,
    node: usize,
    depth: u32,
    confidence: Confidence,
    path: &[EvidenceStep],
) -> AffectedSymbol {
    AffectedSymbol {
        symbol: symbol_ref(graph, node),
        depth,
        confidence,
        path: path.to_vec(),
    }
}

/// Inputs of the blast-radius score, all counted over certain results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoreInputs {
    pub direct_dependents: u32,
    pub transitive_dependents: u32,
    pub affected_modules: u32,
    pub affected_tests: u32,
    /// Distinct callers of the changed symbols (outside the changed set).
    pub fan_in: u32,
    /// Largest depth at which an affected symbol was found.
    pub dependency_depth: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImpactLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoreFactor {
    pub name: String,
    pub value: u32,
    /// Value at which the factor reaches its full weight.
    pub saturation: u32,
    pub weight: f64,
    /// `ln(1 + value) / ln(1 + saturation)`, capped at 1.
    pub normalized: f64,
    /// `100 × weight × normalized`.
    pub contribution: f64,
}

/// A deterministic summary of how large the affected part of the graph
/// is: the sum of the factor contributions, 0–100. It measures reach, not
/// the probability that something breaks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImpactScore {
    pub total: f64,
    pub level: ImpactLevel,
    pub factors: Vec<ScoreFactor>,
}

impl ImpactScore {
    /// `(name, weight, saturation)`; weights sum to 1.
    pub const FACTORS: [(&'static str, f64, u32); 6] = [
        ("direct_dependents", 0.20, 10),
        ("transitive_dependents", 0.25, 100),
        ("affected_modules", 0.20, 10),
        ("affected_tests", 0.15, 25),
        ("fan_in", 0.10, 10),
        ("dependency_depth", 0.10, 6),
    ];

    pub fn compute(inputs: ScoreInputs) -> Self {
        let values = [
            inputs.direct_dependents,
            inputs.transitive_dependents,
            inputs.affected_modules,
            inputs.affected_tests,
            inputs.fan_in,
            inputs.dependency_depth,
        ];
        let factors: Vec<ScoreFactor> = Self::FACTORS
            .iter()
            .zip(values)
            .map(|(&(name, weight, saturation), value)| {
                let normalized =
                    ((1.0 + f64::from(value)).ln() / (1.0 + f64::from(saturation)).ln()).min(1.0);
                ScoreFactor {
                    name: name.to_string(),
                    value,
                    saturation,
                    weight,
                    normalized: round(normalized, 3),
                    contribution: round(100.0 * weight * normalized, 1),
                }
            })
            .collect();
        let total = round(factors.iter().map(|f| f.contribution).sum(), 1);
        let level = match total {
            t if t < 25.0 => ImpactLevel::Low,
            t if t < 60.0 => ImpactLevel::Medium,
            _ => ImpactLevel::High,
        };
        Self {
            total,
            level,
            factors,
        }
    }
}

fn round(value: f64, places: i32) -> f64 {
    let factor = 10f64.powi(places);
    (value * factor).round() / factor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_is_zero_without_dependents_and_saturates() {
        let zero = ImpactScore::compute(ScoreInputs {
            direct_dependents: 0,
            transitive_dependents: 0,
            affected_modules: 0,
            affected_tests: 0,
            fan_in: 0,
            dependency_depth: 0,
        });
        assert_eq!((zero.total, zero.level), (0.0, ImpactLevel::Low));

        let huge = ImpactScore::compute(ScoreInputs {
            direct_dependents: 1_000,
            transitive_dependents: 10_000,
            affected_modules: 500,
            affected_tests: 900,
            fan_in: 1_000,
            dependency_depth: 40,
        });
        assert_eq!((huge.total, huge.level), (100.0, ImpactLevel::High));
        assert!(huge.factors.iter().all(|f| f.normalized == 1.0));
    }

    #[test]
    fn score_factors_add_up_to_the_total() {
        let score = ImpactScore::compute(ScoreInputs {
            direct_dependents: 2,
            transitive_dependents: 5,
            affected_modules: 2,
            affected_tests: 2,
            fan_in: 2,
            dependency_depth: 3,
        });
        let sum: f64 = score.factors.iter().map(|f| f.contribution).sum();
        assert!((sum - score.total).abs() < 0.11, "{sum} vs {}", score.total);
        // direct: 0.20 * ln(3)/ln(11) * 100 = 9.16...
        assert_eq!(score.factors[0].contribution, 9.2);
        let weights: f64 = ImpactScore::FACTORS.iter().map(|f| f.1).sum();
        assert!((weights - 1.0).abs() < 1e-9);
    }
}
