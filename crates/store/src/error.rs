use thiserror::Error;

pub type Result<T, E = StoreError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("neo4j error: {0}")]
    Neo4j(#[from] neo4rs::Error),

    #[error("unexpected value in query result: {0}")]
    Decode(#[from] neo4rs::DeError),

    #[error("{0} not found")]
    NotFound(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("{0}")]
    Ambiguous(String),
}
