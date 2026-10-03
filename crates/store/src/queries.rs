//! Read queries over the code graph. Every query is scoped to one
//! repository and bounded by [`QueryLimits`].

use std::collections::{BTreeMap, HashMap};

use neo4rs::{query, Node, Row};
use serde::Serialize;

use crate::error::{Result, StoreError};
use crate::GraphStore;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepositoryNode {
    pub id: String,
    pub name: String,
    pub root: String,
    pub origin_url: Option<String>,
    pub branch: Option<String>,
    pub head_sha: Option<String>,
    pub indexed_sha: Option<String>,
    pub indexed_at: String,
    /// Graph format version the repository was written with (0 if unknown).
    pub format_version: i64,
    pub source_files: i64,
    pub loc: i64,
    pub languages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SymbolNode {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub qualified_name: String,
    pub file: String,
    pub crate_name: String,
    pub start_line: i64,
    pub end_line: i64,
    pub visibility: String,
    pub signature: Option<String>,
    pub is_test: bool,
    /// `line|callee|reason` for each unresolved call site in this symbol.
    pub unresolved_calls: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    /// Relationship type, e.g. `CALLS`.
    pub kind: String,
    /// How the edge was resolved; absent for `CALLS_CANDIDATE`.
    pub resolution: Option<String>,
    pub lines: Vec<i64>,
}

/// Relationship types a traversal may follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Relation {
    Calls,
    CallsCandidate,
    Imports,
    Implements,
}

impl Relation {
    fn rel_type(self) -> &'static str {
        match self {
            Relation::Calls => "CALLS",
            Relation::CallsCandidate => "CALLS_CANDIDATE",
            Relation::Imports => "IMPORTS",
            Relation::Implements => "IMPLEMENTS",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Direction {
    /// Follow edges forwards: what the start symbol depends on (callees).
    Dependencies,
    /// Follow edges backwards: what depends on the start symbol (callers).
    Dependents,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TraversalNode {
    pub symbol: SymbolNode,
    pub depth: u32,
    /// The edge through which the node was first reached (BFS order, so the
    /// chain of these edges is a shortest evidence path from the root).
    pub via: GraphEdge,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Traversal {
    pub root: SymbolNode,
    pub direction: Direction,
    pub max_depth: u32,
    pub nodes: Vec<TraversalNode>,
    /// Every edge between returned symbols that the traversal followed.
    pub edges: Vec<GraphEdge>,
    /// Set when node or row limits cut the traversal short.
    pub truncated: bool,
}

impl Traversal {
    /// Evidence path from the root to `id`, as edges in traversal order.
    pub fn path_to(&self, id: &str) -> Vec<GraphEdge> {
        let by_id: HashMap<&str, &TraversalNode> = self
            .nodes
            .iter()
            .map(|n| (n.symbol.id.as_str(), n))
            .collect();
        let mut path = Vec::new();
        let mut current = id;
        while let Some(node) = by_id.get(current) {
            path.push(node.via.clone());
            current = match self.direction {
                Direction::Dependencies => node.via.from.as_str(),
                Direction::Dependents => node.via.to.as_str(),
            };
            if current == self.root.id {
                break;
            }
        }
        path.reverse();
        path
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DependencyPath {
    pub nodes: Vec<SymbolNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AggregateDependency {
    /// File path or module ID on the other end.
    pub target: String,
    pub weight: i64,
    pub via: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelatedTest {
    pub test: SymbolNode,
    pub depth: u32,
    /// Call chain from the test down to the symbol.
    pub path: Vec<GraphEdge>,
}

/// Node counts per label and relationship counts per type for one
/// repository's graph.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct GraphStats {
    pub labels: BTreeMap<String, i64>,
    pub relationships: BTreeMap<String, i64>,
}

impl GraphStore {
    pub async fn graph_stats(&self, repo: &str) -> Result<GraphStats> {
        let label_rows = self
            .rows(
                query(
                    "CALL { MATCH (n:Symbol {repo_id: $repo}) RETURN n \
                     UNION ALL MATCH (n:File {repo_id: $repo}) RETURN n \
                     UNION ALL MATCH (n:Crate {repo_id: $repo}) RETURN n \
                     UNION ALL MATCH (n:Repository {id: $repo}) RETURN n } \
                     UNWIND labels(n) AS label RETURN label, count(*) AS count",
                )
                .param("repo", repo),
            )
            .await?;
        let rel_rows = self
            .rows(
                query(
                    "CALL { MATCH (:Symbol {repo_id: $repo})-[e]->() RETURN e \
                     UNION ALL MATCH (:File {repo_id: $repo})-[e]->() RETURN e \
                     UNION ALL MATCH (:Crate {repo_id: $repo})-[e]->() RETURN e \
                     UNION ALL MATCH (:Repository {id: $repo})-[e]->() RETURN e } \
                     RETURN type(e) AS type, count(*) AS count",
                )
                .param("repo", repo),
            )
            .await?;
        let mut stats = GraphStats::default();
        for row in &label_rows {
            stats.labels.insert(row.get("label")?, row.get("count")?);
        }
        for row in &rel_rows {
            stats
                .relationships
                .insert(row.get("type")?, row.get("count")?);
        }
        Ok(stats)
    }

    pub async fn repositories(&self) -> Result<Vec<RepositoryNode>> {
        let rows = self
            .rows(query("MATCH (r:Repository) RETURN r ORDER BY r.name, r.id"))
            .await?;
        rows.iter()
            .map(|row| repository_node(&row.get("r")?))
            .collect()
    }

    /// Finds a repository by ID or, if unique, by name.
    pub async fn repository(&self, id_or_name: &str) -> Result<RepositoryNode> {
        let rows = self
            .rows(
                query(
                    "MATCH (r:Repository) WHERE r.id = $key OR r.name = $key \
                     RETURN r ORDER BY r.id LIMIT 10",
                )
                .param("key", id_or_name),
            )
            .await?;
        let mut repos: Vec<RepositoryNode> = rows
            .iter()
            .map(|row| repository_node(&row.get("r")?))
            .collect::<Result<_>>()?;
        if let Some(exact) = repos.iter().position(|r| r.id == id_or_name) {
            return Ok(repos.swap_remove(exact));
        }
        match repos.len() {
            0 => Err(StoreError::NotFound(format!("repository `{id_or_name}`"))),
            1 => Ok(repos.remove(0)),
            _ => Err(StoreError::Ambiguous(format!(
                "several repositories are named `{id_or_name}`; use an ID: {}",
                repos
                    .iter()
                    .map(|r| r.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    pub async fn symbol(&self, repo: &str, id: &str) -> Result<SymbolNode> {
        self.symbols(repo, &[id.to_string()])
            .await?
            .pop()
            .ok_or_else(|| StoreError::NotFound(format!("symbol `{id}`")))
    }

    /// Symbols by ID, in the order requested; unknown IDs are skipped.
    pub async fn symbols(&self, repo: &str, ids: &[String]) -> Result<Vec<SymbolNode>> {
        let rows = self
            .rows(
                query(
                    "UNWIND range(0, size($ids) - 1) AS i \
                     MATCH (s:Symbol {repo_id: $repo, id: $ids[i]}) RETURN s ORDER BY i",
                )
                .param("repo", repo)
                .param("ids", ids.to_vec()),
            )
            .await?;
        rows.iter().map(|row| symbol_node(&row.get("s")?)).collect()
    }

    /// Resolves user input to one symbol: an exact ID, a qualified name, a
    /// qualified-name suffix (`PaymentService::authorize`) or a unique name.
    pub async fn find_symbol(&self, repo: &str, text: &str) -> Result<SymbolNode> {
        self.find_symbol_preferring(repo, text, &[]).await
    }

    /// Like [`find_symbol`](Self::find_symbol), but when several symbols
    /// match and exactly one has a kind in `preferred`, that one is chosen
    /// (e.g. the function `checkout` over the module `checkout`).
    pub async fn find_symbol_preferring(
        &self,
        repo: &str,
        text: &str,
        preferred: &[&str],
    ) -> Result<SymbolNode> {
        let attempts = [
            "MATCH (s:Symbol {repo_id: $repo, id: $text}) RETURN s",
            "MATCH (s:Symbol {repo_id: $repo, qualified_name: $text}) RETURN s ORDER BY s.id LIMIT 11",
            "MATCH (s:Symbol {repo_id: $repo}) WHERE s.qualified_name ENDS WITH '::' + $text \
             RETURN s ORDER BY s.id LIMIT 11",
            "MATCH (s:Symbol {repo_id: $repo, name: $text}) RETURN s ORDER BY s.id LIMIT 11",
        ];
        for cypher in attempts {
            let rows = self
                .rows(query(cypher).param("repo", repo).param("text", text))
                .await?;
            let found: Vec<SymbolNode> = rows
                .iter()
                .map(|row| symbol_node(&row.get("s")?))
                .collect::<Result<_>>()?;
            let mut preferred_matches: Vec<&SymbolNode> = found
                .iter()
                .filter(|s| preferred.contains(&s.kind.as_str()))
                .collect();
            if found.len() > 1 && preferred_matches.len() == 1 {
                return Ok(preferred_matches.remove(0).clone());
            }
            match found.len() {
                0 => continue,
                1 => return Ok(found.into_iter().next().unwrap_or_else(|| unreachable!())),
                _ => {
                    let ids: Vec<&str> = found.iter().take(10).map(|s| s.id.as_str()).collect();
                    return Err(StoreError::Ambiguous(format!(
                        "`{text}` matches several symbols; use one of: {}",
                        ids.join(", ")
                    )));
                }
            }
        }
        Err(StoreError::NotFound(format!("symbol `{text}`")))
    }

    /// Full-text search over symbol names and qualified names. Terms are
    /// matched as prefixes; exact name matches rank first.
    pub async fn search(
        &self,
        repo: &str,
        text: &str,
        kinds: Option<&[String]>,
        limit: usize,
    ) -> Result<Vec<SymbolNode>> {
        let limit = limit.clamp(1, self.limits.max_results);
        let lucene = lucene_prefix_query(text).ok_or_else(|| {
            StoreError::InvalidArgument("search text has no searchable characters".into())
        })?;
        let kinds: Option<Vec<String>> = kinds.map(<[String]>::to_vec);
        let rows = self
            .rows(
                query(
                    "CALL db.index.fulltext.queryNodes('symbol_search', $lucene) YIELD node, score \
                     WHERE node.repo_id = $repo AND ($kinds IS NULL OR node.kind IN $kinds) \
                     RETURN node ORDER BY toLower(node.name) = $exact DESC, score DESC, \
                     size(node.qualified_name), node.qualified_name LIMIT $limit",
                )
                .param("lucene", lucene)
                .param("repo", repo)
                .param("kinds", kinds)
                .param("exact", text.trim().to_lowercase())
                .param("limit", limit as i64),
            )
            .await?;
        rows.iter()
            .map(|row| symbol_node(&row.get("node")?))
            .collect()
    }

    /// Breadth-first traversal from `start`, one Cypher query per level.
    pub async fn traverse(
        &self,
        repo: &str,
        start: &str,
        direction: Direction,
        relations: &[Relation],
        depth: u32,
    ) -> Result<Traversal> {
        self.check_depth(depth)?;
        if relations.is_empty() {
            return Err(StoreError::InvalidArgument(
                "no relationship types given".into(),
            ));
        }
        let root = self.symbol(repo, start).await?;
        let types = relations
            .iter()
            .map(|r| r.rel_type())
            .collect::<Vec<_>>()
            .join("|");
        let pattern = match direction {
            Direction::Dependencies => format!("(s)-[e:{types}]->(t:Symbol)"),
            Direction::Dependents => format!("(s)<-[e:{types}]-(t:Symbol)"),
        };
        let cypher = format!(
            "UNWIND $frontier AS fid MATCH (s:Symbol {{repo_id: $repo, id: fid}}) \
             MATCH {pattern} \
             RETURN fid AS source, t AS node, type(e) AS kind, e.resolution AS resolution, \
             e.lines AS lines ORDER BY source, t.id, kind LIMIT $cap"
        );
        // Rows per level: enough for every allowed node to be reached
        // through several edges, but never unbounded.
        let row_cap = (self.limits.max_nodes * 20) as i64;

        let mut nodes: Vec<TraversalNode> = Vec::new();
        let mut seen: HashMap<String, usize> = HashMap::from([(root.id.clone(), usize::MAX)]);
        let mut edges = Vec::new();
        let mut frontier = vec![root.id.clone()];
        let mut truncated = false;

        for level in 1..=depth {
            if frontier.is_empty() {
                break;
            }
            let rows = self
                .rows(
                    query(&cypher)
                        .param("repo", repo)
                        .param("frontier", frontier.clone())
                        .param("cap", row_cap),
                )
                .await?;
            truncated |= rows.len() as i64 >= row_cap;

            let mut next = Vec::new();
            for row in &rows {
                let source: String = row.get("source")?;
                let neighbour = symbol_node(&row.get("node")?)?;
                let (from, to) = match direction {
                    Direction::Dependencies => (source, neighbour.id.clone()),
                    Direction::Dependents => (neighbour.id.clone(), source),
                };
                let edge = GraphEdge {
                    from,
                    to,
                    kind: row.get("kind")?,
                    resolution: row.get("resolution").ok(),
                    lines: row.get("lines").unwrap_or_default(),
                };
                if !seen.contains_key(&neighbour.id) {
                    if nodes.len() >= self.limits.max_nodes {
                        truncated = true;
                        continue;
                    }
                    seen.insert(neighbour.id.clone(), nodes.len());
                    next.push(neighbour.id.clone());
                    nodes.push(TraversalNode {
                        symbol: neighbour,
                        depth: level,
                        via: edge.clone(),
                    });
                }
                edges.push(edge);
            }
            frontier = next;
        }

        Ok(Traversal {
            root,
            direction,
            max_depth: depth,
            nodes,
            edges,
            truncated,
        })
    }

    /// Direct callers (`depth` 1) or transitive callers of a symbol.
    pub async fn callers(&self, repo: &str, id: &str, depth: u32) -> Result<Traversal> {
        self.traverse(repo, id, Direction::Dependents, &[Relation::Calls], depth)
            .await
    }

    /// Direct callees (`depth` 1) or transitive callees of a symbol.
    pub async fn callees(&self, repo: &str, id: &str, depth: u32) -> Result<Traversal> {
        self.traverse(repo, id, Direction::Dependencies, &[Relation::Calls], depth)
            .await
    }

    /// Tests that reach `id` through resolved calls within `depth` steps,
    /// each with the call chain that connects it.
    pub async fn related_tests(
        &self,
        repo: &str,
        id: &str,
        depth: u32,
    ) -> Result<Vec<RelatedTest>> {
        let traversal = self.callers(repo, id, depth).await?;
        let mut tests: Vec<RelatedTest> = traversal
            .nodes
            .iter()
            .filter(|n| n.symbol.is_test)
            .map(|n| {
                let mut path = traversal.path_to(&n.symbol.id);
                // Present the chain from the test downwards.
                path.reverse();
                RelatedTest {
                    test: n.symbol.clone(),
                    depth: n.depth,
                    path,
                }
            })
            .collect();
        tests.sort_by(|a, b| a.depth.cmp(&b.depth).then(a.test.id.cmp(&b.test.id)));
        Ok(tests)
    }

    /// Shortest directed path from `from` to `to` over resolved CALLS,
    /// IMPORTS and IMPLEMENTS edges, or `None` within `max_depth`.
    pub async fn shortest_path(
        &self,
        repo: &str,
        from: &str,
        to: &str,
        max_depth: u32,
    ) -> Result<Option<DependencyPath>> {
        self.check_depth(max_depth)?;
        let start = self.symbol(repo, from).await?;
        if from == to {
            return Ok(Some(DependencyPath {
                nodes: vec![start],
                edges: vec![],
            }));
        }
        self.symbol(repo, to).await?;
        let cypher = format!(
            "MATCH (a:Symbol {{repo_id: $repo, id: $from}}), (b:Symbol {{repo_id: $repo, id: $to}}) \
             MATCH p = shortestPath((a)-[:CALLS|IMPORTS|IMPLEMENTS*..{max_depth}]->(b)) \
             RETURN [n IN nodes(p) | n.id] AS ids, \
             [r IN relationships(p) | type(r)] AS kinds, \
             [r IN relationships(p) | r.resolution] AS resolutions, \
             [r IN relationships(p) | r.lines] AS lines"
        );
        let rows = self
            .rows(
                query(&cypher)
                    .param("repo", repo)
                    .param("from", from)
                    .param("to", to),
            )
            .await?;
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        let ids: Vec<String> = row.get("ids")?;
        let kinds: Vec<String> = row.get("kinds")?;
        let resolutions: Vec<Option<String>> = row.get("resolutions")?;
        let lines: Vec<Vec<i64>> = row.get("lines")?;
        let edges = ids
            .windows(2)
            .zip(kinds)
            .zip(resolutions)
            .zip(lines)
            .map(|(((pair, kind), resolution), lines)| GraphEdge {
                from: pair[0].clone(),
                to: pair[1].clone(),
                kind,
                resolution,
                lines,
            })
            .collect();
        Ok(Some(DependencyPath {
            nodes: self.symbols(repo, &ids).await?,
            edges,
        }))
    }

    /// Files that depend on `path` (`Direction::Dependents`) or that `path`
    /// depends on (`Direction::Dependencies`).
    pub async fn file_dependencies(
        &self,
        repo: &str,
        path: &str,
        direction: Direction,
    ) -> Result<Vec<AggregateDependency>> {
        let exists = self
            .rows(
                query("MATCH (f:File {repo_id: $repo, path: $path}) RETURN f.path AS path")
                    .param("repo", repo)
                    .param("path", path),
            )
            .await?;
        if exists.is_empty() {
            return Err(StoreError::NotFound(format!("file `{path}`")));
        }
        let pattern = match direction {
            Direction::Dependents => "(f)<-[d:DEPENDS_ON]-(o:File)",
            Direction::Dependencies => "(f)-[d:DEPENDS_ON]->(o:File)",
        };
        let cypher = format!(
            "MATCH (f:File {{repo_id: $repo, path: $path}}) MATCH {pattern} \
             RETURN o.path AS target, d.weight AS weight, d.via AS via \
             ORDER BY weight DESC, target LIMIT $limit"
        );
        self.aggregate_dependencies(query(&cypher).param("repo", repo).param("path", path))
            .await
    }

    /// Modules that depend on module `id`, or that it depends on.
    pub async fn module_dependencies(
        &self,
        repo: &str,
        id: &str,
        direction: Direction,
    ) -> Result<Vec<AggregateDependency>> {
        let module = self.symbol(repo, id).await?;
        if module.kind != "module" {
            return Err(StoreError::InvalidArgument(format!(
                "`{id}` is not a module"
            )));
        }
        let pattern = match direction {
            Direction::Dependents => "(m)<-[d:DEPENDS_ON]-(o:Module)",
            Direction::Dependencies => "(m)-[d:DEPENDS_ON]->(o:Module)",
        };
        let cypher = format!(
            "MATCH (m:Module {{repo_id: $repo, id: $id}}) MATCH {pattern} \
             RETURN o.id AS target, d.weight AS weight, d.via AS via \
             ORDER BY weight DESC, target LIMIT $limit"
        );
        self.aggregate_dependencies(query(&cypher).param("repo", repo).param("id", id))
            .await
    }

    async fn aggregate_dependencies(&self, q: neo4rs::Query) -> Result<Vec<AggregateDependency>> {
        let rows = self
            .rows(q.param("limit", self.limits.max_nodes as i64))
            .await?;
        rows.iter()
            .map(|row| {
                Ok(AggregateDependency {
                    target: row.get("target")?,
                    weight: row.get("weight")?,
                    via: row.get("via")?,
                })
            })
            .collect()
    }

    fn check_depth(&self, depth: u32) -> Result<()> {
        if depth == 0 || depth > self.limits.max_depth {
            return Err(StoreError::InvalidArgument(format!(
                "depth must be between 1 and {}",
                self.limits.max_depth
            )));
        }
        Ok(())
    }

    pub(crate) async fn rows(&self, q: neo4rs::Query) -> Result<Vec<Row>> {
        let mut stream = self.graph.execute(q).await?;
        let mut rows = Vec::new();
        while let Some(row) = stream.next().await? {
            rows.push(row);
        }
        Ok(rows)
    }
}

fn symbol_node(node: &Node) -> Result<SymbolNode> {
    Ok(SymbolNode {
        id: node.get("id")?,
        kind: node.get("kind")?,
        name: node.get("name")?,
        qualified_name: node.get("qualified_name")?,
        file: node.get("file")?,
        crate_name: node.get("crate")?,
        start_line: node.get("start_line")?,
        end_line: node.get("end_line")?,
        visibility: node.get("visibility")?,
        signature: node.get("signature").ok(),
        is_test: node.get("is_test")?,
        unresolved_calls: node.get("unresolved_calls").unwrap_or_default(),
    })
}

fn repository_node(node: &Node) -> Result<RepositoryNode> {
    Ok(RepositoryNode {
        id: node.get("id")?,
        name: node.get("name")?,
        root: node.get("root")?,
        origin_url: node.get("origin_url").ok(),
        branch: node.get("branch").ok(),
        head_sha: node.get("head_sha").ok(),
        indexed_sha: node.get("indexed_sha").ok(),
        indexed_at: node.get("indexed_at")?,
        format_version: node.get("format_version").unwrap_or(0),
        source_files: node.get("source_files")?,
        loc: node.get("loc")?,
        languages: node.get("languages").unwrap_or_default(),
    })
}

/// Builds a Lucene query matching every term as a prefix. Only
/// `[A-Za-z0-9_]` survive, so no Lucene syntax can be injected.
fn lucene_prefix_query(text: &str) -> Option<String> {
    let terms: Vec<String> = text
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty())
        .map(|t| format!("{}*", t.to_lowercase()))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" AND "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_safe_prefix_queries() {
        assert_eq!(
            lucene_prefix_query("PaymentService.authorize").as_deref(),
            Some("paymentservice* AND authorize*")
        );
        assert_eq!(
            lucene_prefix_query("stripe_call").as_deref(),
            Some("stripe_call*")
        );
        assert_eq!(
            lucene_prefix_query("a) OR (b:*").as_deref(),
            Some("a* AND or* AND b*")
        );
        assert_eq!(lucene_prefix_query(" :: "), None);
    }

    fn edge(from: &str, to: &str) -> GraphEdge {
        GraphEdge {
            from: from.into(),
            to: to.into(),
            kind: "CALLS".into(),
            resolution: None,
            lines: vec![],
        }
    }

    fn symbol(id: &str) -> SymbolNode {
        SymbolNode {
            id: id.into(),
            kind: "function".into(),
            name: id.into(),
            qualified_name: id.into(),
            file: "f.rs".into(),
            crate_name: "c".into(),
            start_line: 1,
            end_line: 1,
            visibility: "pub".into(),
            signature: None,
            is_test: false,
            unresolved_calls: vec![],
        }
    }

    #[test]
    fn reconstructs_evidence_paths_for_dependents() {
        // c calls b calls a; traversal of a's dependents.
        let traversal = Traversal {
            root: symbol("a"),
            direction: Direction::Dependents,
            max_depth: 2,
            nodes: vec![
                TraversalNode {
                    symbol: symbol("b"),
                    depth: 1,
                    via: edge("b", "a"),
                },
                TraversalNode {
                    symbol: symbol("c"),
                    depth: 2,
                    via: edge("c", "b"),
                },
            ],
            edges: vec![edge("b", "a"), edge("c", "b")],
            truncated: false,
        };
        let path: Vec<String> = traversal
            .path_to("c")
            .iter()
            .map(|e| format!("{}->{}", e.from, e.to))
            .collect();
        assert_eq!(path, vec!["b->a", "c->b"]);
    }
}
