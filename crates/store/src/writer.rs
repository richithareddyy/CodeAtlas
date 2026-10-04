//! Persists a [`GraphSnapshot`], either as a whole or as the delta from the
//! previous snapshot.
//!
//! A full write deletes the repository's previous subgraph in batches, then
//! creates every node and relationship with batched `UNWIND` statements in
//! one transaction. An incremental write applies a [`GraphDelta`] in one
//! transaction: removed relationships and nodes first, then updated and new
//! nodes, then new and updated relationships. Readers never observe a
//! partially applied delta. Labels and relationship types are fixed strings
//! chosen in code, never taken from input.

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use codeatlas_analyzer::model::SymbolKind;
use neo4rs::{query, BoltType, Graph, Txn};

use crate::error::Result;
use crate::snapshot::{to_bolt, GraphDelta, GraphSnapshot, NodeKind, Props, RelKey};

/// Version of the stored graph layout. Bump it whenever the writer stores
/// new facts that readers depend on; graphs with another version are
/// rejected by `load_graph` instead of producing silently wrong results.
pub const GRAPH_FORMAT_VERSION: i64 = 2;

/// Rows sent per `UNWIND` statement.
const BATCH_SIZE: usize = 1_000;
/// Nodes deleted per statement when clearing a repository.
const DELETE_BATCH: i64 = 5_000;

#[derive(Debug, Clone, PartialEq)]
pub struct IndexSummary {
    pub repo_id: String,
    /// Nodes and relationships stored for the repository after the write.
    pub nodes: usize,
    pub relationships: usize,
    pub write_ms: f64,
}

/// Replaces the repository's stored graph with `snapshot`.
pub async fn write_full(
    graph: &Graph,
    repo: &str,
    snapshot: &GraphSnapshot,
) -> Result<IndexSummary> {
    let started = Instant::now();
    delete_repository(graph, repo).await?;
    let mut w = Writer {
        txn: graph.start_txn().await?,
        repo,
    };
    w.txn
        .run(
            query("CREATE (r:Repository) SET r = $props")
                .param("props", to_bolt(&snapshot.repository)),
        )
        .await?;
    w.create_crates(snapshot, snapshot.crates.keys()).await?;
    w.create_files(snapshot, snapshot.files.keys()).await?;
    w.create_symbols(snapshot, snapshot.symbols.keys()).await?;
    w.create_relationships(snapshot, snapshot.relationships.keys())
        .await?;
    w.txn.commit().await?;
    Ok(summary(repo, snapshot, started))
}

/// Turns the stored graph `old` into `new` by applying their delta.
pub async fn write_delta(
    graph: &Graph,
    repo: &str,
    old: &GraphSnapshot,
    new: &GraphSnapshot,
    delta: &GraphDelta,
) -> Result<IndexSummary> {
    let started = Instant::now();
    let mut w = Writer {
        txn: graph.start_txn().await?,
        repo,
    };
    let r = &delta.relationships;
    w.delete_relationships(r.removed.iter()).await?;
    w.delete_nodes(NodeKind::Symbol, &delta.symbols.removed)
        .await?;
    w.delete_nodes(NodeKind::File, &delta.files.removed).await?;
    w.delete_nodes(NodeKind::Crate, &delta.crates.removed)
        .await?;

    w.txn
        .run(
            query("MATCH (r:Repository {id: $repo}) SET r = $props")
                .param("repo", repo)
                .param("props", to_bolt(&new.repository)),
        )
        .await?;
    w.update_nodes(
        NodeKind::Crate,
        delta.crates.changed.iter().map(|k| &new.crates[k]),
    )
    .await?;
    w.update_nodes(
        NodeKind::File,
        delta.files.changed.iter().map(|k| &new.files[k]),
    )
    .await?;
    w.update_symbols(old, new, &delta.symbols.changed).await?;
    w.create_crates(new, delta.crates.added.iter()).await?;
    w.create_files(new, delta.files.added.iter()).await?;
    w.create_symbols(new, delta.symbols.added.iter()).await?;

    w.create_relationships(new, r.added.iter()).await?;
    w.update_relationships(new, r.changed.iter()).await?;
    w.txn.commit().await?;
    Ok(summary(repo, new, started))
}

fn summary(repo: &str, snapshot: &GraphSnapshot, started: Instant) -> IndexSummary {
    let summary = IndexSummary {
        repo_id: repo.to_string(),
        nodes: snapshot.node_count(),
        relationships: snapshot.relationships.len(),
        write_ms: started.elapsed().as_secs_f64() * 1000.0,
    };
    tracing::info!(
        repo = %summary.repo_id,
        nodes = summary.nodes,
        relationships = summary.relationships,
        ms = summary.write_ms,
        "graph written"
    );
    summary
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
}

impl Writer<'_> {
    async fn unwind(&mut self, cypher: &str, rows: Vec<BoltType>) -> Result<()> {
        for chunk in rows.chunks(BATCH_SIZE) {
            self.txn
                .run(
                    query(cypher)
                        .param("repo", self.repo)
                        .param("rows", chunk.to_vec()),
                )
                .await?;
        }
        Ok(())
    }

    async fn create_crates<'k>(
        &mut self,
        s: &GraphSnapshot,
        keys: impl Iterator<Item = &'k String>,
    ) -> Result<()> {
        let rows = keys.map(|k| to_bolt(&s.crates[k])).collect();
        self.unwind(
            "UNWIND $rows AS row CREATE (c:Crate {repo_id: $repo}) SET c += row",
            rows,
        )
        .await
    }

    async fn create_files<'k>(
        &mut self,
        s: &GraphSnapshot,
        keys: impl Iterator<Item = &'k String>,
    ) -> Result<()> {
        let rows = keys.map(|k| to_bolt(&s.files[k])).collect();
        self.unwind(
            "UNWIND $rows AS row CREATE (f:File {repo_id: $repo}) SET f += row",
            rows,
        )
        .await
    }

    async fn create_symbols<'k>(
        &mut self,
        s: &GraphSnapshot,
        keys: impl Iterator<Item = &'k String>,
    ) -> Result<()> {
        let mut by_kind: BTreeMap<SymbolKind, Vec<BoltType>> = BTreeMap::new();
        let mut tests = Vec::new();
        for key in keys {
            let node = &s.symbols[key];
            by_kind
                .entry(node.kind)
                .or_default()
                .push(to_bolt(&node.props));
            if node.is_test {
                tests.push(BoltType::from(key.as_str()));
            }
        }
        for (kind, rows) in by_kind {
            let cypher = format!(
                "UNWIND $rows AS row CREATE (s:Symbol:{} {{repo_id: $repo}}) SET s += row",
                label(kind)
            );
            self.unwind(&cypher, rows).await?;
        }
        self.unwind(
            "UNWIND $rows AS id MATCH (s:Symbol {repo_id: $repo, id: id}) SET s:Test",
            tests,
        )
        .await
    }

    async fn delete_nodes(&mut self, kind: NodeKind, keys: &[String]) -> Result<()> {
        let rows = keys
            .iter()
            .map(|k| BoltType::from(HashMap::from([("key", BoltType::from(k.as_str()))])))
            .collect();
        let cypher = format!(
            "UNWIND $rows AS row MATCH {} DETACH DELETE n",
            kind.pattern("n", "key")
        );
        self.unwind(&cypher, rows).await
    }

    async fn update_nodes<'p>(
        &mut self,
        kind: NodeKind,
        props: impl Iterator<Item = &'p Props>,
    ) -> Result<()> {
        let key_field = if kind == NodeKind::File { "path" } else { "id" };
        let rows = props.map(to_bolt).collect();
        // `+=` with every property present: nulls remove stale values.
        let cypher = format!(
            "UNWIND $rows AS row MATCH {} SET n += row",
            kind.pattern("n", key_field)
        );
        self.unwind(&cypher, rows).await
    }

    async fn update_symbols(
        &mut self,
        old: &GraphSnapshot,
        new: &GraphSnapshot,
        keys: &[String],
    ) -> Result<()> {
        self.update_nodes(NodeKind::Symbol, keys.iter().map(|k| &new.symbols[k].props))
            .await?;
        // The kind is part of the ID and never changes; the Test label can.
        let (mut test, mut not_test) = (Vec::new(), Vec::new());
        for key in keys {
            match (old.symbols[key].is_test, new.symbols[key].is_test) {
                (false, true) => test.push(BoltType::from(key.as_str())),
                (true, false) => not_test.push(BoltType::from(key.as_str())),
                _ => {}
            }
        }
        self.unwind(
            "UNWIND $rows AS id MATCH (s:Symbol {repo_id: $repo, id: id}) SET s:Test",
            test,
        )
        .await?;
        self.unwind(
            "UNWIND $rows AS id MATCH (s:Symbol {repo_id: $repo, id: id}) REMOVE s:Test",
            not_test,
        )
        .await
    }

    async fn create_relationships<'k>(
        &mut self,
        s: &GraphSnapshot,
        keys: impl Iterator<Item = &'k RelKey>,
    ) -> Result<()> {
        for ((rel, from, to), rows) in group(keys, |k| Some(to_bolt(&s.relationships[k]))) {
            let cypher = format!(
                "UNWIND $rows AS row MATCH {} MATCH {} CREATE (a)-[r:{rel}]->(b) SET r = row.props",
                from.pattern("a", "from"),
                to.pattern("b", "to")
            );
            self.unwind(&cypher, rows).await?;
        }
        Ok(())
    }

    async fn update_relationships<'k>(
        &mut self,
        s: &GraphSnapshot,
        keys: impl Iterator<Item = &'k RelKey>,
    ) -> Result<()> {
        for ((rel, from, to), rows) in group(keys, |k| Some(to_bolt(&s.relationships[k]))) {
            let cypher = format!(
                "UNWIND $rows AS row MATCH {}-[r:{rel}]->{} SET r = row.props",
                from.pattern("a", "from"),
                to.pattern("b", "to")
            );
            self.unwind(&cypher, rows).await?;
        }
        Ok(())
    }

    async fn delete_relationships<'k>(
        &mut self,
        keys: impl Iterator<Item = &'k RelKey>,
    ) -> Result<()> {
        for ((rel, from, to), rows) in group(keys, |_| None) {
            let cypher = format!(
                "UNWIND $rows AS row MATCH {}-[r:{rel}]->{} DELETE r",
                from.pattern("a", "from"),
                to.pattern("b", "to")
            );
            self.unwind(&cypher, rows).await?;
        }
        Ok(())
    }
}

type RelGroup = (&'static str, NodeKind, NodeKind);

/// Rows `{from, to, props}` grouped by relationship type and endpoint
/// kinds, which determine the Cypher statement.
fn group<'k>(
    keys: impl Iterator<Item = &'k RelKey>,
    props: impl Fn(&RelKey) -> Option<BoltType>,
) -> BTreeMap<RelGroup, Vec<BoltType>> {
    let mut groups: BTreeMap<RelGroup, Vec<BoltType>> = BTreeMap::new();
    for key in keys {
        let mut row: HashMap<&str, BoltType> = HashMap::from([
            ("from", key.from.key().unwrap_or_default().into()),
            ("to", key.to.key().unwrap_or_default().into()),
        ]);
        if let Some(props) = props(key) {
            row.insert("props", props);
        }
        groups
            .entry((key.rel, key.from.kind(), key.to.kind()))
            .or_default()
            .push(BoltType::from(row));
    }
    groups
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
