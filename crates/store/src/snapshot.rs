//! The graph of one repository as plain, comparable data.
//!
//! [`GraphSnapshot::new`] computes every node and relationship the writer
//! stores for an analysis, with all of their properties. A full write
//! creates the whole snapshot; an incremental write applies the
//! [`GraphDelta`] between the previous snapshot and the new one. Both paths
//! use the same rows, so they cannot disagree about what is stored.

use std::collections::{BTreeMap, HashMap};

use codeatlas_analyzer::dependencies::{derive_dependencies, locate_symbols, Dependency};
use codeatlas_analyzer::model::{EdgeKind, Symbol, SymbolKind, Visibility};
use codeatlas_analyzer::RepositoryAnalysis;
use neo4rs::BoltType;

/// A property value. Comparable, unlike the driver's types, and converted
/// to them only when written.
#[derive(Debug, Clone, PartialEq)]
pub enum Prop {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Prop>),
}

impl From<&Prop> for BoltType {
    fn from(prop: &Prop) -> Self {
        match prop {
            Prop::Null => BoltType::Null(neo4rs::BoltNull),
            Prop::Bool(b) => (*b).into(),
            Prop::Int(i) => (*i).into(),
            Prop::Float(f) => (*f).into(),
            Prop::Str(s) => s.as_str().into(),
            Prop::List(items) => items.iter().map(BoltType::from).collect::<Vec<_>>().into(),
        }
    }
}

impl From<&str> for Prop {
    fn from(s: &str) -> Self {
        Prop::Str(s.to_string())
    }
}

impl From<String> for Prop {
    fn from(s: String) -> Self {
        Prop::Str(s)
    }
}

impl From<Option<String>> for Prop {
    fn from(s: Option<String>) -> Self {
        s.map_or(Prop::Null, Prop::Str)
    }
}

impl From<bool> for Prop {
    fn from(b: bool) -> Self {
        Prop::Bool(b)
    }
}

impl From<i64> for Prop {
    fn from(i: i64) -> Self {
        Prop::Int(i)
    }
}

impl From<u32> for Prop {
    fn from(i: u32) -> Self {
        Prop::Int(i64::from(i))
    }
}

impl<T: Into<Prop>> From<Vec<T>> for Prop {
    fn from(items: Vec<T>) -> Self {
        Prop::List(items.into_iter().map(Into::into).collect())
    }
}

/// Properties of a node or relationship. Every property the writer knows
/// is present, `Null` when absent, so that updates also remove values.
pub type Props = BTreeMap<&'static str, Prop>;

pub fn to_bolt(props: &Props) -> BoltType {
    BoltType::from(
        props
            .iter()
            .map(|(k, v)| (*k, BoltType::from(v)))
            .collect::<HashMap<_, _>>(),
    )
}

/// One end of a relationship.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NodeRef {
    Repository,
    Crate(String),
    File(String),
    Symbol(String),
}

impl NodeRef {
    pub fn key(&self) -> Option<&str> {
        match self {
            NodeRef::Repository => None,
            NodeRef::Crate(k) | NodeRef::File(k) | NodeRef::Symbol(k) => Some(k),
        }
    }

    pub fn kind(&self) -> NodeKind {
        match self {
            NodeRef::Repository => NodeKind::Repository,
            NodeRef::Crate(_) => NodeKind::Crate,
            NodeRef::File(_) => NodeKind::File,
            NodeRef::Symbol(_) => NodeKind::Symbol,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NodeKind {
    Repository,
    Crate,
    File,
    Symbol,
}

impl NodeKind {
    /// Cypher pattern matching the node whose key is `row.<field>`.
    pub fn pattern(self, var: &str, field: &str) -> String {
        match self {
            NodeKind::Repository => format!("({var}:Repository {{id: $repo}})"),
            NodeKind::Crate => format!("({var}:Crate {{repo_id: $repo, id: row.{field}}})"),
            NodeKind::File => format!("({var}:File {{repo_id: $repo, path: row.{field}}})"),
            NodeKind::Symbol => format!("({var}:Symbol {{repo_id: $repo, id: row.{field}}})"),
        }
    }
}

/// Relationships are unique per type and endpoints: the analyzer
/// aggregates parallel edges (e.g. several call sites) into one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelKey {
    pub rel: &'static str,
    pub from: NodeRef,
    pub to: NodeRef,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SymbolNode {
    pub kind: SymbolKind,
    pub is_test: bool,
    pub props: Props,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GraphSnapshot {
    pub repository: Props,
    pub crates: BTreeMap<String, Props>,
    pub files: BTreeMap<String, Props>,
    pub symbols: BTreeMap<String, SymbolNode>,
    pub relationships: BTreeMap<RelKey, Props>,
}

impl GraphSnapshot {
    /// Everything stored for `a`. `index_token` identifies this write (see
    /// `incremental`); `indexed_at` is the write time.
    pub fn new(a: &RepositoryAnalysis, index_token: &str, indexed_at: &str) -> Self {
        let mut s = GraphSnapshot {
            repository: repository_props(a, index_token, indexed_at),
            ..Default::default()
        };
        s.add_crates(a);
        s.add_files(a);
        s.add_symbols(a);
        s.add_structure(a);
        s.add_edges(a);
        s.add_candidates(a);
        s.add_dependencies(a);
        s
    }

    pub fn node_count(&self) -> usize {
        1 + self.crates.len() + self.files.len() + self.symbols.len()
    }

    fn rel(&mut self, rel: &'static str, from: NodeRef, to: NodeRef, props: Props) {
        self.relationships.insert(RelKey { rel, from, to }, props);
    }

    fn add_crates(&mut self, a: &RepositoryAnalysis) {
        for c in &a.crates {
            let id = crate_id(&c.package, &c.name);
            self.crates.insert(
                id.clone(),
                Props::from([
                    ("id", id.as_str().into()),
                    ("name", c.name.as_str().into()),
                    ("package", c.package.as_str().into()),
                    ("target_kind", format!("{:?}", c.kind).to_lowercase().into()),
                    ("root_file", c.root_file.as_str().into()),
                ]),
            );
            self.rel(
                "CONTAINS",
                NodeRef::Repository,
                NodeRef::Crate(id),
                Props::new(),
            );
        }
    }

    fn add_files(&mut self, a: &RepositoryAnalysis) {
        for f in &a.files {
            self.files.insert(
                f.path.clone(),
                Props::from([
                    ("path", f.path.as_str().into()),
                    ("crate", f.crate_name.as_str().into()),
                    ("loc", f.loc.into()),
                    ("syntax_errors", f.syntax_errors.into()),
                    ("content_hash", f.content_hash.as_str().into()),
                ]),
            );
        }
    }

    fn add_symbols(&mut self, a: &RepositoryAnalysis) {
        let mut unresolved: HashMap<&str, Vec<Prop>> = HashMap::new();
        for call in &a.resolution.unresolved_calls {
            unresolved
                .entry(call.caller.as_str())
                .or_default()
                .push(format!("{}|{}|{}", call.line, call.callee, call.reason.as_str()).into());
        }
        let locations = locate_symbols(&a.files);
        for file in &a.files {
            for symbol in &file.symbols {
                let props = symbol_props(
                    symbol,
                    &file.crate_name,
                    locations.get(&symbol.id).map(|l| l.module.as_str()),
                    unresolved.remove(symbol.id.as_str()).unwrap_or_default(),
                );
                self.symbols.insert(
                    symbol.id.as_str().to_string(),
                    SymbolNode {
                        kind: symbol.kind,
                        is_test: symbol.is_test,
                        props,
                    },
                );
            }
        }
    }

    /// CONTAINS (crate → root module, module → submodule, module → file)
    /// and DEFINES (owner → item).
    fn add_structure(&mut self, a: &RepositoryAnalysis) {
        let kinds: HashMap<&str, SymbolKind> = a
            .files
            .iter()
            .flat_map(|f| &f.symbols)
            .map(|s| (s.id.as_str(), s.kind))
            .collect();
        for c in &a.crates {
            let module = a
                .files
                .iter()
                .find(|f| f.path == c.root_file)
                .and_then(|f| f.module.as_ref());
            if let Some(module) = module {
                self.rel(
                    "CONTAINS",
                    NodeRef::Crate(crate_id(&c.package, &c.name)),
                    NodeRef::Symbol(module.to_string()),
                    Props::new(),
                );
            }
        }
        for f in &a.files {
            if let Some(module) = &f.module {
                self.rel(
                    "CONTAINS",
                    NodeRef::Symbol(module.to_string()),
                    NodeRef::File(f.path.clone()),
                    Props::new(),
                );
            }
        }
        for symbol in a.files.iter().flat_map(|f| &f.symbols) {
            let Some(parent) = &symbol.parent else {
                continue;
            };
            let both_modules = symbol.kind == SymbolKind::Module
                && kinds.get(parent.as_str()) == Some(&SymbolKind::Module);
            self.rel(
                if both_modules { "CONTAINS" } else { "DEFINES" },
                NodeRef::Symbol(parent.to_string()),
                NodeRef::Symbol(symbol.id.to_string()),
                Props::new(),
            );
        }
    }

    fn add_edges(&mut self, a: &RepositoryAnalysis) {
        for edge in &a.resolution.edges {
            let rel = match edge.kind {
                EdgeKind::Calls => "CALLS",
                EdgeKind::Imports => "IMPORTS",
                EdgeKind::Implements => "IMPLEMENTS",
            };
            self.rel(
                rel,
                NodeRef::Symbol(edge.from.to_string()),
                NodeRef::Symbol(edge.to.to_string()),
                Props::from([
                    ("resolution", edge.via.as_str().into()),
                    ("lines", edge.lines.clone().into()),
                ]),
            );
        }
    }

    /// One CALLS_CANDIDATE relationship per (caller, candidate) of
    /// ambiguous calls, aggregating the lines of all such call sites.
    fn add_candidates(&mut self, a: &RepositoryAnalysis) {
        let mut aggregated: BTreeMap<(&str, &str), (Vec<u32>, &str, u32)> = BTreeMap::new();
        for call in &a.resolution.ambiguous_calls {
            for candidate in &call.candidates {
                let entry = aggregated
                    .entry((call.caller.as_str(), candidate.as_str()))
                    .or_insert((Vec::new(), call.reason.as_str(), call.candidate_count));
                entry.0.push(call.line);
            }
        }
        for ((from, to), (lines, reason, count)) in aggregated {
            self.rel(
                "CALLS_CANDIDATE",
                NodeRef::Symbol(from.to_string()),
                NodeRef::Symbol(to.to_string()),
                Props::from([
                    ("reason", reason.into()),
                    ("candidates", count.into()),
                    ("lines", lines.into()),
                ]),
            );
        }
    }

    fn add_dependencies(&mut self, a: &RepositoryAnalysis) {
        let deps = derive_dependencies(&a.files, &a.resolution.edges);
        let props = |d: &Dependency| -> Props {
            // `from|to|KIND|line` per sample edge.
            let evidence: Vec<Prop> = d
                .evidence
                .iter()
                .map(|e| format!("{}|{}|{}|{}", e.from, e.to, e.kind.as_str(), e.line).into())
                .collect();
            Props::from([
                ("weight", d.weight.into()),
                (
                    "via",
                    d.via.iter().map(|k| k.as_str()).collect::<Vec<_>>().into(),
                ),
                ("evidence", Prop::List(evidence)),
            ])
        };
        for d in &deps.files {
            self.rel(
                "DEPENDS_ON",
                NodeRef::File(d.from.clone()),
                NodeRef::File(d.to.clone()),
                props(d),
            );
        }
        for d in &deps.modules {
            self.rel(
                "DEPENDS_ON",
                NodeRef::Symbol(d.from.clone()),
                NodeRef::Symbol(d.to.clone()),
                props(d),
            );
        }
    }
}

fn repository_props(a: &RepositoryAnalysis, index_token: &str, indexed_at: &str) -> Props {
    let r = &a.repository;
    let calls = &a.resolution.stats.calls;
    Props::from([
        ("id", r.id.as_str().into()),
        ("format_version", crate::GRAPH_FORMAT_VERSION.into()),
        ("index_token", index_token.into()),
        ("name", r.name.as_str().into()),
        ("root", r.root.to_string_lossy().into_owned().into()),
        ("origin_url", r.origin_url.clone().into()),
        ("branch", r.branch.clone().into()),
        ("head_sha", r.head_sha.clone().into()),
        ("indexed_sha", r.head_sha.clone().into()),
        ("analyzed_at", r.analyzed_at.to_rfc3339().into()),
        ("indexed_at", indexed_at.into()),
        (
            "languages",
            r.languages
                .iter()
                .map(|l| format!("{:?}", l.language).to_lowercase())
                .collect::<Vec<_>>()
                .into(),
        ),
        (
            "language_files",
            r.languages
                .iter()
                .map(|l| l.files)
                .collect::<Vec<_>>()
                .into(),
        ),
        (
            "language_loc",
            r.languages
                .iter()
                .map(|l| l.loc as i64)
                .collect::<Vec<_>>()
                .into(),
        ),
        ("source_files", r.source_files.into()),
        ("loc", (r.loc as i64).into()),
        ("calls_total", calls.total.into()),
        ("calls_resolved", calls.resolved.into()),
        ("calls_ambiguous", calls.ambiguous.into()),
        ("calls_unresolved", calls.unresolved.into()),
        (
            "resolution_rate",
            calls.resolution_rate.map_or(Prop::Null, Prop::Float),
        ),
    ])
}

fn symbol_props(
    symbol: &Symbol,
    crate_name: &str,
    module: Option<&str>,
    unresolved: Vec<Prop>,
) -> Props {
    let visibility = match &symbol.visibility {
        Visibility::Public => "pub".to_string(),
        Visibility::Crate => "pub(crate)".to_string(),
        Visibility::Super => "pub(super)".to_string(),
        Visibility::Restricted(path) => format!("pub(in {path})"),
        Visibility::Private => "private".to_string(),
    };
    Props::from([
        ("id", symbol.id.as_str().into()),
        ("kind", symbol.kind.as_str().into()),
        ("module", module.map(str::to_string).into()),
        ("name", symbol.name.as_str().into()),
        ("qualified_name", symbol.qualified_name.as_str().into()),
        ("file", symbol.file.as_str().into()),
        ("crate", crate_name.into()),
        ("start_line", symbol.span.start_line.into()),
        ("end_line", symbol.span.end_line.into()),
        ("visibility", visibility.into()),
        ("signature", symbol.signature.clone().into()),
        (
            "parent_id",
            symbol
                .parent
                .as_ref()
                .map(|p| p.as_str().to_string())
                .into(),
        ),
        ("is_test", symbol.is_test.into()),
        ("cfg_test", symbol.cfg_test.into()),
        ("unresolved_calls", Prop::List(unresolved)),
    ])
}

pub(crate) fn crate_id(package: &str, name: &str) -> String {
    format!("crate:{package}:{name}")
}

/// Keys added, removed and changed between two maps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changes<K> {
    pub added: Vec<K>,
    pub removed: Vec<K>,
    pub changed: Vec<K>,
}

impl<K> Default for Changes<K> {
    fn default() -> Self {
        Self {
            added: Vec::new(),
            removed: Vec::new(),
            changed: Vec::new(),
        }
    }
}

impl<K> Changes<K> {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

fn changes<K: Ord + Clone, V: PartialEq>(old: &BTreeMap<K, V>, new: &BTreeMap<K, V>) -> Changes<K> {
    let mut out = Changes::default();
    for (key, value) in new {
        match old.get(key) {
            None => out.added.push(key.clone()),
            Some(previous) if previous != value => out.changed.push(key.clone()),
            Some(_) => {}
        }
    }
    out.removed = old
        .keys()
        .filter(|k| !new.contains_key(*k))
        .cloned()
        .collect();
    out
}

/// What turns one snapshot into another. The repository node is always
/// rewritten (its timestamps change on every write).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GraphDelta {
    pub crates: Changes<String>,
    pub files: Changes<String>,
    pub symbols: Changes<String>,
    pub relationships: Changes<RelKey>,
}

impl GraphDelta {
    pub fn between(old: &GraphSnapshot, new: &GraphSnapshot) -> Self {
        Self {
            crates: changes(&old.crates, &new.crates),
            files: changes(&old.files, &new.files),
            symbols: changes(&old.symbols, &new.symbols),
            relationships: changes(&old.relationships, &new.relationships),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.crates.is_empty()
            && self.files.is_empty()
            && self.symbols.is_empty()
            && self.relationships.is_empty()
    }

    pub fn nodes_added(&self) -> usize {
        self.crates.added.len() + self.files.added.len() + self.symbols.added.len()
    }

    pub fn nodes_removed(&self) -> usize {
        self.crates.removed.len() + self.files.removed.len() + self.symbols.removed.len()
    }

    pub fn nodes_changed(&self) -> usize {
        self.crates.changed.len() + self.files.changed.len() + self.symbols.changed.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(value: i64) -> Props {
        Props::from([("weight", Prop::Int(value))])
    }

    #[test]
    fn changes_are_computed_per_key() {
        let old = BTreeMap::from([("a", props(1)), ("b", props(2)), ("c", props(3))]);
        let new = BTreeMap::from([("a", props(1)), ("b", props(20)), ("d", props(4))]);
        let c = changes(&old, &new);
        assert_eq!(
            (c.added, c.removed, c.changed),
            (vec!["d"], vec!["c"], vec!["b"])
        );
        assert!(changes(&old, &old).is_empty());
    }

    #[test]
    fn props_convert_to_driver_values() {
        let value = BoltType::from(&Prop::List(vec![Prop::Int(1), Prop::Null]));
        assert!(matches!(value, BoltType::List(ref l) if l.len() == 2));
        assert!(matches!(BoltType::from(&Prop::Null), BoltType::Null(_)));
    }
}
