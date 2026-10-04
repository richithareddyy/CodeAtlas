//! `codeatlas index` and `codeatlas query`: commands backed by Neo4j.

use std::fmt::Write as _;

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use codeatlas_analyzer::graph::{architecture, impact, impact::ImpactOptions};
use codeatlas_analyzer::model::SymbolId;

use crate::views::{self, LevelArg};
use codeatlas_store::{
    AggregateDependency, Direction, GraphEdge, GraphStore, IndexMode, Prepared, RepositoryNode,
    StateDir, StoreConfig, SymbolNode, Traversal,
};

#[derive(Subcommand)]
pub enum Query {
    /// List indexed repositories.
    Repos,
    /// Search symbols by name or qualified name (prefix match).
    Search {
        text: String,
        /// Restrict to kinds: module, struct, enum, trait, function, method.
        #[arg(long)]
        kind: Vec<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show one symbol. Accepts an ID, qualified name, `Type::method` or unique name.
    Symbol { symbol: String },
    /// Functions and methods that call the symbol.
    Callers {
        symbol: String,
        #[arg(long, default_value_t = 1)]
        depth: u32,
    },
    /// Functions and methods the symbol calls.
    Callees {
        symbol: String,
        #[arg(long, default_value_t = 1)]
        depth: u32,
    },
    /// Tests that reach the symbol through resolved calls.
    Tests {
        symbol: String,
        #[arg(long, default_value_t = 5)]
        depth: u32,
    },
    /// Shortest dependency path from one symbol to another.
    Path {
        from: String,
        to: String,
        #[arg(long, default_value_t = 10)]
        max_depth: u32,
    },
    /// Files that depend on a file.
    FileDependents { path: String },
    /// Files a file depends on.
    FileDependencies { path: String },
    /// Modules that depend on a module.
    ModuleDependents { module: String },
    /// Modules a module depends on.
    ModuleDependencies { module: String },
    /// What could be affected by changing a symbol (or every symbol in a file), and why.
    Impact {
        /// Symbol to change; types, traits and modules include their members.
        #[arg(required_unless_present = "file")]
        symbol: Option<String>,
        /// Treat every symbol defined in this file as changed.
        #[arg(long, conflicts_with = "symbol")]
        file: Option<String>,
        #[arg(long, default_value_t = 8)]
        depth: u32,
        /// Also follow ambiguous calls; results are marked as possible.
        #[arg(long)]
        include_ambiguous: bool,
        /// Affected symbols listed in text output.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Circular dependencies with the edges that form them.
    Cycles {
        #[arg(long, value_enum, default_value_t = LevelArg::Module)]
        level: LevelArg,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Most central symbols, files or modules (betweenness, fan-in, fan-out).
    Hotspots {
        #[arg(long, value_enum, default_value_t = LevelArg::Function)]
        level: LevelArg,
        #[arg(long, default_value_t = 15)]
        limit: usize,
    },
    /// Dependency layers, foundations first.
    Layers {
        #[arg(long, value_enum, default_value_t = LevelArg::Module)]
        level: LevelArg,
    },
}

pub async fn connect() -> Result<GraphStore> {
    let config = StoreConfig::from_env()?;
    let store = GraphStore::connect(&config)
        .await
        .with_context(|| format!("cannot connect to Neo4j at {}", config.uri))?;
    store.ensure_schema().await?;
    Ok(store)
}

pub async fn index(prepared: Prepared, states: &StateDir, full: bool) -> Result<String> {
    let store = connect().await?;
    let name = prepared.analysis.repository.name.clone();
    let report = store.index_prepared(prepared, states, full).await?;
    let summary = &report.summary;
    let stats = store.graph_stats(&summary.repo_id).await?;
    let mut out = String::new();
    writeln!(out, "Indexed     {name} ({})", summary.repo_id)?;
    let r = &report.reuse;
    match report.mode {
        IndexMode::Incremental => writeln!(
            out,
            "Mode        incremental: {} of {} Rust files changed ({} added, {} removed); {} parsed, {} reused",
            r.changed + r.added,
            r.files,
            r.added,
            r.removed,
            r.parsed,
            r.reused
        )?,
        IndexMode::Full(reason) => writeln!(out, "Mode        full ({})", reason.describe())?,
    }
    writeln!(
        out,
        "Graph       {} nodes, {} relationships",
        summary.nodes, summary.relationships
    )?;
    let d = &report.delta;
    writeln!(
        out,
        "Written     nodes +{} -{} ~{}, relationships +{} -{} ~{}, in {:.0} ms",
        d.nodes_added,
        d.nodes_removed,
        d.nodes_changed,
        d.relationships_added,
        d.relationships_removed,
        d.relationships_changed,
        summary.write_ms
    )?;
    writeln!(out, "Analysis    {:.0} ms", report.analysis_ms)?;
    if !report.state_saved {
        writeln!(
            out,
            "Note        could not save incremental state in {}; the next run indexes in full",
            states.dir().display()
        )?;
    }
    let join = |m: &std::collections::BTreeMap<String, i64>| {
        m.iter()
            .map(|(k, v)| format!("{k} {v}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    writeln!(out, "Labels      {}", join(&stats.labels))?;
    write!(out, "Relations   {}", join(&stats.relationships))?;
    Ok(out)
}

pub async fn remove(key: &str, states: &StateDir) -> Result<String> {
    let store = connect().await?;
    let repo = store.repository(key).await?;
    let deleted = store.delete_repository(&repo.id).await?;
    states.remove(&repo.id)?;
    Ok(format!(
        "Removed {} ({}): {deleted} nodes",
        repo.name, repo.id
    ))
}

pub async fn query(repo: Option<&str>, json: bool, query: Query) -> Result<String> {
    let store = connect().await?;
    if let Query::Repos = query {
        let repos = store.repositories().await?;
        return if json {
            Ok(serde_json::to_string_pretty(&repos)?)
        } else {
            Ok(render_repos(&repos))
        };
    }
    let repo = select_repository(&store, repo).await?;
    let r = repo.id.as_str();

    let render = |value: &dyn erased::Render| -> Result<String> {
        if json {
            value.json()
        } else {
            Ok(value.text())
        }
    };

    match query {
        Query::Repos => unreachable!("handled above"),
        Query::Search { text, kind, limit } => {
            let kinds = (!kind.is_empty()).then_some(kind.as_slice());
            let hits = store.search(r, &text, kinds, limit).await?;
            render(&hits)
        }
        Query::Symbol { symbol } => {
            let symbol = store.find_symbol(r, &symbol).await?;
            render(&symbol)
        }
        Query::Callers { symbol, depth } => {
            let id = callable(&store, r, &symbol).await?;
            render(&store.callers(r, &id, depth).await?)
        }
        Query::Callees { symbol, depth } => {
            let id = callable(&store, r, &symbol).await?;
            render(&store.callees(r, &id, depth).await?)
        }
        Query::Tests { symbol, depth } => {
            let id = callable(&store, r, &symbol).await?;
            render(&store.related_tests(r, &id, depth).await?)
        }
        Query::Path {
            from,
            to,
            max_depth,
        } => {
            let from = callable(&store, r, &from).await?;
            let to = callable(&store, r, &to).await?;
            render(&store.shortest_path(r, &from, &to, max_depth).await?)
        }
        Query::FileDependents { path } => render(&Aggregate(
            store
                .file_dependencies(r, &path, Direction::Dependents)
                .await?,
        )),
        Query::FileDependencies { path } => render(&Aggregate(
            store
                .file_dependencies(r, &path, Direction::Dependencies)
                .await?,
        )),
        Query::ModuleDependents { module } => {
            let id = store.find_symbol(r, &module).await?.id;
            render(&Aggregate(
                store
                    .module_dependencies(r, &id, Direction::Dependents)
                    .await?,
            ))
        }
        Query::ModuleDependencies { module } => {
            let id = store.find_symbol(r, &module).await?.id;
            render(&Aggregate(
                store
                    .module_dependencies(r, &id, Direction::Dependencies)
                    .await?,
            ))
        }
        Query::Impact {
            symbol,
            file,
            depth,
            include_ambiguous,
            limit,
        } => {
            if depth == 0 || depth > store.limits().max_depth {
                bail!("--depth must be between 1 and {}", store.limits().max_depth);
            }
            let options = ImpactOptions {
                max_depth: depth,
                include_ambiguous,
                ..Default::default()
            };
            let target = match &symbol {
                Some(text) => Some(callable(&store, r, text).await?),
                None => None,
            };
            let graph = store.load_graph(r).await?;
            let report = match (target, file) {
                (Some(id), _) => {
                    impact::impact_of_symbol(&graph, &SymbolId::from_stored(id), options)?
                }
                (None, Some(path)) => impact::impact_of_file(&graph, &path, options)?,
                (None, None) => bail!("give a symbol or --file"),
            };
            output(json, &report, || views::impact(&report, limit))
        }
        Query::Cycles { level, limit } => {
            let graph = store.load_graph(r).await?;
            let found = architecture::cycles(&graph, level.into());
            output(json, &found, || views::cycles(&found, level.into(), limit))
        }
        Query::Hotspots { level, limit } => {
            let graph = store.load_graph(r).await?;
            let spots = architecture::hotspots(&graph, level.into(), limit);
            output(json, &spots, || views::hotspots(&spots))
        }
        Query::Layers { level } => {
            let graph = store.load_graph(r).await?;
            let found = architecture::layers(&graph, level.into());
            output(json, &found, || views::layers(&found))
        }
    }
}

fn output<T: serde::Serialize>(
    json: bool,
    value: &T,
    text: impl FnOnce() -> String,
) -> Result<String> {
    if json {
        Ok(serde_json::to_string_pretty(value)?)
    } else {
        Ok(text())
    }
}

/// Resolves a symbol argument, preferring functions and methods when the
/// text is ambiguous (call-graph queries are about callables).
async fn callable(store: &GraphStore, repo: &str, text: &str) -> Result<String> {
    Ok(store
        .find_symbol_preferring(repo, text, &["function", "method"])
        .await?
        .id)
}

/// `--repo` if given; otherwise the only indexed repository.
async fn select_repository(store: &GraphStore, key: Option<&str>) -> Result<RepositoryNode> {
    if let Some(key) = key {
        return Ok(store.repository(key).await?);
    }
    let mut repos = store.repositories().await?;
    match repos.len() {
        0 => bail!("no repositories are indexed; run `codeatlas index <source>` first"),
        1 => Ok(repos.remove(0)),
        _ => bail!(
            "several repositories are indexed; choose one with --repo:\n{}",
            render_repos(&repos)
        ),
    }
}

fn render_repos(repos: &[RepositoryNode]) -> String {
    if repos.is_empty() {
        return "no repositories indexed".into();
    }
    let id_width = repos.iter().map(|r| r.id.len()).max().unwrap_or(0);
    let name_width = repos.iter().map(|r| r.name.len()).max().unwrap_or(0);
    repos
        .iter()
        .map(|r| {
            let revision = match (&r.branch, &r.indexed_sha) {
                (Some(b), Some(sha)) => format!("{b} @ {}", &sha[..sha.len().min(10)]),
                (None, Some(sha)) => sha[..sha.len().min(10)].to_string(),
                _ => "no commit".into(),
            };
            format!(
                "{:<id_width$}  {:<name_width$}  {:<22} {} files, {} LOC, indexed {}",
                r.id, r.name, revision, r.source_files, r.loc, r.indexed_at
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn location(s: &SymbolNode) -> String {
    format!("{}:{}", s.file, s.start_line)
}

fn edge_text(e: &GraphEdge) -> String {
    let lines = e
        .lines
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    match &e.resolution {
        Some(how) => format!("{} -[{} {how} @{lines}]-> {}", e.from, e.kind, e.to),
        None => format!("{} -[{} @{lines}]-> {}", e.from, e.kind, e.to),
    }
}

/// A list of file or module dependencies.
struct Aggregate(Vec<AggregateDependency>);

/// Uniform text / JSON rendering of query results.
mod erased {
    use super::*;
    use codeatlas_store::{DependencyPath, RelatedTest};
    use serde::Serialize;

    pub trait Render {
        fn text(&self) -> String;
        fn json(&self) -> Result<String>;
    }

    fn to_json<T: Serialize + ?Sized>(value: &T) -> Result<String> {
        Ok(serde_json::to_string_pretty(value)?)
    }

    impl Render for Vec<SymbolNode> {
        fn text(&self) -> String {
            if self.is_empty() {
                return "no matches".into();
            }
            self.iter()
                .map(|s| format!("{:<8} {:<60} {}", s.kind, s.qualified_name, location(s)))
                .collect::<Vec<_>>()
                .join("\n")
        }
        fn json(&self) -> Result<String> {
            to_json(self)
        }
    }

    impl Render for SymbolNode {
        fn text(&self) -> String {
            let mut out = format!(
                "{}\n  kind        {}\n  location    {}-{}\n  crate       {}\n  visibility  {}\n",
                self.id,
                self.kind,
                location(self),
                self.end_line,
                self.crate_name,
                self.visibility
            );
            if let Some(signature) = &self.signature {
                out.push_str(&format!("  signature   {signature}\n"));
            }
            if self.is_test {
                out.push_str("  test        yes\n");
            }
            for call in &self.unresolved_calls {
                out.push_str(&format!("  unresolved  {call}\n"));
            }
            out.trim_end().to_string()
        }
        fn json(&self) -> Result<String> {
            to_json(self)
        }
    }

    impl Render for Traversal {
        fn text(&self) -> String {
            let label = match self.direction {
                Direction::Dependents => "Dependents",
                Direction::Dependencies => "Dependencies",
            };
            let mut out = format!(
                "{label} of {} ({}), depth <= {}\n",
                self.root.id,
                location(&self.root),
                self.max_depth
            );
            if self.nodes.is_empty() {
                out.push_str("  none\n");
            }
            let mut nodes: Vec<_> = self.nodes.iter().collect();
            nodes.sort_by(|a, b| a.depth.cmp(&b.depth).then(a.symbol.id.cmp(&b.symbol.id)));
            for node in nodes {
                out.push_str(&format!(
                    "  {}  {:<60} {}\n       via {}\n",
                    node.depth,
                    node.symbol.id,
                    location(&node.symbol),
                    edge_text(&node.via)
                ));
            }
            if self.truncated {
                out.push_str("  (truncated by query limits)\n");
            }
            out.trim_end().to_string()
        }
        fn json(&self) -> Result<String> {
            to_json(self)
        }
    }

    impl Render for Vec<RelatedTest> {
        fn text(&self) -> String {
            if self.is_empty() {
                return "no tests reach this symbol through resolved calls".into();
            }
            let mut out = String::new();
            for test in self {
                out.push_str(&format!(
                    "{}  (depth {}, {})\n",
                    test.test.id,
                    test.depth,
                    location(&test.test)
                ));
                for edge in &test.path {
                    out.push_str(&format!("    {}\n", edge_text(edge)));
                }
            }
            out.trim_end().to_string()
        }
        fn json(&self) -> Result<String> {
            to_json(self)
        }
    }

    impl Render for Option<DependencyPath> {
        fn text(&self) -> String {
            let Some(path) = self else {
                return "no dependency path within the depth limit".into();
            };
            let mut out = String::new();
            for (i, node) in path.nodes.iter().enumerate() {
                out.push_str(&format!("{}  ({})\n", node.id, location(node)));
                if let Some(edge) = path.edges.get(i) {
                    let lines = edge
                        .lines
                        .iter()
                        .map(i64::to_string)
                        .collect::<Vec<_>>()
                        .join(",");
                    out.push_str(&format!("  └─ {} at line {lines}\n", edge.kind));
                }
            }
            out.trim_end().to_string()
        }
        fn json(&self) -> Result<String> {
            to_json(self)
        }
    }

    impl Render for Aggregate {
        fn text(&self) -> String {
            if self.0.is_empty() {
                return "none".into();
            }
            self.0
                .iter()
                .map(|d| format!("{:>4}  {:<60} via {}", d.weight, d.target, d.via.join(", ")))
                .collect::<Vec<_>>()
                .join("\n")
        }
        fn json(&self) -> Result<String> {
            to_json(&self.0)
        }
    }
}
