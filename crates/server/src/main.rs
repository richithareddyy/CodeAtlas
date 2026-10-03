use anyhow::{Context, Result};
use codeatlas_server::{router, schema, AppState, ServerConfig};
use codeatlas_store::{GraphStore, StoreConfig};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--print-schema") {
        println!("{}", schema::build(None).sdl());
        return Ok(());
    }

    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info,neo4rs=warn")),
        )
        .init();

    let config = ServerConfig::from_env()?;
    let store_config = StoreConfig::from_env()?;
    let store = GraphStore::connect(&store_config)
        .await
        .with_context(|| format!("cannot connect to Neo4j at {}", store_config.uri))?;
    store.ensure_schema().await?;

    if config.allow_indexing && !config.addr.ip().is_loopback() {
        tracing::warn!(
            addr = %config.addr,
            "indexing mutations are enabled on a non-loopback address; anyone who can reach \
             the server can make it read local paths or clone URLs \
             (set CODEATLAS_ALLOW_INDEXING=false to disable)"
        );
    }

    let state = AppState::new(store, config.allow_indexing, config.clone_dir.clone());
    let app = router(state, &config);
    let listener = tokio::net::TcpListener::bind(config.addr)
        .await
        .with_context(|| format!("cannot listen on {}", config.addr))?;
    tracing::info!(
        addr = %config.addr,
        graphiql = config.graphiql,
        indexing = config.allow_indexing,
        "CodeAtlas GraphQL server listening"
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
