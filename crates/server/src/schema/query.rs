use std::sync::Arc;

use async_graphql::connection::{self, Connection, Edge};
use async_graphql::{Context, Object, Result, ID};
use codeatlas_analyzer::diff::{analyze_diff, DiffOptions};
use codeatlas_analyzer::error::{AnalyzerError, GitError};
use codeatlas_analyzer::git::Git;
use codeatlas_analyzer::graph::{architecture, impact, test_selection};
use codeatlas_analyzer::ingest::IngestOptions;
use codeatlas_analyzer::model::SymbolId;
use codeatlas_analyzer::RepoSource;
use codeatlas_explain as explain;
use codeatlas_store::{Direction as StoreDirection, StoreError};

use super::types::*;
use crate::error::{bad_input, coded, from_diff, from_impact, StoreResultExt};
use crate::source::{read_snippet, SourceError};
use crate::state::AppState;

/// Upper bound for `first` in `searchSymbols`.
const MAX_PAGE: usize = 50;
/// Upper bound for `first` in `hotspots`.
const MAX_HOTSPOTS: i32 = 100;
/// Upper bound for `first` in `gitRefs`.
const MAX_REFS: i32 = 200;
/// Upper bound for `symbolIds` in `testImpact`.
const MAX_TEST_IMPACT_SYMBOLS: usize = 100;

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

/// Working-tree directory of an indexed repository on the server.
async fn repository_root(state: &AppState, key: &ID) -> Result<std::path::PathBuf> {
    let repository = state.store.repository(key).await.gql()?;
    let root = std::path::PathBuf::from(repository.root);
    if !root.is_dir() {
        return Err(coded(
            format!(
                "repository directory {} is not available on the server",
                root.display()
            ),
            "SOURCE_UNAVAILABLE",
        ));
    }
    Ok(root)
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

    /// Tests to run when the given symbols change (types and modules
    /// include their members): direct and transitive tests with the chain
    /// to the change, and changed code that no test reaches.
    async fn test_impact(
        &self,
        ctx: &Context<'_>,
        repo_id: ID,
        symbol_ids: Vec<ID>,
        #[graphql(default = 8)] max_depth: i32,
        #[graphql(default = false)] include_ambiguous: bool,
    ) -> Result<TestSelection> {
        if symbol_ids.is_empty() || symbol_ids.len() > MAX_TEST_IMPACT_SYMBOLS {
            return Err(bad_input(format!(
                "give between 1 and {MAX_TEST_IMPACT_SYMBOLS} symbolIds"
            )));
        }
        let state = state(ctx)?;
        let options = impact::ImpactOptions {
            max_depth: depth(max_depth, state.store.limits().max_depth, "maxDepth")?,
            include_ambiguous,
            ..Default::default()
        };
        let (_, graph) = state.graphs.get(&state.store, &repo_id).await.gql()?;
        let mut changed = Vec::new();
        for id in &symbol_ids {
            let node = graph
                .node(&SymbolId::from_stored(id.0.clone()))
                .ok_or_else(|| from_impact(impact::ImpactError::UnknownSymbol(id.0.clone())))?;
            changed.extend(impact::expand_change(&graph, node));
        }
        let selection =
            test_selection::select_tests(&graph, &changed, options).map_err(from_impact)?;
        Ok(selection.into())
    }

    /// Why changing `symbolId` can affect `affectedId`, or with no
    /// `affectedId` what changing it affects, as numbered facts from the
    /// graph and an explanation that cites them. The explanation is written
    /// by the configured local model when `useModel` is true and one
    /// answers, and only if it passes the check against the facts;
    /// otherwise it is built from the facts directly. Insufficient evidence
    /// is never sent to a model.
    async fn explain_impact(
        &self,
        ctx: &Context<'_>,
        repo_id: ID,
        symbol_id: ID,
        affected_id: Option<ID>,
        #[graphql(default = 8)] max_depth: i32,
        #[graphql(default = true)] use_model: bool,
    ) -> Result<Explanation> {
        let state = state(ctx)?;
        let options = impact::ImpactOptions {
            max_depth: depth(max_depth, state.store.limits().max_depth, "maxDepth")?,
            ..Default::default()
        };
        let (_, graph) = state.graphs.get(&state.store, &repo_id).await.gql()?;
        let changed = SymbolId::from_stored(symbol_id.0);
        let evidence = match affected_id {
            Some(affected) => explain::evidence::why(
                &graph,
                &changed,
                &SymbolId::from_stored(affected.0),
                options,
            ),
            None => explain::evidence::impact(&graph, &changed, options),
        }
        .map_err(from_impact)?;
        let model = state.model.as_ref().filter(|_| use_model);
        Ok(explain::explain(evidence, model).await.into())
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

    /// Branches, tags and recent commits of the repository on the server,
    /// for choosing what `gitImpact` compares.
    async fn git_refs(
        &self,
        ctx: &Context<'_>,
        repo_id: ID,
        #[graphql(default = 50)] first: i32,
    ) -> Result<GitRefs> {
        if !(1..=MAX_REFS).contains(&first) {
            return Err(bad_input(format!("first must be between 1 and {MAX_REFS}")));
        }
        let root = repository_root(state(ctx)?, &repo_id).await?;
        tokio::task::spawn_blocking(move || -> std::result::Result<GitRefs, AnalyzerError> {
            let git =
                Git::discover(&root)?.ok_or(GitError::NotARepository { path: root.clone() })?;
            Ok(GitRefs {
                current_branch: git.current_branch()?,
                head_sha: git.head_sha()?,
                refs: git
                    .refs(first as usize)?
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                commits: git
                    .recent_commits(first as usize)?
                    .into_iter()
                    .map(|c| Commit {
                        sha: c.sha,
                        date: c.date,
                        subject: c.subject,
                    })
                    .collect(),
            })
        })
        .await
        .map_err(crate::error::internal)?
        .map_err(from_diff)
    }

    /// What changed between `base` and `head` (a branch, tag, SHA or
    /// expression such as `HEAD~1`; the working tree when `head` is null),
    /// symbol by symbol, and what else the changes could affect. Both
    /// revisions are analysed from the repository's Git history on the
    /// server; the stored graph is not used.
    async fn git_impact(
        &self,
        ctx: &Context<'_>,
        repo_id: ID,
        base: String,
        head: Option<String>,
        #[graphql(default = 8)] max_depth: i32,
        #[graphql(default = false)] include_ambiguous: bool,
    ) -> Result<GitImpactReport> {
        let state = state(ctx)?;
        let options = DiffOptions {
            impact: impact::ImpactOptions {
                max_depth: depth(max_depth, state.store.limits().max_depth, "maxDepth")?,
                include_ambiguous,
                ..Default::default()
            },
            ingest: IngestOptions {
                clone_dir: state.clone_dir.clone(),
                ..Default::default()
            },
        };
        let root = repository_root(state, &repo_id).await?;
        let _permit = state
            .diffs
            .acquire()
            .await
            .map_err(crate::error::internal)?;
        let report = tokio::task::spawn_blocking(move || {
            analyze_diff(&RepoSource::Local(root), &base, head.as_deref(), &options)
        })
        .await
        .map_err(crate::error::internal)?
        .map_err(from_diff)?;
        Ok(report.into())
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
