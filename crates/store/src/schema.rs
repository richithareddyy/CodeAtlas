//! Constraints and indexes. See `docs/graph-model.md`.

use neo4rs::{query, Graph};

use crate::error::Result;

pub const STATEMENTS: &[&str] = &[
    "CREATE CONSTRAINT repository_id IF NOT EXISTS FOR (r:Repository) REQUIRE r.id IS UNIQUE",
    "CREATE CONSTRAINT symbol_key IF NOT EXISTS FOR (s:Symbol) REQUIRE (s.repo_id, s.id) IS UNIQUE",
    "CREATE CONSTRAINT file_key IF NOT EXISTS FOR (f:File) REQUIRE (f.repo_id, f.path) IS UNIQUE",
    "CREATE CONSTRAINT crate_key IF NOT EXISTS FOR (c:Crate) REQUIRE (c.repo_id, c.id) IS UNIQUE",
    "CREATE INDEX symbol_repo IF NOT EXISTS FOR (s:Symbol) ON (s.repo_id)",
    "CREATE INDEX symbol_name IF NOT EXISTS FOR (s:Symbol) ON (s.repo_id, s.name)",
    "CREATE INDEX symbol_qualified_name IF NOT EXISTS FOR (s:Symbol) ON (s.repo_id, s.qualified_name)",
    "CREATE INDEX symbol_file IF NOT EXISTS FOR (s:Symbol) ON (s.repo_id, s.file)",
    "CREATE INDEX file_repo IF NOT EXISTS FOR (f:File) ON (f.repo_id)",
    "CREATE INDEX crate_repo IF NOT EXISTS FOR (c:Crate) ON (c.repo_id)",
    "CREATE FULLTEXT INDEX symbol_search IF NOT EXISTS FOR (s:Symbol) ON EACH [s.name, s.qualified_name]",
];

/// Creates missing constraints and indexes; safe to run repeatedly and
/// concurrently. `IF NOT EXISTS` is not atomic across concurrent
/// transactions, so losing that race ("already exists") counts as success.
pub async fn ensure(graph: &Graph) -> Result<()> {
    for statement in STATEMENTS {
        match graph.run(query(statement)).await {
            Ok(()) => {}
            Err(neo4rs::Error::Neo4j(err)) if is_already_exists(err.code()) => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

fn is_already_exists(code: &str) -> bool {
    matches!(
        code,
        "Neo.ClientError.Schema.EquivalentSchemaRuleAlreadyExists"
            | "Neo.ClientError.Schema.ConstraintAlreadyExists"
            | "Neo.ClientError.Schema.IndexAlreadyExists"
    )
}
