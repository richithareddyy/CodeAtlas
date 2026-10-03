use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use codeatlas_analyzer::graph::CodeGraph;
use codeatlas_store::{GraphStore, Result, StoreError, GRAPH_FORMAT_VERSION};
use tokio::sync::Mutex;

/// Shared state available to every resolver.
pub struct AppState {
    pub store: GraphStore,
    pub graphs: GraphCache,
    pub allow_indexing: bool,
    pub clone_dir: PathBuf,
}

impl AppState {
    pub fn new(store: GraphStore, allow_indexing: bool, clone_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            store,
            graphs: GraphCache::new(8),
            allow_indexing,
            clone_dir,
        })
    }
}

/// In-memory `CodeGraph`s for the impact and architecture resolvers.
///
/// Loading a graph reads the whole repository from Neo4j, so loaded graphs
/// are kept per repository and reused while the repository's `indexed_at`
/// is unchanged; re-indexing invalidates the entry automatically. At most
/// `capacity` repositories are kept.
pub struct GraphCache {
    capacity: usize,
    entries: Mutex<HashMap<String, CacheEntry>>,
}

struct CacheEntry {
    indexed_at: String,
    graph: Arc<CodeGraph>,
    last_used: u64,
}

impl GraphCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// The graph of `repo` (ID or name), loading it if needed.
    pub async fn get(&self, store: &GraphStore, repo: &str) -> Result<(String, Arc<CodeGraph>)> {
        let repository = store.repository(repo).await?;
        // Checked on every access, not only when loading: a cached graph
        // must not outlive a change of the stored format.
        if repository.format_version != GRAPH_FORMAT_VERSION {
            return Err(StoreError::OutdatedIndex {
                repo: repository.name,
                found: repository.format_version,
                expected: GRAPH_FORMAT_VERSION,
            });
        }
        {
            let mut entries = self.entries.lock().await;
            let tick = next_tick(&entries);
            if let Some(entry) = entries.get_mut(&repository.id) {
                if entry.indexed_at == repository.indexed_at {
                    entry.last_used = tick;
                    return Ok((repository.id, entry.graph.clone()));
                }
            }
        }

        let graph = Arc::new(store.load_graph(&repository.id).await?);
        let mut entries = self.entries.lock().await;
        if entries.len() >= self.capacity && !entries.contains_key(&repository.id) {
            let oldest = entries
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(id, _)| id.clone());
            if let Some(oldest) = oldest {
                entries.remove(&oldest);
            }
        }
        let tick = next_tick(&entries);
        entries.insert(
            repository.id.clone(),
            CacheEntry {
                indexed_at: repository.indexed_at,
                graph: graph.clone(),
                last_used: tick,
            },
        );
        Ok((repository.id, graph))
    }

    pub async fn forget(&self, repo_id: &str) {
        self.entries.lock().await.remove(repo_id);
    }

    pub async fn len(&self) -> usize {
        self.entries.lock().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.entries.lock().await.is_empty()
    }
}

fn next_tick(entries: &HashMap<String, CacheEntry>) -> u64 {
    entries.values().map(|e| e.last_used).max().unwrap_or(0) + 1
}
