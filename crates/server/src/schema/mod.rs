//! The GraphQL schema. See `docs/schema.graphql` for the generated SDL.

mod mutation;
mod query;
pub mod types;

use std::sync::Arc;

use async_graphql::{EmptySubscription, Schema};

pub use mutation::MutationRoot;
pub use query::QueryRoot;

use crate::state::AppState;

pub type AppSchema = Schema<QueryRoot, MutationRoot, EmptySubscription>;

/// Maximum nesting depth of a query document.
pub const MAX_QUERY_DEPTH: usize = 12;
/// Maximum static complexity (fields counted once each).
pub const MAX_QUERY_COMPLEXITY: usize = 1_000;

/// Builds the schema. Without state it can only print its SDL.
pub fn build(state: Option<Arc<AppState>>) -> AppSchema {
    let mut builder = Schema::build(QueryRoot, MutationRoot, EmptySubscription)
        .limit_depth(MAX_QUERY_DEPTH)
        .limit_complexity(MAX_QUERY_COMPLEXITY);
    if let Some(state) = state {
        builder = builder.data(state);
    }
    builder.finish()
}
