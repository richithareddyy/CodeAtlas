//! GraphQL output types. They mirror the store and analyzer types but are
//! defined here so the public API is an explicit contract that the domain
//! crates do not depend on.

use async_graphql::{Enum, SimpleObject, ID};
use codeatlas_analyzer::graph::{architecture, impact, test_selection};
use codeatlas_analyzer::{diff, git};
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
pub struct SelectedTest {
    pub test: SymbolRef,
    /// Length of the evidence chain.
    pub depth: i32,
    /// Call steps in the chain; 1 for direct tests.
    pub calls: i32,
    pub confidence: Confidence,
    /// From the test to a changed symbol.
    pub path: Vec<EvidenceStep>,
    /// Changed symbols the test reaches.
    pub reaches: Vec<ID>,
}

impl From<test_selection::SelectedTest> for SelectedTest {
    fn from(t: test_selection::SelectedTest) -> Self {
        Self {
            test: t.test.into(),
            depth: int(t.depth),
            calls: int(t.calls),
            confidence: match t.confidence {
                impact::Confidence::Certain => Confidence::Certain,
                impact::Confidence::Possible => Confidence::Possible,
            },
            path: t.path.into_iter().map(Into::into).collect(),
            reaches: t.reaches.into_iter().map(|id| ID(id.to_string())).collect(),
        }
    }
}

/// Tests to run for a change.
#[derive(SimpleObject)]
pub struct TestSelection {
    /// Changed symbols, with types and modules expanded to their members.
    pub changed: Vec<SymbolRef>,
    /// Tests among the changed symbols.
    pub changed_tests: Vec<SymbolRef>,
    /// Tests calling changed code directly.
    pub direct: Vec<SelectedTest>,
    /// Tests reaching changed code through other code.
    pub transitive: Vec<SelectedTest>,
    /// Tests reached only through ambiguous calls (when requested).
    pub possible: Vec<SelectedTest>,
    /// Changed functions and methods no test reaches.
    pub untested: Vec<SymbolRef>,
    pub total_tests: i32,
    pub truncated: bool,
}

impl From<test_selection::TestSelection> for TestSelection {
    fn from(s: test_selection::TestSelection) -> Self {
        Self {
            changed: s.changed.into_iter().map(Into::into).collect(),
            changed_tests: s.changed_tests.into_iter().map(Into::into).collect(),
            direct: s.direct.into_iter().map(Into::into).collect(),
            transitive: s.transitive.into_iter().map(Into::into).collect(),
            possible: s.possible.into_iter().map(Into::into).collect(),
            untested: s.untested.into_iter().map(Into::into).collect(),
            total_tests: int(s.total_tests),
            truncated: s.truncated,
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
    /// Nodes and relationships stored after the write.
    pub nodes: i32,
    pub relationships: i32,
    pub resolution_rate: Option<f64>,
    pub analysis_ms: f64,
    pub write_ms: f64,
    pub mode: IndexMode,
    /// Why the whole graph was written (`null` for incremental writes):
    /// `requested`, `not_indexed`, `no_state` or `index_changed`.
    pub full_reason: Option<String>,
    /// Rust files whose content changed since the previous index (including
    /// added ones); `filesParsed` were read and parsed, `filesReused` taken
    /// from the saved state.
    pub files_changed: i32,
    pub files_removed: i32,
    pub files_parsed: i32,
    pub files_reused: i32,
    /// Size of the write; for full writes everything counts as added.
    pub nodes_added: i32,
    pub nodes_removed: i32,
    pub nodes_changed: i32,
    pub relationships_added: i32,
    pub relationships_removed: i32,
    pub relationships_changed: i32,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum IndexMode {
    /// The whole graph was written.
    Full,
    /// Only the difference to the previously stored graph was written.
    Incremental,
}

// ---- Git diff impact --------------------------------------------------------

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum FileStatus {
    Added,
    Removed,
    Modified,
    Renamed,
}

impl From<git::FileStatus> for FileStatus {
    fn from(status: git::FileStatus) -> Self {
        match status {
            git::FileStatus::Added => FileStatus::Added,
            git::FileStatus::Removed => FileStatus::Removed,
            git::FileStatus::Modified => FileStatus::Modified,
            git::FileStatus::Renamed => FileStatus::Renamed,
        }
    }
}

/// A zero-context diff hunk. A pure deletion has `newLines = 0`, and
/// `newStart` is the line after which the lines were removed.
#[derive(SimpleObject)]
pub struct Hunk {
    pub old_start: i32,
    pub old_lines: i32,
    pub new_start: i32,
    pub new_lines: i32,
}

#[derive(SimpleObject)]
pub struct ChangedFile {
    pub status: FileStatus,
    pub path: String,
    /// Previous path of a renamed file.
    pub old_path: Option<String>,
    /// A Rust file, so its symbols were compared.
    pub rust: bool,
    pub hunks: Vec<Hunk>,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ChangeKind {
    Modified,
    Added,
    Removed,
    /// Same code under a new ID (e.g. after a file rename).
    Moved,
}

#[derive(SimpleObject)]
pub struct LineRange {
    pub start: i32,
    pub end: i32,
}

#[derive(SimpleObject)]
pub struct SignatureChange {
    pub before: String,
    pub after: String,
}

#[derive(SimpleObject)]
pub struct SymbolChange {
    pub change: ChangeKind,
    /// The symbol in the head revision (the base revision for removed ones).
    pub symbol: SymbolRef,
    /// The symbol in the base revision, for modified and moved symbols.
    pub previous: Option<SymbolRef>,
    pub signature: Option<SignatureChange>,
    /// Changed lines inside the symbol, in `symbol.file`.
    pub lines: Vec<LineRange>,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum DiffSide {
    Head,
    Base,
}

/// A symbol the diff did not change that depends on one it did.
#[derive(SimpleObject)]
pub struct DownstreamSymbol {
    pub symbol: SymbolRef,
    pub depth: i32,
    pub confidence: Confidence,
    /// From this symbol to a changed symbol.
    pub path: Vec<EvidenceStep>,
    /// Revision the chain comes from: `BASE` for dependents of removed
    /// code, whose line numbers refer to the base revision.
    pub revision: DiffSide,
}

#[derive(SimpleObject)]
pub struct DiffSummary {
    pub files_changed: i32,
    pub files_added: i32,
    pub files_removed: i32,
    pub files_modified: i32,
    pub files_renamed: i32,
    /// Functions and methods, tests excluded.
    pub functions_added: i32,
    pub functions_removed: i32,
    pub functions_modified: i32,
    pub tests_added: i32,
    pub tests_removed: i32,
    pub tests_modified: i32,
    /// Structs, enums and traits.
    pub types_added: i32,
    pub types_removed: i32,
    pub types_modified: i32,
    pub signatures_changed: i32,
    pub moved: i32,
    /// Symbols where only whitespace or comments changed.
    pub cosmetic: i32,
    pub downstream_symbols: i32,
    pub possible_symbols: i32,
    pub affected_modules: i32,
    pub affected_files: i32,
    pub affected_tests: i32,
    pub untested_changes: i32,
}

#[derive(SimpleObject)]
pub struct Revision {
    /// As requested (`main`, `HEAD~2`, ...) or `working tree`.
    pub label: String,
    /// `null` for the working tree.
    pub sha: Option<String>,
}

#[derive(SimpleObject)]
pub struct GitImpactReport {
    pub base: Revision,
    pub head: Revision,
    pub files: Vec<ChangedFile>,
    pub symbols: Vec<SymbolChange>,
    /// Touched by the diff, but only whitespace or comments changed.
    pub cosmetic: Vec<SymbolRef>,
    pub max_depth: i32,
    pub include_ambiguous: bool,
    pub downstream: Vec<DownstreamSymbol>,
    pub affected_files: Vec<AffectedGroup>,
    pub affected_modules: Vec<AffectedGroup>,
    pub tests: Vec<ID>,
    /// Modified functions and methods that no test reaches.
    pub untested: Vec<SymbolRef>,
    pub truncated: bool,
    pub summary: DiffSummary,
    /// Time to analyse both revisions and compare them.
    pub analysis_ms: f64,
}

impl From<diff::DiffReport> for GitImpactReport {
    fn from(r: diff::DiffReport) -> Self {
        let s = r.summary;
        Self {
            base: Revision {
                label: r.base.label,
                sha: r.base.sha,
            },
            head: Revision {
                label: r.head.label,
                sha: r.head.sha,
            },
            files: r
                .files
                .into_iter()
                .map(|f| ChangedFile {
                    status: f.status.into(),
                    path: f.path,
                    old_path: f.old_path,
                    rust: f.rust,
                    hunks: f
                        .hunks
                        .into_iter()
                        .map(|h| Hunk {
                            old_start: int(h.old_start),
                            old_lines: int(h.old_lines),
                            new_start: int(h.new_start),
                            new_lines: int(h.new_lines),
                        })
                        .collect(),
                })
                .collect(),
            symbols: r
                .symbols
                .into_iter()
                .map(|c| SymbolChange {
                    change: match c.change {
                        diff::ChangeKind::Modified => ChangeKind::Modified,
                        diff::ChangeKind::Added => ChangeKind::Added,
                        diff::ChangeKind::Removed => ChangeKind::Removed,
                        diff::ChangeKind::Moved => ChangeKind::Moved,
                    },
                    symbol: c.symbol.into(),
                    previous: c.previous.map(Into::into),
                    signature: c.signature.map(|s| SignatureChange {
                        before: s.before,
                        after: s.after,
                    }),
                    lines: c
                        .lines
                        .into_iter()
                        .map(|l| LineRange {
                            start: int(l.start),
                            end: int(l.end),
                        })
                        .collect(),
                })
                .collect(),
            cosmetic: r.cosmetic.into_iter().map(Into::into).collect(),
            max_depth: int(r.impact.max_depth),
            include_ambiguous: r.impact.include_ambiguous,
            downstream: r
                .impact
                .downstream
                .into_iter()
                .map(|d| {
                    let a = AffectedSymbol::from(d.affected);
                    DownstreamSymbol {
                        symbol: a.symbol,
                        depth: a.depth,
                        confidence: a.confidence,
                        path: a.path,
                        revision: match d.revision {
                            diff::Side::Head => DiffSide::Head,
                            diff::Side::Base => DiffSide::Base,
                        },
                    }
                })
                .collect(),
            affected_files: r.impact.files.into_iter().map(Into::into).collect(),
            affected_modules: r.impact.modules.into_iter().map(Into::into).collect(),
            tests: r
                .impact
                .tests
                .into_iter()
                .map(|t| ID(t.to_string()))
                .collect(),
            untested: r.impact.untested.into_iter().map(Into::into).collect(),
            truncated: r.impact.truncated,
            summary: DiffSummary {
                files_changed: int(s.files_changed),
                files_added: int(s.files_added),
                files_removed: int(s.files_removed),
                files_modified: int(s.files_modified),
                files_renamed: int(s.files_renamed),
                functions_added: int(s.functions_added),
                functions_removed: int(s.functions_removed),
                functions_modified: int(s.functions_modified),
                tests_added: int(s.tests_added),
                tests_removed: int(s.tests_removed),
                tests_modified: int(s.tests_modified),
                types_added: int(s.types_added),
                types_removed: int(s.types_removed),
                types_modified: int(s.types_modified),
                signatures_changed: int(s.signatures_changed),
                moved: int(s.moved),
                cosmetic: int(s.cosmetic),
                downstream_symbols: int(s.downstream_symbols),
                possible_symbols: int(s.possible_symbols),
                affected_modules: int(s.affected_modules),
                affected_files: int(s.affected_files),
                affected_tests: int(s.affected_tests),
                untested_changes: int(s.untested_changes),
            },
            analysis_ms: r.stats.total_ms,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum RefKind {
    Branch,
    RemoteBranch,
    Tag,
}

#[derive(SimpleObject)]
pub struct GitRef {
    pub name: String,
    pub kind: RefKind,
    /// Commit the ref points to.
    pub sha: String,
    pub date: String,
    pub subject: String,
}

impl From<git::GitRef> for GitRef {
    fn from(r: git::GitRef) -> Self {
        Self {
            name: r.name,
            kind: match r.kind {
                git::RefKind::Branch => RefKind::Branch,
                git::RefKind::RemoteBranch => RefKind::RemoteBranch,
                git::RefKind::Tag => RefKind::Tag,
            },
            sha: r.sha,
            date: r.date,
            subject: r.subject,
        }
    }
}

#[derive(SimpleObject)]
pub struct Commit {
    pub sha: String,
    pub date: String,
    pub subject: String,
}

/// Revisions available for `gitImpact`, read from the repository on the
/// server.
#[derive(SimpleObject)]
pub struct GitRefs {
    /// Checked-out branch, `null` when detached.
    pub current_branch: Option<String>,
    pub head_sha: Option<String>,
    /// Most recently committed first.
    pub refs: Vec<GitRef>,
    /// Recent commits reachable from `HEAD`.
    pub commits: Vec<Commit>,
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
