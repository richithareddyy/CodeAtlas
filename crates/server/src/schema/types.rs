//! GraphQL output types. They mirror the store and analyzer types but are
//! defined here so the public API is an explicit contract that the domain
//! crates do not depend on.

use async_graphql::{Enum, SimpleObject, ID};
use codeatlas_analyzer::graph::{architecture, impact};
use codeatlas_store as store;

fn int(value: impl TryInto<i32>) -> i32 {
    value.try_into().unwrap_or(i32::MAX)
}

#[derive(SimpleObject)]
pub struct Repository {
    pub id: ID,
    pub name: String,
    /// Working-tree path on the server.
    pub root: String,
    pub origin_url: Option<String>,
    pub branch: Option<String>,
    pub head_sha: Option<String>,
    /// Commit the stored graph was built from.
    pub indexed_sha: Option<String>,
    pub indexed_at: String,
    pub format_version: i32,
    pub source_files: i32,
    pub loc: i32,
    pub languages: Vec<String>,
}

impl From<store::RepositoryNode> for Repository {
    fn from(r: store::RepositoryNode) -> Self {
        Self {
            id: ID(r.id),
            name: r.name,
            root: r.root,
            origin_url: r.origin_url,
            branch: r.branch,
            head_sha: r.head_sha,
            indexed_sha: r.indexed_sha,
            indexed_at: r.indexed_at,
            format_version: int(r.format_version),
            source_files: int(r.source_files),
            loc: int(r.loc),
            languages: r.languages,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum SymbolKind {
    Module,
    Struct,
    Enum,
    Trait,
    Function,
    Method,
}

impl SymbolKind {
    pub fn parse(kind: &str) -> Self {
        match kind {
            "module" => SymbolKind::Module,
            "struct" => SymbolKind::Struct,
            "enum" => SymbolKind::Enum,
            "trait" => SymbolKind::Trait,
            "method" => SymbolKind::Method,
            _ => SymbolKind::Function,
        }
    }

    pub fn as_store_kind(self) -> &'static str {
        match self {
            SymbolKind::Module => "module",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait => "trait",
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
        }
    }
}

impl From<codeatlas_analyzer::model::SymbolKind> for SymbolKind {
    fn from(kind: codeatlas_analyzer::model::SymbolKind) -> Self {
        SymbolKind::parse(kind.as_str())
    }
}

/// A call inside a symbol that could not be resolved.
#[derive(SimpleObject)]
pub struct UnresolvedCall {
    pub line: i32,
    pub callee: String,
    /// e.g. `name_not_in_scope`, `dynamic_call`.
    pub reason: String,
}

impl UnresolvedCall {
    /// Parses the stored `line|callee|reason` form. The callee text may
    /// itself contain `|` (closures), so line and reason are split off the
    /// ends.
    fn parse(text: &str) -> Option<Self> {
        let (line, rest) = text.split_once('|')?;
        let (callee, reason) = rest.rsplit_once('|')?;
        Some(Self {
            line: line.parse().ok()?,
            callee: callee.to_string(),
            reason: reason.to_string(),
        })
    }
}

#[derive(SimpleObject)]
pub struct Symbol {
    pub id: ID,
    pub kind: SymbolKind,
    pub name: String,
    pub qualified_name: String,
    pub file: String,
    pub crate_name: String,
    pub start_line: i32,
    pub end_line: i32,
    pub visibility: String,
    pub signature: Option<String>,
    pub is_test: bool,
    pub unresolved_calls: Vec<UnresolvedCall>,
}

impl From<store::SymbolNode> for Symbol {
    fn from(s: store::SymbolNode) -> Self {
        Self {
            id: ID(s.id),
            kind: SymbolKind::parse(&s.kind),
            name: s.name,
            qualified_name: s.qualified_name,
            file: s.file,
            crate_name: s.crate_name,
            start_line: int(s.start_line),
            end_line: int(s.end_line),
            visibility: s.visibility,
            signature: s.signature,
            is_test: s.is_test,
            unresolved_calls: s
                .unresolved_calls
                .iter()
                .filter_map(|c| UnresolvedCall::parse(c))
                .collect(),
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum Relation {
    Calls,
    CallsCandidate,
    Imports,
    Implements,
}

impl Relation {
    fn parse(text: &str) -> Self {
        match text {
            "CALLS_CANDIDATE" => Relation::CallsCandidate,
            "IMPORTS" => Relation::Imports,
            "IMPLEMENTS" => Relation::Implements,
            _ => Relation::Calls,
        }
    }

    pub fn to_store(self) -> store::Relation {
        match self {
            Relation::Calls => store::Relation::Calls,
            Relation::CallsCandidate => store::Relation::CallsCandidate,
            Relation::Imports => store::Relation::Imports,
            Relation::Implements => store::Relation::Implements,
        }
    }
}

#[derive(SimpleObject)]
pub struct GraphEdge {
    pub from: ID,
    pub to: ID,
    pub relation: Relation,
    /// How the edge was resolved (`import`, `receiver_type`, ...); absent
    /// for ambiguous-call candidates.
    pub resolution: Option<String>,
    /// Source lines in the `from` symbol's file.
    pub lines: Vec<i32>,
}

impl From<store::GraphEdge> for GraphEdge {
    fn from(e: store::GraphEdge) -> Self {
        Self {
            from: ID(e.from),
            to: ID(e.to),
            relation: Relation::parse(&e.kind),
            resolution: e.resolution,
            lines: e.lines.into_iter().map(int).collect(),
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum Direction {
    /// What the symbol depends on (callees).
    Dependencies,
    /// What depends on the symbol (callers).
    Dependents,
}

#[derive(SimpleObject)]
pub struct NeighborhoodNode {
    pub symbol: Symbol,
    pub depth: i32,
    /// The edge through which the node was first reached; following these
    /// edges back gives a shortest evidence path to the root.
    pub via: GraphEdge,
}

#[derive(SimpleObject)]
pub struct Neighborhood {
    pub root: Symbol,
    pub direction: Direction,
    pub max_depth: i32,
    pub nodes: Vec<NeighborhoodNode>,
    pub edges: Vec<GraphEdge>,
    /// Set when node or row limits cut the traversal short.
    pub truncated: bool,
}

impl From<store::Traversal> for Neighborhood {
    fn from(t: store::Traversal) -> Self {
        Self {
            root: t.root.into(),
            direction: match t.direction {
                store::Direction::Dependencies => Direction::Dependencies,
                store::Direction::Dependents => Direction::Dependents,
            },
            max_depth: int(t.max_depth),
            nodes: t
                .nodes
                .into_iter()
                .map(|n| NeighborhoodNode {
                    symbol: n.symbol.into(),
                    depth: int(n.depth),
                    via: n.via.into(),
                })
                .collect(),
            edges: t.edges.into_iter().map(Into::into).collect(),
            truncated: t.truncated,
        }
    }
}

#[derive(SimpleObject)]
pub struct DependencyPath {
    pub nodes: Vec<Symbol>,
    pub edges: Vec<GraphEdge>,
}

impl From<store::DependencyPath> for DependencyPath {
    fn from(p: store::DependencyPath) -> Self {
        Self {
            nodes: p.nodes.into_iter().map(Into::into).collect(),
            edges: p.edges.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(SimpleObject)]
pub struct AggregateDependency {
    /// File path or module ID on the other end.
    pub target: String,
    /// Number of symbol-level edges behind the dependency.
    pub weight: i32,
    pub via: Vec<String>,
}

impl From<store::AggregateDependency> for AggregateDependency {
    fn from(d: store::AggregateDependency) -> Self {
        Self {
            target: d.target,
            weight: int(d.weight),
            via: d.via,
        }
    }
}

// ---- Impact ----------------------------------------------------------------

#[derive(SimpleObject)]
pub struct SymbolRef {
    pub id: ID,
    pub kind: SymbolKind,
    pub qualified_name: String,
    pub file: String,
    pub line: i32,
    pub is_test: bool,
}

impl From<impact::SymbolRef> for SymbolRef {
    fn from(s: impact::SymbolRef) -> Self {
        Self {
            id: ID(s.id.to_string()),
            kind: s.kind.into(),
            qualified_name: s.qualified_name,
            file: s.file,
            line: int(s.line),
            is_test: s.is_test,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum DependencyKind {
    /// `source` calls `target`.
    Calls,
    /// Calls to the trait method `source` may dispatch to `target`.
    DispatchesTo,
    /// `source` implements the changed trait method `target`.
    Implements,
    /// `source` has an ambiguous call with `target` as a candidate.
    MayCall,
}

#[derive(SimpleObject)]
pub struct EvidenceStep {
    /// The dependent side (further from the change).
    pub source: ID,
    /// The side closer to the change.
    pub target: ID,
    pub kind: DependencyKind,
    pub file: String,
    pub lines: Vec<i32>,
    pub resolution: Option<String>,
}

impl From<impact::EvidenceStep> for EvidenceStep {
    fn from(s: impact::EvidenceStep) -> Self {
        Self {
            source: ID(s.source.to_string()),
            target: ID(s.target.to_string()),
            kind: match s.kind {
                impact::DependencyKind::Calls => DependencyKind::Calls,
                impact::DependencyKind::DispatchesTo => DependencyKind::DispatchesTo,
                impact::DependencyKind::Implements => DependencyKind::Implements,
                impact::DependencyKind::MayCall => DependencyKind::MayCall,
            },
            file: s.file,
            lines: s.lines.into_iter().map(int).collect(),
            resolution: s.resolution.map(|r| r.as_str().to_string()),
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum Confidence {
    Certain,
    Possible,
}

#[derive(SimpleObject)]
pub struct AffectedSymbol {
    pub symbol: SymbolRef,
    pub depth: i32,
    pub confidence: Confidence,
    /// From this symbol to a changed symbol.
    pub path: Vec<EvidenceStep>,
}

impl From<impact::AffectedSymbol> for AffectedSymbol {
    fn from(a: impact::AffectedSymbol) -> Self {
        Self {
            symbol: a.symbol.into(),
            depth: int(a.depth),
            confidence: match a.confidence {
                impact::Confidence::Certain => Confidence::Certain,
                impact::Confidence::Possible => Confidence::Possible,
            },
            path: a.path.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(SimpleObject)]
pub struct AffectedGroup {
    /// File path or module ID.
    pub name: String,
    pub symbols: i32,
    pub tests: i32,
}

impl From<impact::AffectedGroup> for AffectedGroup {
    fn from(g: impact::AffectedGroup) -> Self {
        Self {
            name: g.name,
            symbols: int(g.symbols),
            tests: int(g.tests),
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ImpactLevel {
    Low,
    Medium,
    High,
}

#[derive(SimpleObject)]
pub struct ScoreFactor {
    pub name: String,
    pub value: i32,
    pub saturation: i32,
    pub weight: f64,
    pub normalized: f64,
    pub contribution: f64,
}

/// Size of the affected graph (0–100) as a sum of factor contributions.
/// Not a probability of breakage.
#[derive(SimpleObject)]
pub struct ImpactScore {
    pub total: f64,
    pub level: ImpactLevel,
    pub factors: Vec<ScoreFactor>,
}

#[derive(SimpleObject)]
pub struct ImpactReport {
    pub changed: Vec<SymbolRef>,
    pub max_depth: i32,
    pub include_ambiguous: bool,
    pub direct_count: i32,
    pub indirect_count: i32,
    pub possible_count: i32,
    pub affected: Vec<AffectedSymbol>,
    pub files: Vec<AffectedGroup>,
    pub modules: Vec<AffectedGroup>,
    pub tests: Vec<ID>,
    pub truncated: bool,
    pub score: ImpactScore,
}

impl From<impact::ImpactReport> for ImpactReport {
    fn from(r: impact::ImpactReport) -> Self {
        let direct = r.direct().count();
        let indirect = r.indirect().count();
        let possible = r.affected.len() - direct - indirect;
        Self {
            changed: r.changed.into_iter().map(Into::into).collect(),
            max_depth: int(r.max_depth),
            include_ambiguous: r.include_ambiguous,
            direct_count: int(direct),
            indirect_count: int(indirect),
            possible_count: int(possible),
            affected: r.affected.into_iter().map(Into::into).collect(),
            files: r.files.into_iter().map(Into::into).collect(),
            modules: r.modules.into_iter().map(Into::into).collect(),
            tests: r.tests.into_iter().map(|t| ID(t.to_string())).collect(),
            truncated: r.truncated,
            score: ImpactScore {
                total: r.score.total,
                level: match r.score.level {
                    impact::ImpactLevel::Low => ImpactLevel::Low,
                    impact::ImpactLevel::Medium => ImpactLevel::Medium,
                    impact::ImpactLevel::High => ImpactLevel::High,
                },
                factors: r
                    .score
                    .factors
                    .into_iter()
                    .map(|f| ScoreFactor {
                        name: f.name,
                        value: int(f.value),
                        saturation: int(f.saturation),
                        weight: f.weight,
                        normalized: f.normalized,
                        contribution: f.contribution,
                    })
                    .collect(),
            },
        }
    }
}

#[derive(SimpleObject)]
pub struct AffectedTest {
    pub test: SymbolRef,
    pub depth: i32,
    /// From the test to the changed symbol.
    pub path: Vec<EvidenceStep>,
}

// ---- Architecture ----------------------------------------------------------

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum Level {
    Function,
    File,
    Module,
}

impl From<Level> for architecture::Level {
    fn from(level: Level) -> Self {
        match level {
            Level::Function => architecture::Level::Function,
            Level::File => architecture::Level::File,
            Level::Module => architecture::Level::Module,
        }
    }
}

#[derive(SimpleObject)]
pub struct EvidenceRef {
    pub from: ID,
    pub to: ID,
    pub relation: String,
    pub file: String,
    pub line: i32,
}

#[derive(SimpleObject)]
pub struct Hop {
    pub from: String,
    pub to: String,
    pub weight: i32,
    pub via: Vec<String>,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(SimpleObject)]
pub struct Cycle {
    pub members: Vec<String>,
    /// A shortest cycle through the members; the last hop returns to the start.
    pub hops: Vec<Hop>,
}

impl From<architecture::Cycle> for Cycle {
    fn from(c: architecture::Cycle) -> Self {
        Self {
            members: c.members,
            hops: c
                .hops
                .into_iter()
                .map(|h| Hop {
                    from: h.from,
                    to: h.to,
                    weight: int(h.weight),
                    via: h.via,
                    evidence: h
                        .evidence
                        .into_iter()
                        .map(|e| EvidenceRef {
                            from: ID(e.from.to_string()),
                            to: ID(e.to.to_string()),
                            relation: e.relation,
                            file: e.file,
                            line: int(e.line),
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

#[derive(SimpleObject)]
pub struct Hotspot {
    pub name: String,
    pub fan_in: i32,
    pub fan_out: i32,
    pub betweenness: f64,
}

impl From<architecture::Hotspot> for Hotspot {
    fn from(h: architecture::Hotspot) -> Self {
        Self {
            name: h.name,
            fan_in: int(h.fan_in),
            fan_out: int(h.fan_out),
            betweenness: h.betweenness,
        }
    }
}

#[derive(SimpleObject)]
pub struct Layer {
    pub layer: i32,
    pub members: Vec<String>,
}

impl From<architecture::Layer> for Layer {
    fn from(l: architecture::Layer) -> Self {
        Self {
            layer: int(l.layer),
            members: l.members,
        }
    }
}

/// A Cargo build target.
#[derive(SimpleObject)]
pub struct Crate {
    pub id: ID,
    pub name: String,
    pub package: String,
    /// `lib`, `bin`, `test`, `example`, `bench` or `buildscript`.
    pub kind: String,
    pub root_file: String,
    /// Module symbol at the root of the crate, if it was analysed.
    pub root_module: Option<ID>,
}

impl From<store::CrateNode> for Crate {
    fn from(c: store::CrateNode) -> Self {
        Self {
            id: ID(c.id),
            name: c.name,
            package: c.package,
            kind: c.kind,
            root_file: c.root_file,
            root_module: c.root_module.map(ID),
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ArchitectureLevel {
    Crate,
    Module,
    File,
}

impl From<ArchitectureLevel> for architecture::ViewLevel {
    fn from(level: ArchitectureLevel) -> Self {
        match level {
            ArchitectureLevel::Crate => architecture::ViewLevel::Crate,
            ArchitectureLevel::Module => architecture::ViewLevel::Module,
            ArchitectureLevel::File => architecture::ViewLevel::File,
        }
    }
}

#[derive(SimpleObject)]
pub struct ArchitectureNode {
    /// Crate name, module ID or file path.
    pub id: String,
    pub fan_in: i32,
    pub fan_out: i32,
    pub in_cycle: bool,
}

#[derive(SimpleObject)]
pub struct ArchitectureEdge {
    pub from: String,
    pub to: String,
    /// Number of symbol-level edges aggregated into this one.
    pub weight: i32,
    pub via: Vec<String>,
}

#[derive(SimpleObject)]
pub struct ArchitectureGraph {
    pub level: ArchitectureLevel,
    pub nodes: Vec<ArchitectureNode>,
    pub edges: Vec<ArchitectureEdge>,
}

impl ArchitectureGraph {
    pub fn new(level: ArchitectureLevel, view: architecture::DependencyView) -> Self {
        Self {
            level,
            nodes: view
                .nodes
                .into_iter()
                .map(|n| ArchitectureNode {
                    id: n.id,
                    fan_in: int(n.fan_in),
                    fan_out: int(n.fan_out),
                    in_cycle: n.in_cycle,
                })
                .collect(),
            edges: view
                .edges
                .into_iter()
                .map(|e| ArchitectureEdge {
                    from: e.from,
                    to: e.to,
                    weight: int(e.weight),
                    via: e.via,
                })
                .collect(),
        }
    }
}

// ---- Misc ------------------------------------------------------------------

#[derive(SimpleObject)]
pub struct SourceSnippet {
    pub file: String,
    pub start_line: i32,
    pub end_line: i32,
    pub total_lines: i32,
    pub lines: Vec<String>,
}

#[derive(SimpleObject)]
pub struct IndexResult {
    pub repository: Repository,
    pub files_analyzed: i32,
    pub nodes: i32,
    pub relationships: i32,
    pub resolution_rate: Option<f64>,
    pub analysis_ms: f64,
    pub write_ms: f64,
}

#[cfg(test)]
mod tests {
    use super::UnresolvedCall;

    #[test]
    fn parses_unresolved_calls_with_pipes_in_the_callee() {
        let call = UnresolvedCall::parse("12|(|x| x)(1)|dynamic_call").unwrap();
        assert_eq!((call.line, call.callee.as_str()), (12, "(|x| x)(1)"));
        assert_eq!(call.reason, "dynamic_call");
        assert!(UnresolvedCall::parse("nonsense").is_none());
    }
}
