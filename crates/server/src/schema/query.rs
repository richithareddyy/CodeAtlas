use std::sync::Arc;

use async_graphql::connection::{self, Connection, Edge};
use async_graphql::{Context, Object, Result, ID};
use codeatlas_analyzer::graph::{architecture, impact};
use codeatlas_analyzer::model::SymbolId;
use codeatlas_store::{Direction as StoreDirection, StoreError};

use super::types::*;
use crate::error::{bad_input, coded, from_impact, StoreResultExt};
use crate::source::{read_snippet, SourceError};
use crate::state::AppState;

/// Upper bound for `first` in `searchSymbols`.
const MAX_PAGE: usize = 50;
/// Upper bound for `first` in `hotspots`.
const MAX_HOTSPOTS: i32 = 100;

pub struct QueryRoot;

fn state<'a>(ctx: &'a Context<'_>) -> Result<&'a Arc<AppState>> {
    ctx.data::<Arc<AppState>>()
}

/// Canonical repository ID for an ID or (unique) name.
async fn repo_id(state: &AppState, key: &ID) -> Result<String> {
    Ok(state.store.repository(key).await.gql()?.id)
}

fn depth(value: i32, max: u32, argument: &str) -> Result<u32> {
    u32::try_from(value)
        .ok()
        .filter(|d| (1..=max).contains(d))
        .ok_or_else(|| bad_input(format!("{argument} must be between 1 and {max}")))
}

fn optional<T>(result: std::result::Result<T, StoreError>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(StoreError::NotFound(_)) => Ok(None),
        Err(err) => Err(crate::error::from_store(err)),
    }
}

#[Object(name = "Query")]
impl QueryRoot {
    /// All indexed repositories.
    async fn repositories(&self, ctx: &Context<'_>) -> Result<Vec<Repository>> {
        let state = state(ctx)?;
        Ok(state
            .store
            .repositories()
            .await
            .gql()?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// A repository by ID or unique name; `null` if unknown.
    async fn repository(&self, ctx: &Context<'_>, id: ID) -> Result<Option<Repository>> {
        let state = state(ctx)?;
        Ok(optional(state.store.repository(&id).await)?.map(Into::into))
    }

    /// A symbol by exact ID; `null` if unknown.
    async fn symbol(&self, ctx: &Context<'_>, repo_id: ID, id: ID) -> Result<Option<Symbol>> {
        let state = state(ctx)?;
        let repo = self::repo_id(state, &repo_id).await?;
        Ok(optional(state.store.symbol(&repo, &id).await)?.map(Into::into))
    }

    /// Symbols whose name or qualified name matches every term of `query`
    /// as a prefix. Exact name matches come first. Cursor-paginated with
    /// `first` (at most 50) and `after`.
    async fn search_symbols(
        &self,
        ctx: &Context<'_>,
        repo_id: ID,
        query: String,
        kinds: Option<Vec<SymbolKind>>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Connection<usize, Symbol>> {
        let state = state(ctx)?.clone();
        let repo = self::repo_id(&state, &repo_id).await?;
        let first = match first {
            None => 20,
            Some(n) if (1..=MAX_PAGE as i32).contains(&n) => n as usize,
            Some(_) => return Err(bad_input(format!("first must be between 1 and {MAX_PAGE}"))),
        };
        let kinds: Option<Vec<String>> =
            kinds.map(|ks| ks.iter().map(|k| k.as_store_kind().to_string()).collect());
        connection::query(
            after,
            None,
            Some(first as i32),
            None,
            |after: Option<usize>, _before: Option<usize>, first: Option<usize>, _last| async move {
                let offset = after.map_or(0, |cursor| cursor + 1);
                let page = state
                    .store
                    .search_page(&repo, &query, kinds.as_deref(), first.unwrap_or(20), offset)
                    .await
                    .gql()?;
                let mut connection = Connection::new(offset > 0, page.has_more);
                connection.edges.extend(
                    page.symbols
                        .into_iter()
                        .enumerate()
                        .map(|(i, symbol)| Edge::new(offset + i, Symbol::from(symbol))),
                );
                Ok::<_, async_graphql::Error>(connection)
            },
        )
        .await
    }

    /// Build targets of a repository (libraries first) with their root modules.
    async fn crates(&self, ctx: &Context<'_>, repo_id: ID) -> Result<Vec<Crate>> {
        let state = state(ctx)?;
        let repo = self::repo_id(state, &repo_id).await?;
        Ok(state
            .store
            .crates(&repo)
            .await
            .gql()?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Direct children of a symbol: items of a module, methods of a type.
    async fn children(&self, ctx: &Context<'_>, repo_id: ID, id: ID) -> Result<Vec<Symbol>> {
        let state = state(ctx)?;
        let repo = self::repo_id(state, &repo_id).await?;
        Ok(state
            .store
            .children(&repo, &id)
            .await
            .gql()?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Whole-repository dependency graph at crate, module or file level.
    async fn architecture_graph(
        &self,
        ctx: &Context<'_>,
        repo_id: ID,
        #[graphql(default_with = "ArchitectureLevel::Module")] level: ArchitectureLevel,
    ) -> Result<ArchitectureGraph> {
        let state = state(ctx)?;
        let (_, graph) = state.graphs.get(&state.store, &repo_id).await.gql()?;
        Ok(ArchitectureGraph::new(
            level,
            architecture::dependency_graph(&graph, level.into()),
        ))
    }

    /// What `symbolId` depends on, breadth-first up to `depth`.
    async fn dependencies(
        &self,
        ctx: &Context<'_>,
        repo_id: ID,
        symbol_id: ID,
        #[graphql(default = 1)] depth: i32,
        #[graphql(default_with = "vec![Relation::Calls]")] relations: Vec<Relation>,
    ) -> Result<Neighborhood> {
        neighborhood(
            ctx,
            repo_id,
            symbol_id,
            depth,
            relations,
            StoreDirection::Dependencies,
        )
        .await
    }

    /// What depends on `symbolId`, breadth-first up to `depth`.
    async fn dependents(
        &self,
        ctx: &Context<'_>,
        repo_id: ID,
        symbol_id: ID,
        #[graphql(default = 1)] depth: i32,
        #[graphql(default_with = "vec![Relation::Calls]")] relations: Vec<Relation>,
    ) -> Result<Neighborhood> {
        neighborhood(
            ctx,
            repo_id,
            symbol_id,
            depth,
            relations,
            StoreDirection::Dependents,
        )
        .await
    }

    /// Shortest dependency path from `from` to `to` over resolved calls,
    /// imports and implementations; `null` if none within `maxDepth`.
    async fn dependency_path(
        &self,
        ctx: &Context<'_>,
        repo_id: ID,
        from: ID,
        to: ID,
        #[graphql(default = 10)] max_depth: i32,
    ) -> Result<Option<DependencyPath>> {
        let state = state(ctx)?;
        let max_depth = depth(max_depth, state.store.limits().max_depth, "maxDepth")?;
        let repo = self::repo_id(state, &repo_id).await?;
        Ok(state
            .store
            .shortest_path(&repo, &from, &to, max_depth)
            .await
            .gql()?
            .map(Into::into))
    }

    /// Files depending on `path` (`DEPENDENTS`) or that it depends on.
    async fn file_dependencies(
        &self,
        ctx: &Context<'_>,
        repo_id: ID,
        path: String,
        direction: Direction,
    ) -> Result<Vec<AggregateDependency>> {
        let state = state(ctx)?;
        let repo = self::repo_id(state, &repo_id).await?;
        Ok(state
            .store
            .file_dependencies(&repo, &path, store_direction(direction))
            .await
            .gql()?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Modules depending on `moduleId` (`DEPENDENTS`) or that it depends on.
    async fn module_dependencies(
        &self,
        ctx: &Context<'_>,
        repo_id: ID,
        module_id: ID,
        direction: Direction,
    ) -> Result<Vec<AggregateDependency>> {
        let state = state(ctx)?;
        let repo = self::repo_id(state, &repo_id).await?;
        Ok(state
            .store
            .module_dependencies(&repo, &module_id, store_direction(direction))
            .await
            .gql()?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// What could be affected by changing `symbolId` (types, traits and
    /// modules include their members) or every symbol in `file`, and why.
    async fn impact(
        &self,
        ctx: &Context<'_>,
        repo_id: ID,
        symbol_id: Option<ID>,
        file: Option<String>,
        #[graphql(default = 8)] max_depth: i32,
        #[graphql(default = false)] include_ambiguous: bool,
    ) -> Result<ImpactReport> {
        let state = state(ctx)?;
        let options = impact::ImpactOptions {
            max_depth: depth(max_depth, state.store.limits().max_depth, "maxDepth")?,
            include_ambiguous,
            ..Default::default()
        };
        let (_, graph) = state.graphs.get(&state.store, &repo_id).await.gql()?;
        let report = match (symbol_id, file) {
            (Some(id), None) => {
                impact::impact_of_symbol(&graph, &SymbolId::from_stored(id.0), options)
            }
            (None, Some(path)) => impact::impact_of_file(&graph, &path, options),
            _ => return Err(bad_input("give exactly one of symbolId and file")),
        }
        .map_err(from_impact)?;
        Ok(report.into())
    }

    /// Tests affected by changing `symbolId`, with the chain from each test
    /// to the change (follows calls and trait dispatch).
    async fn affected_tests(
        &self,
        ctx: &Context<'_>,
        repo_id: ID,
        symbol_id: ID,
        #[graphql(default = 8)] max_depth: i32,
    ) -> Result<Vec<AffectedTest>> {
        let state = state(ctx)?;
        let options = impact::ImpactOptions {
            max_depth: depth(max_depth, state.store.limits().max_depth, "maxDepth")?,
            ..Default::default()
        };
        let (_, graph) = state.graphs.get(&state.store, &repo_id).await.gql()?;
        let report = impact::impact_of_symbol(&graph, &SymbolId::from_stored(symbol_id.0), options)
            .map_err(from_impact)?;
        Ok(report
            .affected
            .into_iter()
            .filter(|a| a.symbol.is_test && a.confidence == impact::Confidence::Certain)
            .map(|a| AffectedTest {
                test: a.symbol.into(),
                depth: a.depth as i32,
                path: a.path.into_iter().map(Into::into).collect(),
            })
            .collect())
    }

    /// Strongly connected components at the given level, largest first.
    async fn circular_dependencies(
        &self,
        ctx: &Context<'_>,
        repo_id: ID,
        #[graphql(default_with = "Level::Module")] level: Level,
    ) -> Result<Vec<Cycle>> {
        let state = state(ctx)?;
        let (_, graph) = state.graphs.get(&state.store, &repo_id).await.gql()?;
        Ok(architecture::cycles(&graph, level.into())
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Most central nodes by betweenness, then fan-in.
    async fn hotspots(
        &self,
        ctx: &Context<'_>,
        repo_id: ID,
        #[graphql(default_with = "Level::Function")] level: Level,
        #[graphql(default = 15)] first: i32,
    ) -> Result<Vec<Hotspot>> {
        if !(1..=MAX_HOTSPOTS).contains(&first) {
            return Err(bad_input(format!(
                "first must be between 1 and {MAX_HOTSPOTS}"
            )));
        }
        let state = state(ctx)?;
        let (_, graph) = state.graphs.get(&state.store, &repo_id).await.gql()?;
        Ok(architecture::hotspots(&graph, level.into(), first as usize)
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Dependency layers, foundations (layer 0) first.
    async fn layers(
        &self,
        ctx: &Context<'_>,
        repo_id: ID,
        #[graphql(default_with = "Level::Module")] level: Level,
    ) -> Result<Vec<Layer>> {
        let state = state(ctx)?;
        let (_, graph) = state.graphs.get(&state.store, &repo_id).await.gql()?;
        Ok(architecture::layers(&graph, level.into())
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Source lines of a repository file (at most 400 per request), read
    /// from the repository's working tree on the server.
    async fn source(
        &self,
        ctx: &Context<'_>,
        repo_id: ID,
        file: String,
        start_line: i32,
        end_line: i32,
    ) -> Result<SourceSnippet> {
        let state = state(ctx)?;
        let repository = state.store.repository(&repo_id).await.gql()?;
        let (start, end) = match (u32::try_from(start_line), u32::try_from(end_line)) {
            (Ok(s), Ok(e)) => (s, e),
            _ => return Err(bad_input("line numbers must not be negative")),
        };
        let root = std::path::PathBuf::from(repository.root);
        let file_for_read = file.clone();
        let snippet =
            tokio::task::spawn_blocking(move || read_snippet(&root, &file_for_read, start, end))
                .await
                .map_err(crate::error::internal)?
                .map_err(|err| match err {
                    SourceError::OutsideRepository | SourceError::InvalidRange(_) => {
                        bad_input(err.to_string())
                    }
                    SourceError::NotFound => coded(err.to_string(), "NOT_FOUND"),
                    SourceError::TooLarge | SourceError::Unreadable(_) => {
                        coded(err.to_string(), "SOURCE_UNAVAILABLE")
                    }
                })?;
        Ok(SourceSnippet {
            file,
            start_line: snippet.start_line as i32,
            end_line: snippet.end_line as i32,
            total_lines: snippet.total_lines as i32,
            lines: snippet.lines,
        })
    }
}

fn store_direction(direction: Direction) -> StoreDirection {
    match direction {
        Direction::Dependencies => StoreDirection::Dependencies,
        Direction::Dependents => StoreDirection::Dependents,
    }
}

async fn neighborhood(
    ctx: &Context<'_>,
    repo_id: ID,
    symbol_id: ID,
    max_depth: i32,
    relations: Vec<Relation>,
    direction: StoreDirection,
) -> Result<Neighborhood> {
    let state = state(ctx)?;
    let max_depth = depth(max_depth, state.store.limits().max_depth, "depth")?;
    if relations.is_empty() {
        return Err(bad_input("relations must not be empty"));
    }
    let repo = self::repo_id(state, &repo_id).await?;
    let relations: Vec<_> = relations.into_iter().map(Relation::to_store).collect();
    Ok(state
        .store
        .traverse(&repo, &symbol_id, direction, &relations, max_depth)
        .await
        .gql()?
        .into())
}
