use std::sync::Arc;

use async_graphql::{Context, Object, Result, ID};
use codeatlas_analyzer::ingest::{ingest, IngestOptions};
use codeatlas_analyzer::RepoSource;
use codeatlas_store::{prepare, IndexMode as StoreIndexMode};

use super::types::{IndexMode, IndexResult};
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
    /// Analyses a local path or Git URL (as seen by the server) and stores
    /// its graph. When the repository was indexed before by this server,
    /// only changed files are parsed and only the difference to the stored
    /// graph is written; `full: true` rewrites everything.
    async fn index_repository(
        &self,
        ctx: &Context<'_>,
        source: String,
        #[graphql(default = false)] full: bool,
    ) -> Result<IndexResult> {
        let state = writable(ctx)?;
        let options = IngestOptions {
            clone_dir: state.clone_dir.clone(),
            ..Default::default()
        };
        let states = state.states.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            let repo = ingest(&RepoSource::parse(&source), &options)?;
            prepare(&repo, &states)
        })
        .await
        .map_err(internal)?
        .map_err(|err| coded(format!("analysis failed: {err}"), "INDEX_FAILED"))?;
        let files_analyzed = prepared.analysis.stats.files_analyzed as i32;
        let resolution_rate = prepared.analysis.resolution.stats.calls.resolution_rate;

        let report = state
            .store
            .index_prepared(prepared, &state.states, full)
            .await
            .gql()?;
        state.graphs.forget(&report.summary.repo_id).await;
        let repository = state
            .store
            .repository(&report.summary.repo_id)
            .await
            .gql()?;
        let (mode, full_reason) = match report.mode {
            StoreIndexMode::Incremental => (IndexMode::Incremental, None),
            StoreIndexMode::Full(reason) => (
                IndexMode::Full,
                Some(
                    serde_json::to_value(reason)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default(),
                ),
            ),
        };
        let (r, d) = (&report.reuse, &report.delta);
        let int = |n: usize| i32::try_from(n).unwrap_or(i32::MAX);
        Ok(IndexResult {
            repository: repository.into(),
            files_analyzed,
            nodes: int(report.summary.nodes),
            relationships: int(report.summary.relationships),
            resolution_rate,
            analysis_ms: report.analysis_ms,
            write_ms: report.summary.write_ms,
            mode,
            full_reason,
            files_changed: (r.changed + r.added) as i32,
            files_removed: r.removed as i32,
            files_parsed: r.parsed as i32,
            files_reused: r.reused as i32,
            nodes_added: int(d.nodes_added),
            nodes_removed: int(d.nodes_removed),
            nodes_changed: int(d.nodes_changed),
            relationships_added: int(d.relationships_added),
            relationships_removed: int(d.relationships_removed),
            relationships_changed: int(d.relationships_changed),
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
        if let Err(err) = state.states.remove(&repository.id) {
            tracing::warn!(%err, "cannot remove index state");
        }
        Ok(true)
    }
}
