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
    /// `CODEATLAS_ALLOW_INDEXING`: whether the `indexRepository` /
    /// `removeRepository` mutations are enabled. Indexing reads any local
    /// path or clones any URL the server can reach, so it defaults to on
    /// only when the server listens on a loopback address; elsewhere it has
    /// to be enabled explicitly.
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
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let var = |name: &str| lookup(name).filter(|v| !v.trim().is_empty());
        let flag = |name: &str, default: bool| -> Result<bool> {
            match var(name).as_deref().map(str::trim) {
                None => Ok(default),
                Some("1" | "true" | "yes") => Ok(true),
                Some("0" | "false" | "no") => Ok(false),
                Some(other) => anyhow::bail!("{name} must be true or false, got `{other}`"),
            }
        };
        let addr: SocketAddr = var("CODEATLAS_HTTP_ADDR")
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
            allow_indexing: flag("CODEATLAS_ALLOW_INDEXING", addr.ip().is_loopback())?,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn config(vars: &[(&str, &str)]) -> ServerConfig {
        ServerConfig::from_lookup(|name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        })
        .unwrap()
    }

    #[test]
    fn indexing_is_on_by_default_only_on_loopback() {
        assert!(config(&[]).allow_indexing);
        assert!(config(&[("CODEATLAS_HTTP_ADDR", "[::1]:8080")]).allow_indexing);
        assert!(!config(&[("CODEATLAS_HTTP_ADDR", "0.0.0.0:8080")]).allow_indexing);
        assert!(
            config(&[
                ("CODEATLAS_HTTP_ADDR", "0.0.0.0:8080"),
                ("CODEATLAS_ALLOW_INDEXING", "true")
            ])
            .allow_indexing
        );
        assert!(!config(&[("CODEATLAS_ALLOW_INDEXING", "false")]).allow_indexing);
    }
}
