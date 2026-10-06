//! Neo4j persistence and graph queries for CodeAtlas.
//!
//! The analyzer produces a [`codeatlas_analyzer::RepositoryAnalysis`];
//! [`GraphStore::index`] writes it as the graph described in
//! `docs/graph-model.md`, and the query methods read it back with hard
//! limits on depth and result size.

mod config;
mod error;
pub mod incremental;
mod load;
mod queries;
mod schema;
pub mod snapshot;
mod writer;

pub use config::{QueryLimits, StoreConfig};
pub use error::{Result, StoreError};
pub use incremental::{
    prepare, DeltaStats, FullReason, IndexMode, IndexReport, IndexState, Prepared, StateDir,
};
pub use queries::{
    AggregateDependency, CrateNode, DependencyPath, Direction, GraphEdge, GraphStats, RelatedTest,
    Relation, RepositoryNode, SearchPage, SymbolNode, Traversal, TraversalNode,
};
pub use writer::{IndexSummary, GRAPH_FORMAT_VERSION};

use codeatlas_analyzer::RepositoryAnalysis;
use neo4rs::{query, ConfigBuilder, Graph};

pub struct GraphStore {
    graph: Graph,
    limits: QueryLimits,
}

impl GraphStore {
    /// Connects and verifies the connection with a trivial query, so that
    /// a wrong URI or password fails here rather than on first use.
    pub async fn connect(config: &StoreConfig) -> Result<Self> {
        let neo4j_config = ConfigBuilder::default()
            .uri(&config.uri)
            .user(&config.user)
            .password(&config.password)
            .db(config.database.as_str())
            .max_connections(config.max_connections)
            .build()?;
        let graph = Graph::connect(neo4j_config).await?;
        graph.run(query("RETURN 1")).await?;
        Ok(Self {
            graph,
            limits: QueryLimits::default(),
        })
    }

    pub fn with_limits(mut self, limits: QueryLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn limits(&self) -> QueryLimits {
        self.limits
    }

    /// `name version edition` of the connected Neo4j server.
    pub async fn server_version(&self) -> Result<String> {
        let mut rows = self
            .graph
            .execute(query(
                "CALL dbms.components() YIELD name, versions, edition \
                 RETURN name + ' ' + versions[0] + ' ' + edition AS version",
            ))
            .await?;
        Ok(match rows.next().await? {
            Some(row) => row.get("version")?,
            None => String::new(),
        })
    }

    /// Creates constraints and indexes if they do not exist.
    pub async fn ensure_schema(&self) -> Result<()> {
        schema::ensure(&self.graph).await
    }

    /// Replaces the stored graph of `analysis.repository` with `analysis`,
    /// without using or saving incremental state (see [`incremental`]).
    pub async fn index(&self, analysis: &RepositoryAnalysis) -> Result<IndexSummary> {
        let snapshot = snapshot::GraphSnapshot::new(
            analysis,
            &incremental::new_token(),
            &chrono::Utc::now().to_rfc3339(),
        );
        writer::write_full(&self.graph, &analysis.repository.id, &snapshot).await
    }

    /// Overwrites the stored format version (to exercise version checks).
    #[doc(hidden)]
    pub async fn set_format_version_for_tests(&self, repo_id: &str, version: i64) -> Result<()> {
        self.graph
            .run(
                query("MATCH (r:Repository {id: $repo}) SET r.format_version = $version")
                    .param("repo", repo_id)
                    .param("version", version),
            )
            .await?;
        Ok(())
    }

    /// Every stored node and relationship of a repository with all
    /// properties, as sorted lines, for comparing two stored graphs. The
    /// repository node's identity and timestamps are left out.
    #[doc(hidden)]
    pub async fn dump_for_tests(&self, repo_id: &str) -> Result<Vec<String>> {
        const VOLATILE: [&str; 4] = ["id", "index_token", "indexed_at", "analyzed_at"];
        let mut lines = Vec::new();
        let mut rows = self
            .graph
            .execute(
                query(
                    "MATCH (n) WHERE n.repo_id = $repo OR (n:Repository AND n.id = $repo) \
                     RETURN labels(n) AS labels, properties(n) AS props",
                )
                .param("repo", repo_id),
            )
            .await?;
        while let Some(row) = rows.next().await? {
            let mut labels: Vec<String> = row.get("labels")?;
            labels.sort();
            let repository = labels.iter().any(|l| l == "Repository");
            let props: neo4rs::BoltMap = row.get("props")?;
            lines.push(format!(
                "node {} {}",
                labels.join(":"),
                render_map(&props, if repository { &VOLATILE } else { &["repo_id"] })
            ));
        }
        let mut rows = self
            .graph
            .execute(
                query(
                    "MATCH (a)-[r]->(b) WHERE a.repo_id = $repo OR (a:Repository AND a.id = $repo) \
                     RETURN type(r) AS type, \
                       CASE WHEN a:Repository THEN 'repository' ELSE coalesce(a.id, a.path) END AS from, \
                       coalesce(b.id, b.path) AS to, properties(r) AS props",
                )
                .param("repo", repo_id),
            )
            .await?;
        while let Some(row) = rows.next().await? {
            let props: neo4rs::BoltMap = row.get("props")?;
            lines.push(format!(
                "rel {} {} -> {} {}",
                row.get::<String>("type")?,
                row.get::<String>("from")?,
                row.get::<String>("to")?,
                render_map(&props, &[])
            ));
        }
        lines.sort();
        Ok(lines)
    }

    /// Deletes a repository's graph; returns the number of deleted nodes.
    pub async fn delete_repository(&self, repo_id: &str) -> Result<u64> {
        writer::delete_repository(&self.graph, repo_id).await
    }
}

fn render_map(map: &neo4rs::BoltMap, skip: &[&str]) -> String {
    let mut entries: Vec<String> = map
        .value
        .iter()
        .filter(|(k, _)| !skip.contains(&k.value.as_str()))
        .map(|(k, v)| format!("{}={}", k.value, render(v)))
        .collect();
    entries.sort();
    format!("{{{}}}", entries.join(", "))
}

fn render(value: &neo4rs::BoltType) -> String {
    use neo4rs::BoltType;
    match value {
        BoltType::String(s) => format!("{:?}", s.value),
        BoltType::Integer(i) => i.value.to_string(),
        BoltType::Float(f) => f.value.to_string(),
        BoltType::Boolean(b) => b.value.to_string(),
        BoltType::Null(_) => "null".to_string(),
        BoltType::List(items) => format!(
            "[{}]",
            items
                .value
                .iter()
                .map(render)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        BoltType::Map(map) => render_map(map, &[]),
        other => format!("{other:?}"),
    }
}
