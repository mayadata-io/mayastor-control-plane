//! Registry containing all mayastor instances which register themselves via the
//! `Register` Message.
//! Said instances may also send `Deregister` to unregister themselves
//! during node/pod shutdown/restart. When this happens the node state is
//! set as `Unknown`. It's TBD how to detect when a node is really going
//! away for good.
//!
//! A mayastor instance sends `Register` every N seconds as sort of a keep
//! alive message.
//! A watchful watchdog is started for each node and it will change the
//! state of said node to `Offline` if it is not petted before its
//! `deadline`.
//!
//! Each instance also contains the known nexus, pools and replicas that live in
//! said instance.
use super::{specs::*, wrapper::NodeWrapper};
use crate::core::{reconciler::ReconcilerControl, wrapper::InternalOps};
use common::errors::SvcError;
use common_lib::{
    store::etcd::Etcd,
    types::v0::{
        message_bus::NodeId,
        store::{
            definitions::{StorableObject, Store, StoreError, StoreKey},
            registry::{CoreRegistryConfig, NodeRegistration},
        },
    },
};
use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
    sync::Arc,
};
use tokio::sync::{Mutex, RwLock};

/// Registry containing all mayastor instances (aka nodes)
#[derive(Clone, Debug)]
pub struct Registry {
    inner: Arc<RegistryInner<Etcd>>,
}

impl Deref for Registry {
    type Target = Arc<RegistryInner<Etcd>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// Generic Registry Inner with a Store trait
#[derive(Debug)]
pub struct RegistryInner<S: Store> {
    /// the actual state of the node
    pub(crate) nodes: Arc<RwLock<HashMap<NodeId, Arc<Mutex<NodeWrapper>>>>>,
    /// spec (aka desired state) of the various resources
    pub(crate) specs: ResourceSpecsLocked,
    /// period to refresh the cache
    cache_period: std::time::Duration,
    pub(crate) store: Arc<Mutex<S>>,
    /// store gRPC operation timeout
    store_timeout: std::time::Duration,
    /// reconciliation period when no work is being done
    pub(crate) reconcile_idle_period: std::time::Duration,
    /// reconciliation period when work is pending
    pub(crate) reconcile_period: std::time::Duration,
    reconciler: ReconcilerControl,
    config: CoreRegistryConfig,
}

impl Registry {
    /// Create a new registry with the `cache_period` to reload the cache, the
    /// `store_url` to connect to, a `store_timeout` for store operations
    /// and a `reconcile_period` for reconcile operations
    pub async fn new(
        cache_period: std::time::Duration,
        store_url: String,
        store_timeout: std::time::Duration,
        reconcile_period: std::time::Duration,
        reconcile_idle_period: std::time::Duration,
    ) -> Self {
        let store_endpoint = Self::format_store_endpoint(&store_url);
        let store = Etcd::new(&store_endpoint)
            .await
            .expect("Should connect to the persistent store");
        tracing::info!("Connected to persistent store at {}", store_endpoint);
        let registry = Self {
            inner: Arc::new(RegistryInner {
                nodes: Default::default(),
                specs: ResourceSpecsLocked::new(),
                cache_period,
                store: Arc::new(Mutex::new(store.clone())),
                store_timeout,
                reconcile_period,
                reconcile_idle_period,
                reconciler: ReconcilerControl::new(),
                config: Self::get_config_or_panic(store).await,
            }),
        };
        registry.init().await;
        registry
    }

    /// Adds the default port to the endpoint if one isn't already specified.
    fn format_store_endpoint(endpoint: &str) -> String {
        let default_port = "2379";
        match endpoint.contains(':') {
            true => endpoint.to_string(),
            false => format!("{}:{}", endpoint, default_port),
        }
    }

    /// Get the `CoreRegistryConfig` from etcd, if it exists, or use the default
    async fn get_config_or_panic<S: Store>(mut store: S) -> CoreRegistryConfig {
        let config = CoreRegistryConfig::new(NodeRegistration::Automatic);
        match store.get_obj(&config.key()).await {
            Ok(config) => config,
            Err(StoreError::MissingEntry { .. }) => config,
            Err(error) => panic!(
                "Must be able to access the persistent store to load configuration information. Got error: '{:#?}'", error
            ),
        }
    }
    /// Get the `CoreRegistryConfig`
    pub(crate) fn config(&self) -> &CoreRegistryConfig {
        &self.config
    }

    /// Serialized write to the persistent store
    pub async fn store_obj<O: StorableObject>(&self, object: &O) -> Result<(), SvcError> {
        let mut store = self.store.lock().await;
        match tokio::time::timeout(
            self.store_timeout,
            async move { store.put_obj(object).await },
        )
        .await
        {
            Ok(result) => result.map_err(Into::into),
            Err(_) => Err(StoreError::Timeout {
                operation: "Put".to_string(),
                timeout: self.store_timeout,
            }
            .into()),
        }
    }

    /// Serialized read from the persistent store
    pub async fn load_obj<O: StorableObject>(&self, key: &O::Key) -> Result<O, SvcError> {
        let mut store = self.store.lock().await;
        match tokio::time::timeout(self.store_timeout, async move { store.get_obj(key).await })
            .await
        {
            Ok(obj) => Ok(obj?),
            Err(_) => Err(StoreError::Timeout {
                operation: "Get".to_string(),
                timeout: self.store_timeout,
            }
            .into()),
        }
    }

    /// Serialized delete to the persistent store
    pub async fn delete_kv<K: StoreKey>(&self, key: &K) -> Result<(), SvcError> {
        let mut store = self.store.lock().await;
        match tokio::time::timeout(
            self.store_timeout,
            async move { store.delete_kv(key).await },
        )
        .await
        {
            Ok(result) => match result {
                Ok(_) => Ok(()),
                // already deleted, no problem
                Err(StoreError::MissingEntry { .. }) => Ok(()),
                Err(error) => Err(SvcError::from(error)),
            },
            Err(_) => Err(SvcError::from(StoreError::Timeout {
                operation: "Delete".to_string(),
                timeout: self.store_timeout,
            })),
        }
    }

    /// Check if the persistent store is currently online
    pub async fn store_online(&self) -> bool {
        let mut store = self.store.lock().await;
        tokio::time::timeout(self.store_timeout, async move { store.online().await })
            .await
            .unwrap_or(false)
    }

    /// Start the worker thread which updates the registry
    pub async fn start(&self) {
        let registry = self.clone();
        tokio::spawn(async move {
            registry.poller().await;
        });
        let registry = self.clone();
        self.reconciler.start(registry).await;
    }

    /// Initialise the registry with the content of the persistent store.
    async fn init(&self) {
        let mut store = self.store.lock().await;
        self.specs.init(store.deref_mut()).await;
    }

    /// Poll each node for resource updates
    async fn poller(&self) {
        loop {
            let nodes = self.nodes.read().await.clone();
            for (_, node) in nodes.iter() {
                let lock = node.grpc_lock().await;
                let _guard = lock.lock().await;

                let mut node_clone = node.lock().await.clone();
                if let Err(e) = node_clone.reload().await {
                    tracing::trace!("Failed to reload node {}. Error {:?}.", node_clone.id, e);
                }
                // update node in the registry
                *node.lock().await = node_clone;
            }
            tokio::time::sleep(self.cache_period).await;
        }
    }
}
