use std::sync::Arc;
use std::time::Instant;

use async_graphql::{Context, Object, Result, ID};
use codeatlas_analyzer::ingest::IngestOptions;
use codeatlas_analyzer::{analyze_source, RepoSource};

use super::types::IndexResult;
use crate::error::{coded, internal, StoreResultExt};
use crate::state::AppState;

pub struct MutationRoot;

fn writable(ctx: &Context<'_>) -> Result<Arc<AppState>> {
    let state = ctx.data::<Arc<AppState>>()?.clone();
    if !state.allow_indexing {
        return Err(coded(
            "indexing is disabled on this server (CODEATLAS_ALLOW_INDEXING=false)",
            "FORBIDDEN",
        ));
    }
    Ok(state)
}

#[Object(name = "Mutation")]
impl MutationRoot {
    /// Analyses a local path or Git URL (as seen by the server) and replaces
    /// its stored graph.
    async fn index_repository(&self, ctx: &Context<'_>, source: String) -> Result<IndexResult> {
        let state = writable(ctx)?;
        let options = IngestOptions {
            clone_dir: state.clone_dir.clone(),
            ..Default::default()
        };
        let started = Instant::now();
        let analysis = tokio::task::spawn_blocking(move || {
            analyze_source(&RepoSource::parse(&source), &options)
        })
        .await
        .map_err(internal)?
        .map_err(|err| coded(format!("analysis failed: {err}"), "INDEX_FAILED"))?;
        let analysis_ms = started.elapsed().as_secs_f64() * 1000.0;

        let summary = state.store.index(&analysis).await.gql()?;
        state.graphs.forget(&summary.repo_id).await;
        let repository = state.store.repository(&summary.repo_id).await.gql()?;
        Ok(IndexResult {
            repository: repository.into(),
            files_analyzed: analysis.stats.files_analyzed as i32,
            nodes: summary.nodes as i32,
            relationships: summary.relationships as i32,
            resolution_rate: analysis.resolution.stats.calls.resolution_rate,
            analysis_ms,
            write_ms: summary.write_ms,
        })
    }

    /// Deletes a repository's graph. Returns `true` when it existed.
    async fn remove_repository(&self, ctx: &Context<'_>, id: ID) -> Result<bool> {
        let state = writable(ctx)?;
        let repository = match state.store.repository(&id).await {
            Ok(repository) => repository,
            Err(codeatlas_store::StoreError::NotFound(_)) => return Ok(false),
            Err(err) => return Err(crate::error::from_store(err)),
        };
        state.store.delete_repository(&repository.id).await.gql()?;
        state.graphs.forget(&repository.id).await;
        Ok(true)
    }
}
