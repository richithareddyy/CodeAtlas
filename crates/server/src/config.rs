use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use codeatlas_analyzer::ingest::default_clone_dir;
use codeatlas_store::StateDir;

/// HTTP server settings, read from the environment.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// `CODEATLAS_HTTP_ADDR`, default `127.0.0.1:8080`.
    pub addr: SocketAddr,
    /// `CODEATLAS_CORS_ORIGINS`, comma-separated. Defaults to the SvelteKit
    /// dev server (`http://localhost:5173`, `http://127.0.0.1:5173`).
    pub cors_origins: Vec<String>,
    /// `CODEATLAS_ALLOW_INDEXING` (default `true`): whether the
    /// `indexRepository` / `removeRepository` mutations are enabled. Indexing
    /// reads any local path or clones any URL the server can reach, so
    /// disable it when the server is reachable by others.
    pub allow_indexing: bool,
    /// `CODEATLAS_GRAPHIQL` (default `true`): serve GraphiQL at `GET /graphql`.
    pub graphiql: bool,
    /// `CODEATLAS_CLONE_DIR`: where remote repositories are cloned.
    pub clone_dir: PathBuf,
    /// `CODEATLAS_STATE_DIR`: where incremental indexing state is kept.
    pub state_dir: PathBuf,
}

impl ServerConfig {
    pub fn from_env() -> Result<Self> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let flag = |name: &str, default: bool| -> Result<bool> {
            match var(name).as_deref().map(str::trim) {
                None => Ok(default),
                Some("1" | "true" | "yes") => Ok(true),
                Some("0" | "false" | "no") => Ok(false),
                Some(other) => anyhow::bail!("{name} must be true or false, got `{other}`"),
            }
        };
        let addr = var("CODEATLAS_HTTP_ADDR")
            .unwrap_or_else(|| "127.0.0.1:8080".into())
            .parse()
            .context("CODEATLAS_HTTP_ADDR must be an address such as 127.0.0.1:8080")?;
        let cors_origins = var("CODEATLAS_CORS_ORIGINS")
            .unwrap_or_else(|| "http://localhost:5173,http://127.0.0.1:5173".into())
            .split(',')
            .map(|o| o.trim().to_string())
            .filter(|o| !o.is_empty())
            .collect();
        Ok(Self {
            addr,
            cors_origins,
            allow_indexing: flag("CODEATLAS_ALLOW_INDEXING", true)?,
            graphiql: flag("CODEATLAS_GRAPHIQL", true)?,
            clone_dir: var("CODEATLAS_CLONE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(default_clone_dir),
            state_dir: var("CODEATLAS_STATE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(StateDir::default_dir),
        })
    }
}
