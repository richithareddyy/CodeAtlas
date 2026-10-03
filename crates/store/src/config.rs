use std::fmt;

use crate::error::{Result, StoreError};

/// Connection settings. The password is never printed by `Debug`.
#[derive(Clone)]
pub struct StoreConfig {
    pub uri: String,
    pub user: String,
    pub password: String,
    pub database: String,
    pub max_connections: usize,
}

impl StoreConfig {
    /// Reads `CODEATLAS_NEO4J_URI`, `CODEATLAS_NEO4J_USER`,
    /// `CODEATLAS_NEO4J_PASSWORD` and `CODEATLAS_NEO4J_DATABASE`.
    /// Only the password is required.
    pub fn from_env() -> Result<Self> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let password = var("CODEATLAS_NEO4J_PASSWORD").ok_or_else(|| {
            StoreError::Config(
                "CODEATLAS_NEO4J_PASSWORD is not set (copy .env.example to .env)".into(),
            )
        })?;
        Ok(Self {
            uri: var("CODEATLAS_NEO4J_URI").unwrap_or_else(|| "bolt://localhost:7687".into()),
            user: var("CODEATLAS_NEO4J_USER").unwrap_or_else(|| "neo4j".into()),
            password,
            database: var("CODEATLAS_NEO4J_DATABASE").unwrap_or_else(|| "neo4j".into()),
            max_connections: 8,
        })
    }
}

impl fmt::Debug for StoreConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoreConfig")
            .field("uri", &self.uri)
            .field("user", &self.user)
            .field("password", &"<redacted>")
            .field("database", &self.database)
            .finish()
    }
}

/// Hard limits that keep graph queries bounded regardless of input.
#[derive(Debug, Clone, Copy)]
pub struct QueryLimits {
    /// Maximum traversal depth accepted by any query.
    pub max_depth: u32,
    /// Maximum number of nodes a traversal returns before it is truncated.
    pub max_nodes: usize,
    /// Maximum number of search results.
    pub max_results: usize,
}

impl Default for QueryLimits {
    fn default() -> Self {
        Self {
            max_depth: 10,
            max_nodes: 500,
            max_results: 50,
        }
    }
}
