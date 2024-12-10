use crate::v0::core_grpc;
use grpc::operations::node::traits::NodeOperations;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// This is a type to cache the liveness of the agent-core service.
/// This is meant to be wrapped inside an Arc and used across threads.
pub struct CachedCoreState {
    state: Mutex<ServerState>,
    cache_duration: Duration,
}

/// This type remembers a liveness state, and when this data was refreshed.
struct ServerState {
    is_live: bool,
    last_updated: Instant,
}

impl ServerState {
    /// Update the state of the agent-core service, or assume it's unavailable if something
    /// went wrong.
    async fn update_or_assume_unavailable(&mut self) {
        let new_value = core_grpc().node().probe(None).await.unwrap_or(false);
        self.is_live = new_value;
        self.last_updated = Instant::now();
    }
}

impl CachedCoreState {
    /// Create a new cache for serving readiness health checks based on agent-core health.
    pub async fn new(cache_duration: Duration) -> Self {
        let agent_core_is_live = core_grpc().node().probe(None).await.unwrap_or(false);

        CachedCoreState {
            state: Mutex::new(ServerState {
                is_live: agent_core_is_live,
                last_updated: Instant::now(),
            }),
            cache_duration,
        }
    }

    /// Get the cached state of the agent-core service, or assume it's unavailable if something
    /// went wrong.
    pub async fn get_or_assume_unavailable(&self) -> bool {
        let mut state = self.state.lock().await;

        if state.last_updated.elapsed() >= self.cache_duration {
            state.update_or_assume_unavailable().await;
        }

        state.is_live
    }
}
