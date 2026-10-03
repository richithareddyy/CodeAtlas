//! Reads a stored repository back into an in-memory [`CodeGraph`], so the
//! analyzer's algorithms and impact engine run on exactly what is stored.

use codeatlas_analyzer::dependencies::{Dependencies, Dependency, DependencyEvidence};
use codeatlas_analyzer::graph::{CodeGraph, GraphSymbol, Relation, SymbolEdge};
use codeatlas_analyzer::model::{EdgeKind, ResolutionMethod, SymbolId, SymbolKind};
use neo4rs::query;

use crate::error::{Result, StoreError};
use crate::writer::GRAPH_FORMAT_VERSION;
use crate::GraphStore;

impl GraphStore {
    /// Loads a repository's symbols, symbol relationships and derived
    /// dependencies. Fails if the repository is not indexed.
    pub async fn load_graph(&self, repo: &str) -> Result<CodeGraph> {
        let repository = self.repository(repo).await?;
        if repository.format_version != GRAPH_FORMAT_VERSION {
            return Err(StoreError::OutdatedIndex {
                repo: repository.name,
                found: repository.format_version,
                expected: GRAPH_FORMAT_VERSION,
            });
        }

        let rows = self
            .rows(
                query(
                    "MATCH (s:Symbol {repo_id: $repo}) RETURN s.id AS id, s.kind AS kind, \
                     s.name AS name, s.qualified_name AS qualified_name, s.file AS file, \
                     s.start_line AS start_line, s.end_line AS end_line, \
                     s.parent_id AS parent, s.module AS module, s.is_test AS is_test",
                )
                .param("repo", repo),
            )
            .await?;
        let mut symbols = Vec::with_capacity(rows.len());
        for row in &rows {
            let kind: String = row.get("kind")?;
            symbols.push(GraphSymbol {
                id: symbol_id(row.get("id")?),
                kind: SymbolKind::parse(&kind)
                    .ok_or_else(|| StoreError::NotFound(format!("symbol kind `{kind}`")))?,
                name: row.get("name")?,
                qualified_name: row.get("qualified_name")?,
                file: row.get("file")?,
                start_line: line(row.get("start_line")?),
                end_line: line(row.get("end_line")?),
                parent: row.get::<Option<String>>("parent")?.map(symbol_id),
                module: row.get::<Option<String>>("module")?.map(symbol_id),
                is_test: row.get("is_test")?,
            });
        }

        let rows = self
            .rows(
                query(
                    "MATCH (a:Symbol {repo_id: $repo})-[e:CALLS|CALLS_CANDIDATE|IMPORTS|IMPLEMENTS]->(b:Symbol) \
                     RETURN a.id AS from, b.id AS to, type(e) AS type, \
                     e.resolution AS resolution, e.lines AS lines",
                )
                .param("repo", repo),
            )
            .await?;
        let mut edges = Vec::with_capacity(rows.len());
        for row in &rows {
            let rel_type: String = row.get("type")?;
            let resolution: Option<String> = row.get("resolution")?;
            edges.push(SymbolEdge {
                from: symbol_id(row.get("from")?),
                to: symbol_id(row.get("to")?),
                relation: Relation::parse(&rel_type)
                    .ok_or_else(|| StoreError::NotFound(format!("relationship `{rel_type}`")))?,
                resolution: resolution.as_deref().and_then(ResolutionMethod::parse),
                lines: row
                    .get::<Vec<i64>>("lines")?
                    .into_iter()
                    .map(line)
                    .collect(),
            });
        }

        let dependencies = Dependencies {
            files: self
                .stored_dependencies(
                    repo,
                    "MATCH (a:File {repo_id: $repo})-[d:DEPENDS_ON]->(b:File) \
                     RETURN a.path AS from, b.path AS to, d.weight AS weight, \
                     d.via AS via, d.evidence AS evidence",
                )
                .await?,
            modules: self
                .stored_dependencies(
                    repo,
                    "MATCH (a:Module {repo_id: $repo})-[d:DEPENDS_ON]->(b:Module) \
                     RETURN a.id AS from, b.id AS to, d.weight AS weight, \
                     d.via AS via, d.evidence AS evidence",
                )
                .await?,
        };

        Ok(CodeGraph::new(symbols, edges, dependencies))
    }

    async fn stored_dependencies(&self, repo: &str, cypher: &str) -> Result<Vec<Dependency>> {
        let rows = self.rows(query(cypher).param("repo", repo)).await?;
        rows.iter()
            .map(|row| {
                let via: Vec<String> = row.get("via")?;
                let evidence: Vec<String> = row.get("evidence").unwrap_or_default();
                Ok(Dependency {
                    from: row.get("from")?,
                    to: row.get("to")?,
                    weight: row.get::<i64>("weight")? as u32,
                    via: via.iter().filter_map(|v| EdgeKind::parse(v)).collect(),
                    evidence: evidence.iter().filter_map(|e| parse_evidence(e)).collect(),
                })
            })
            .collect()
    }
}

fn symbol_id(text: String) -> SymbolId {
    SymbolId::from_stored(text)
}

fn line(value: i64) -> u32 {
    u32::try_from(value).unwrap_or(0)
}

/// Parses `from|to|KIND|line`.
fn parse_evidence(text: &str) -> Option<DependencyEvidence> {
    let mut parts = text.split('|');
    let (from, to, kind, line_text) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    Some(DependencyEvidence {
        from: symbol_id(from.to_string()),
        to: symbol_id(to.to_string()),
        kind: EdgeKind::parse(kind)?,
        line: line_text.parse().ok()?,
    })
}
