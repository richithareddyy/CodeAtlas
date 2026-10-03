//! Persists a [`RepositoryAnalysis`] as a code graph.
//!
//! A repository is written as a whole: its previous subgraph is deleted in
//! batches, then nodes and relationships are created with batched `UNWIND`
//! statements inside one transaction, so readers never observe a partially
//! written graph. Labels and relationship types are fixed strings chosen in
//! code, never taken from input.

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use codeatlas_analyzer::dependencies::{derive_dependencies, Dependency};
use codeatlas_analyzer::model::{EdgeKind, Symbol, SymbolKind, Visibility};
use codeatlas_analyzer::RepositoryAnalysis;
use neo4rs::{query, BoltType, Graph, Txn};

use crate::error::Result;

/// Rows sent per `UNWIND` statement.
const BATCH_SIZE: usize = 1_000;
/// Nodes deleted per statement when clearing a repository.
const DELETE_BATCH: i64 = 5_000;

#[derive(Debug, Clone, PartialEq)]
pub struct IndexSummary {
    pub repo_id: String,
    pub nodes: usize,
    pub relationships: usize,
    pub write_ms: f64,
}

pub async fn write_repository(
    graph: &Graph,
    analysis: &RepositoryAnalysis,
) -> Result<IndexSummary> {
    let started = Instant::now();
    let repo = analysis.repository.id.as_str();
    delete_repository(graph, repo).await?;

    let mut writer = Writer {
        txn: graph.start_txn().await?,
        repo,
        nodes: 0,
        relationships: 0,
    };
    writer.repository(analysis).await?;
    writer.crates(analysis).await?;
    writer.files(analysis).await?;
    writer.symbols(analysis).await?;
    writer.structure(analysis).await?;
    writer.edges(analysis).await?;
    writer.candidates(analysis).await?;
    writer.dependencies(analysis).await?;

    let (nodes, relationships) = (writer.nodes, writer.relationships);
    writer.txn.commit().await?;

    let summary = IndexSummary {
        repo_id: repo.to_string(),
        nodes,
        relationships,
        write_ms: started.elapsed().as_secs_f64() * 1000.0,
    };
    tracing::info!(
        repo = %summary.repo_id,
        nodes = summary.nodes,
        relationships = summary.relationships,
        ms = summary.write_ms,
        "graph written"
    );
    Ok(summary)
}

/// Removes a repository and everything indexed for it. Returns the number
/// of deleted nodes.
pub async fn delete_repository(graph: &Graph, repo: &str) -> Result<u64> {
    let mut deleted = 0u64;
    for label in ["Symbol", "File", "Crate"] {
        let cypher = format!(
            "MATCH (n:{label} {{repo_id: $repo}}) WITH n LIMIT $batch \
             DETACH DELETE n RETURN count(*) AS deleted"
        );
        loop {
            let mut rows = graph
                .execute(
                    query(&cypher)
                        .param("repo", repo)
                        .param("batch", DELETE_BATCH),
                )
                .await?;
            let batch: i64 = match rows.next().await? {
                Some(row) => row.get("deleted")?,
                None => 0,
            };
            deleted += batch as u64;
            if batch < DELETE_BATCH {
                break;
            }
        }
    }
    let mut rows = graph
        .execute(
            query("MATCH (r:Repository {id: $repo}) DETACH DELETE r RETURN count(*) AS deleted")
                .param("repo", repo),
        )
        .await?;
    if let Some(row) = rows.next().await? {
        deleted += row.get::<i64>("deleted")? as u64;
    }
    Ok(deleted)
}

struct Writer<'a> {
    txn: Txn,
    repo: &'a str,
    nodes: usize,
    relationships: usize,
}

impl Writer<'_> {
    async fn unwind(&mut self, cypher: &str, rows: Vec<BoltType>) -> Result<usize> {
        let count = rows.len();
        for chunk in rows.chunks(BATCH_SIZE) {
            self.txn
                .run(
                    query(cypher)
                        .param("repo", self.repo)
                        .param("rows", chunk.to_vec()),
                )
                .await?;
        }
        Ok(count)
    }

    async fn create_nodes(&mut self, cypher: &str, rows: Vec<BoltType>) -> Result<()> {
        self.nodes += self.unwind(cypher, rows).await?;
        Ok(())
    }

    async fn create_relationships(&mut self, cypher: &str, rows: Vec<BoltType>) -> Result<()> {
        self.relationships += self.unwind(cypher, rows).await?;
        Ok(())
    }

    async fn repository(&mut self, a: &RepositoryAnalysis) -> Result<()> {
        let r = &a.repository;
        let calls = &a.resolution.stats.calls;
        let languages = |f: fn(&codeatlas_analyzer::ingest::LanguageStats) -> BoltType| {
            r.languages.iter().map(f).collect::<Vec<_>>()
        };
        let props = map([
            ("id", r.id.clone().into()),
            ("name", r.name.clone().into()),
            ("root", r.root.to_string_lossy().into_owned().into()),
            ("origin_url", r.origin_url.clone().into()),
            ("branch", r.branch.clone().into()),
            ("head_sha", r.head_sha.clone().into()),
            ("indexed_sha", r.head_sha.clone().into()),
            ("analyzed_at", r.analyzed_at.to_rfc3339().into()),
            ("indexed_at", chrono::Utc::now().to_rfc3339().into()),
            (
                "languages",
                languages(|l| format!("{:?}", l.language).to_lowercase().into()).into(),
            ),
            (
                "language_files",
                languages(|l| i64::from(l.files).into()).into(),
            ),
            ("language_loc", languages(|l| (l.loc as i64).into()).into()),
            ("source_files", i64::from(r.source_files).into()),
            ("loc", (r.loc as i64).into()),
            ("calls_total", i64::from(calls.total).into()),
            ("calls_resolved", i64::from(calls.resolved).into()),
            ("calls_ambiguous", i64::from(calls.ambiguous).into()),
            ("calls_unresolved", i64::from(calls.unresolved).into()),
            ("resolution_rate", calls.resolution_rate.into()),
        ]);
        self.txn
            .run(query("CREATE (r:Repository) SET r = $props").param("props", props))
            .await?;
        self.nodes += 1;
        Ok(())
    }

    async fn crates(&mut self, a: &RepositoryAnalysis) -> Result<()> {
        let rows = a
            .crates
            .iter()
            .map(|c| {
                map([
                    ("id", crate_id(&c.package, &c.name).into()),
                    ("name", c.name.clone().into()),
                    ("package", c.package.clone().into()),
                    ("target_kind", format!("{:?}", c.kind).to_lowercase().into()),
                    ("root_file", c.root_file.clone().into()),
                ])
            })
            .collect();
        self.create_nodes(
            "UNWIND $rows AS row CREATE (c:Crate {repo_id: $repo}) SET c += row",
            rows,
        )
        .await?;

        let rows = a
            .crates
            .iter()
            .map(|c| map([("id", crate_id(&c.package, &c.name).into())]))
            .collect();
        self.create_relationships(
            "MATCH (r:Repository {id: $repo}) UNWIND $rows AS row \
             MATCH (c:Crate {repo_id: $repo, id: row.id}) CREATE (r)-[:CONTAINS]->(c)",
            rows,
        )
        .await
    }

    async fn files(&mut self, a: &RepositoryAnalysis) -> Result<()> {
        let rows = a
            .files
            .iter()
            .map(|f| {
                map([
                    ("path", f.path.clone().into()),
                    ("crate", f.crate_name.clone().into()),
                    ("loc", i64::from(f.loc).into()),
                    ("syntax_errors", i64::from(f.syntax_errors).into()),
                    ("content_hash", f.content_hash.clone().into()),
                ])
            })
            .collect();
        self.create_nodes(
            "UNWIND $rows AS row CREATE (f:File {repo_id: $repo}) SET f += row",
            rows,
        )
        .await
    }

    async fn symbols(&mut self, a: &RepositoryAnalysis) -> Result<()> {
        let mut unresolved: HashMap<&str, Vec<BoltType>> = HashMap::new();
        for call in &a.resolution.unresolved_calls {
            unresolved
                .entry(call.caller.as_str())
                .or_default()
                .push(format!("{}|{}|{}", call.line, call.callee, call.reason.as_str()).into());
        }

        let mut by_kind: BTreeMap<SymbolKind, Vec<BoltType>> = BTreeMap::new();
        let mut tests = Vec::new();
        for file in &a.files {
            for symbol in &file.symbols {
                let row = symbol_row(
                    symbol,
                    &file.crate_name,
                    unresolved.remove(symbol.id.as_str()),
                );
                by_kind.entry(symbol.kind).or_default().push(row);
                if symbol.is_test {
                    tests.push(map([("id", symbol.id.as_str().into())]));
                }
            }
        }
        for (kind, rows) in by_kind {
            let cypher = format!(
                "UNWIND $rows AS row CREATE (s:Symbol:{} {{repo_id: $repo}}) SET s += row",
                label(kind)
            );
            self.create_nodes(&cypher, rows).await?;
        }
        self.unwind(
            "UNWIND $rows AS row MATCH (s:Symbol {repo_id: $repo, id: row.id}) SET s:Test",
            tests,
        )
        .await?;
        Ok(())
    }

    /// CONTAINS (crate → root module, module → submodule, module → file)
    /// and DEFINES (owner → item) relationships.
    async fn structure(&mut self, a: &RepositoryAnalysis) -> Result<()> {
        let kinds: HashMap<&str, SymbolKind> = a
            .files
            .iter()
            .flat_map(|f| &f.symbols)
            .map(|s| (s.id.as_str(), s.kind))
            .collect();

        let roots = a
            .crates
            .iter()
            .filter_map(|c| {
                let file = a.files.iter().find(|f| f.path == c.root_file)?;
                let module = file.module.as_ref()?;
                Some(map([
                    ("crate", crate_id(&c.package, &c.name).into()),
                    ("module", module.as_str().into()),
                ]))
            })
            .collect();
        self.create_relationships(
            "UNWIND $rows AS row MATCH (c:Crate {repo_id: $repo, id: row.crate}) \
             MATCH (m:Symbol {repo_id: $repo, id: row.module}) CREATE (c)-[:CONTAINS]->(m)",
            roots,
        )
        .await?;

        let module_files = a
            .files
            .iter()
            .filter_map(|f| {
                let module = f.module.as_ref()?;
                Some(map([
                    ("module", module.as_str().into()),
                    ("path", f.path.clone().into()),
                ]))
            })
            .collect();
        self.create_relationships(
            "UNWIND $rows AS row MATCH (m:Symbol {repo_id: $repo, id: row.module}) \
             MATCH (f:File {repo_id: $repo, path: row.path}) CREATE (m)-[:CONTAINS]->(f)",
            module_files,
        )
        .await?;

        let (mut contains, mut defines) = (Vec::new(), Vec::new());
        for symbol in a.files.iter().flat_map(|f| &f.symbols) {
            let Some(parent) = &symbol.parent else {
                continue;
            };
            let row = pair(parent.as_str(), symbol.id.as_str());
            let both_modules = symbol.kind == SymbolKind::Module
                && kinds.get(parent.as_str()) == Some(&SymbolKind::Module);
            if both_modules {
                contains.push(row);
            } else {
                defines.push(row);
            }
        }
        self.create_relationships(&symbol_rel("CONTAINS", ""), contains)
            .await?;
        self.create_relationships(&symbol_rel("DEFINES", ""), defines)
            .await
    }

    async fn edges(&mut self, a: &RepositoryAnalysis) -> Result<()> {
        let mut by_kind: BTreeMap<EdgeKind, Vec<BoltType>> = BTreeMap::new();
        for edge in &a.resolution.edges {
            let lines: Vec<BoltType> = edge.lines.iter().map(|l| i64::from(*l).into()).collect();
            by_kind.entry(edge.kind).or_default().push(map([
                ("from", edge.from.as_str().into()),
                ("to", edge.to.as_str().into()),
                ("resolution", edge.via.as_str().into()),
                ("lines", lines.into()),
            ]));
        }
        for (kind, rows) in by_kind {
            let rel_type = match kind {
                EdgeKind::Calls => "CALLS",
                EdgeKind::Imports => "IMPORTS",
                EdgeKind::Implements => "IMPLEMENTS",
            };
            let cypher = symbol_rel(rel_type, "{resolution: row.resolution, lines: row.lines}");
            self.create_relationships(&cypher, rows).await?;
        }
        Ok(())
    }

    /// One CALLS_CANDIDATE relationship per (caller, candidate) of
    /// ambiguous calls, aggregating the lines of all such call sites.
    async fn candidates(&mut self, a: &RepositoryAnalysis) -> Result<()> {
        let mut aggregated: BTreeMap<(&str, &str), (Vec<i64>, &str, u32)> = BTreeMap::new();
        for call in &a.resolution.ambiguous_calls {
            for candidate in &call.candidates {
                let entry = aggregated
                    .entry((call.caller.as_str(), candidate.as_str()))
                    .or_insert((Vec::new(), call.reason.as_str(), call.candidate_count));
                entry.0.push(i64::from(call.line));
            }
        }
        let rows = aggregated
            .into_iter()
            .map(|((from, to), (lines, reason, count))| {
                let lines: Vec<BoltType> = lines.into_iter().map(Into::into).collect();
                map([
                    ("from", from.into()),
                    ("to", to.into()),
                    ("reason", reason.into()),
                    ("candidates", i64::from(count).into()),
                    ("lines", lines.into()),
                ])
            })
            .collect();
        let cypher = symbol_rel(
            "CALLS_CANDIDATE",
            "{reason: row.reason, candidates: row.candidates, lines: row.lines}",
        );
        self.create_relationships(&cypher, rows).await
    }

    async fn dependencies(&mut self, a: &RepositoryAnalysis) -> Result<()> {
        let deps = derive_dependencies(&a.files, &a.resolution.edges);
        let rows = |deps: &[Dependency]| -> Vec<BoltType> {
            deps.iter()
                .map(|d| {
                    let via: Vec<BoltType> = d
                        .via
                        .iter()
                        .map(|k| format!("{k:?}").to_uppercase().into())
                        .collect();
                    map([
                        ("from", d.from.clone().into()),
                        ("to", d.to.clone().into()),
                        ("weight", i64::from(d.weight).into()),
                        ("via", via.into()),
                    ])
                })
                .collect()
        };
        self.create_relationships(
            "UNWIND $rows AS row MATCH (a:File {repo_id: $repo, path: row.from}) \
             MATCH (b:File {repo_id: $repo, path: row.to}) \
             CREATE (a)-[:DEPENDS_ON {weight: row.weight, via: row.via}]->(b)",
            rows(&deps.files),
        )
        .await?;
        self.create_relationships(
            &symbol_rel("DEPENDS_ON", "{weight: row.weight, via: row.via}"),
            rows(&deps.modules),
        )
        .await
    }
}

fn symbol_row(symbol: &Symbol, crate_name: &str, unresolved: Option<Vec<BoltType>>) -> BoltType {
    let visibility = match &symbol.visibility {
        Visibility::Public => "pub".to_string(),
        Visibility::Crate => "pub(crate)".to_string(),
        Visibility::Super => "pub(super)".to_string(),
        Visibility::Restricted(path) => format!("pub(in {path})"),
        Visibility::Private => "private".to_string(),
    };
    map([
        ("id", symbol.id.as_str().into()),
        ("kind", label(symbol.kind).to_lowercase().into()),
        ("name", symbol.name.clone().into()),
        ("qualified_name", symbol.qualified_name.clone().into()),
        ("file", symbol.file.clone().into()),
        ("crate", crate_name.into()),
        ("start_line", i64::from(symbol.span.start_line).into()),
        ("end_line", i64::from(symbol.span.end_line).into()),
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
        ("unresolved_calls", unresolved.unwrap_or_default().into()),
    ])
}

pub(crate) fn label(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Module => "Module",
        SymbolKind::Struct => "Struct",
        SymbolKind::Enum => "Enum",
        SymbolKind::Trait => "Trait",
        SymbolKind::Function => "Function",
        SymbolKind::Method => "Method",
    }
}

/// `UNWIND` statement creating a symbol-to-symbol relationship.
fn symbol_rel(rel_type: &str, props: &str) -> String {
    format!(
        "UNWIND $rows AS row MATCH (a:Symbol {{repo_id: $repo, id: row.from}}) \
         MATCH (b:Symbol {{repo_id: $repo, id: row.to}}) CREATE (a)-[:{rel_type} {props}]->(b)"
    )
}

fn crate_id(package: &str, name: &str) -> String {
    format!("crate:{package}:{name}")
}

fn pair(from: &str, to: &str) -> BoltType {
    map([("from", from.into()), ("to", to.into())])
}

fn map<const N: usize>(entries: [(&'static str, BoltType); N]) -> BoltType {
    BoltType::from(HashMap::from(entries))
}
