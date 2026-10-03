//! HTTP server exposing CodeAtlas through GraphQL.
//!
//! * `POST /graphql`: GraphQL endpoint ([`schema`]).
//! * `GET /graphql`: GraphiQL (when enabled).
//! * `GET /health`: liveness plus a Neo4j round trip.

pub mod config;
pub mod error;
pub mod schema;
pub mod source;
pub mod state;

use std::sync::Arc;

use async_graphql::http::GraphiQLSource;
use async_graphql_axum::GraphQL;
use axum::extract::State;
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post_service};
use axum::{Json, Router};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;

pub use config::ServerConfig;
pub use schema::AppSchema;
pub use state::AppState;

pub fn router(state: Arc<AppState>, config: &ServerConfig) -> Router {
    let schema = schema::build(Some(state.clone()));
    let graphql = if config.graphiql {
        get(graphiql).post_service(GraphQL::new(schema))
    } else {
        post_service(GraphQL::new(schema))
    };
    let origins: Vec<HeaderValue> = config
        .cors_origins
        .iter()
        .filter_map(|o| HeaderValue::from_str(o).ok())
        .collect();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::CONTENT_TYPE]);

    Router::new()
        .route("/health", get(health))
        .route("/graphql", graphql)
        .with_state(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http())
}

async fn graphiql() -> impl IntoResponse {
    Html(GraphiQLSource::build().endpoint("/graphql").finish())
}

async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.store.repositories().await {
        Ok(repositories) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "ok",
                "neo4j": "ok",
                "repositories": repositories.len(),
            })),
        ),
        Err(error) => {
            tracing::warn!(%error, "health check failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "status": "unavailable", "neo4j": "unreachable" })),
            )
        }
    }
}
