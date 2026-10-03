//! Neo4j persistence and graph queries for CodeAtlas.
//!
//! The analyzer produces a [`codeatlas_analyzer::RepositoryAnalysis`];
//! [`GraphStore::index`] writes it as the graph described in
//! `docs/graph-model.md`, and the query methods read it back with hard
//! limits on depth and result size.

mod config;
mod error;
mod queries;
mod schema;
mod writer;

pub use config::{QueryLimits, StoreConfig};
pub use error::{Result, StoreError};
pub use queries::{
    AggregateDependency, DependencyPath, Direction, GraphEdge, GraphStats, RelatedTest, Relation,
    RepositoryNode, SymbolNode, Traversal, TraversalNode,
};
pub use writer::IndexSummary;

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

    /// Creates constraints and indexes if they do not exist.
    pub async fn ensure_schema(&self) -> Result<()> {
        schema::ensure(&self.graph).await
    }

    /// Replaces the stored graph of `analysis.repository` with `analysis`.
    pub async fn index(&self, analysis: &RepositoryAnalysis) -> Result<IndexSummary> {
        writer::write_repository(&self.graph, analysis).await
    }

    /// Deletes a repository's graph; returns the number of deleted nodes.
    pub async fn delete_repository(&self, repo_id: &str) -> Result<u64> {
        writer::delete_repository(&self.graph, repo_id).await
    }
}
