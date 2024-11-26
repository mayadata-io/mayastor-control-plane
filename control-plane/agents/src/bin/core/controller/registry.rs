//! Registry containing all io-engine instances which register themselves via the
//! `Register` Message.
//! Said instances may also send `Deregister` to unregister themselves
//! during node/pod shutdown/restart. When this happens the node state is
//! set as `Unknown`. It's TBD how to detect when a node is really going
//! away for good.
//!
//! An io-engine instance sends `Register` every N seconds as sort of a keep
//! alive message.
//! A watchful watchdog is started for each node and it will change the
//! state of said node to `Offline` if it is not petted before its
//! `deadline`.
//!
//! Each instance also contains the known nexus, pools and replicas that live in
//! said instance.
use super::{resources::operations_helper::*, wrapper::NodeWrapper};
use crate::{
    controller::{
        reconciler::ReconcilerControl,
        task_poller::{PollEvent, PollTriggerEvent},
        wrapper::InternalOps,
    },
    ThinArgs,
};
use agents::errors::SvcError;
use std::{
    collections::HashMap,
    future::Future,
    ops::{Deref, DerefMut},
    sync::Arc,
};
use stor_port::{
    pstor::{
        detect_product_v1_prefix, etcd::Etcd, Error as StoreError, StorableObject, Store, StoreKey,
        StoreKv, StoreObj,
    },
    types::v0::{
        store::{
            nexus_persistence::delete_all_v1_nexus_info,
            registry::{ControlPlaneService, CoreRegistryConfig, NodeRegistration},
            volume::InitiatorAC,
        },
        transport::{DeregisterAppNode, HostNqn, NodeId, RegisterAppNode},
    },
    HostAccessControl,
};
use tokio::sync::{Mutex, RwLock};
use tracing::warn;

/// Registry containing all io-engine instances (aka nodes).
#[derive(Clone, Debug)]
pub(crate) struct Registry {
    inner: Arc<RegistryInner<Etcd>>,
}

/// Map that stores the actual state of the nodes.
pub(crate) type NodesMapLocked = Arc<RwLock<HashMap<NodeId, Arc<RwLock<NodeWrapper>>>>>;

impl Deref for Registry {
    type Target = Arc<RegistryInner<Etcd>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// Number of rebuilds.
pub(crate) type NumRebuilds = u32;

/// Generic Registry Inner with a Store trait.
#[derive(Debug)]
pub(crate) struct RegistryInner<S: Store> {
    /// the actual state of the nodes.
    nodes: NodesMapLocked,
    /// spec (aka desired state) of the various resources.
    specs: ResourceSpecsLocked,
    /// period to refresh the cache.
    cache_period: std::time::Duration,
    store: Arc<Mutex<S>>,
    /// store gRPC operation timeout.
    store_timeout: std::time::Duration,
    /// reconciliation period when no work is being done.
    reconcile_idle_period: std::time::Duration,
    /// reconciliation period when work is pending.
    reconcile_period: std::time::Duration,
    /// The duration for which the reconciler waits for the replica to
    /// to be healthy again before attempting to online the faulted child.
    faulted_child_wait_period: Option<std::time::Duration>,
    /// When the pool creation gRPC times out, the actual call in the io-engine
    /// may still progress.
    /// We wait up to this period before considering the operation a failure and
    /// GC'ing the pool.
    pool_async_creat_tmo: std::time::Duration,
    /// Disable partial rebuild for volume targets.
    disable_partial_rebuild: bool,
    /// Disable nvmf target access control gates.
    disable_target_acc: bool,
    reconciler: ReconcilerControl,
    config: parking_lot::RwLock<CoreRegistryConfig>,
    /// system-wide maximum number of concurrent rebuilds allowed.
    max_rebuilds: Option<NumRebuilds>,
    /// The maximum number of concurrent create volume requests.
    create_volume_limit: usize,
    /// Enablement of host access control.
    host_acl: Vec<HostAccessControl>,
    /// Check for the legacy product version's key prefix present.
    legacy_prefix_present: bool,
    /// Thin provisioning parameters.
    thin_args: ThinArgs,
    /// Check if the HA feature is enabled.
    ha_disabled: bool,
    /// Etcd max page size.
    etcd_max_page_size: i64,
}

impl Registry {
    /// Create a new registry with the `cache_period` to reload the cache, the
    /// `store_url` to connect to, a `store_timeout` for store operations
    /// and a `reconcile_period` for reconcile operations.
    /// todo: move cmdline args into it's own config container.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn new(
        cache_period: std::time::Duration,
        store_url: String,
        store_timeout: std::time::Duration,
        store_lease_tll: std::time::Duration,
        reconcile_period: std::time::Duration,
        reconcile_idle_period: std::time::Duration,
        faulted_child_wait_period: Option<std::time::Duration>,
        pool_async_creat_tmo: std::time::Duration,
        disable_partial_rebuild: bool,
        disable_target_acc: bool,
        max_rebuilds: Option<NumRebuilds>,
        create_volume_limit: usize,
        host_acl: Vec<HostAccessControl>,
        thin_args: ThinArgs,
        ha_enabled: bool,
        etcd_max_page_size: i64,
    ) -> Result<Self, SvcError> {
        let store_endpoint = Self::format_store_endpoint(&store_url);
        tracing::info!("Connecting to persistent store at {}", store_endpoint);
        let mut store = Etcd::new_leased(
            [&store_endpoint],
            ControlPlaneService::CoreAgent.to_string(),
            store_lease_tll,
        )
        .await
        .map_err(|error| StoreError::Generic {
            source: Box::new(error),
            description: "Could not connect to the persistent store".to_string(),
        })?;

        tracing::info!("Connected to persistent store at {}", store_endpoint);

        // Check for the product v1 prefix presence.
        let legacy_prefix_present =
            detect_product_v1_prefix(&mut store)
                .await
                .map_err(|error| StoreError::Generic {
                    source: Box::new(error),
                    description: "Product v1 prefix detection failed".to_string(),
                })?;

        let mut registry = Self {
            inner: Arc::new(RegistryInner {
                nodes: Default::default(),
                specs: ResourceSpecsLocked::new(),
                cache_period,
                store: Arc::new(Mutex::new(store.clone())),
                store_timeout,
                reconcile_period,
                reconcile_idle_period,
                faulted_child_wait_period,
                pool_async_creat_tmo,
                disable_partial_rebuild,
                disable_target_acc,
                reconciler: ReconcilerControl::new(),
                config: parking_lot::RwLock::new(
                    Self::get_config(&mut store, legacy_prefix_present)
                        .await
                        .map_err(|error| StoreError::Generic {
                            source: Box::new(error),
                            description: "Could not get the config".to_string(),
                        })?,
                ),
                max_rebuilds,
                create_volume_limit,
                host_acl,
                legacy_prefix_present,
                thin_args,
                ha_disabled: ha_enabled,
                etcd_max_page_size,
            }),
        };
        registry.init().await?;

        // Disable v1 compat if nexus_info keys are migrated.
        if registry.config().mayastor_compat_v1() && registry.nexus_info_v1_migrated().await? {
            // Delete the v1 nexus_info keys by brute force.
            delete_all_v1_nexus_info(&mut store, etcd_max_page_size)
                .await
                .map_err(|error| StoreError::Generic {
                    source: Box::new(error),
                    description: "Deletion of the v1 nexus_info failed".to_string(),
                })?;
            // Disable the v1 compat mode.
            registry
                .disable_v1_compat(&mut store)
                .await
                .map_err(|error| StoreError::Generic {
                    source: Box::new(error),
                    description: "Disabling of the v1 compatibility mode failed".to_string(),
                })?;
        }

        Ok(registry)
    }

    /// Check if the HA feature is disabled.
    pub(crate) fn ha_disabled(&self) -> bool {
        self.ha_disabled
    }

    /// Check if the partial rebuilds are disabled.
    pub(crate) fn partial_rebuild_disabled(&self) -> bool {
        self.disable_partial_rebuild
    }

    /// Check if the target acc is disabled.
    pub(crate) fn target_acc_disabled(&self) -> bool {
        self.disable_target_acc
    }

    /// Formats the store endpoint with a default port if one isn't supplied.
    fn format_store_endpoint(endpoint: &str) -> String {
        match endpoint.contains(':') {
            true => endpoint.to_string(),
            false => format!("{}:{}", endpoint, "2379"),
        }
    }

    /// Get the `CoreRegistryConfig` from etcd, if it exists, or use the default.
    /// If the mayastor_v1 config exists, then reuse it.
    async fn get_config<S: Store>(
        store: &mut S,
        legacy_prefix_present: bool,
    ) -> Result<CoreRegistryConfig, StoreError> {
        let config = CoreRegistryConfig::new(NodeRegistration::Automatic);
        let mut config = match store.get_obj(&config.key()).await {
            Ok(config) => config,
            Err(StoreError::MissingEntry { .. }) => {
                store.put_obj(&config).await?;
                config
            }
            Err(error) => return Err(error),
        };

        if legacy_prefix_present && !config.mayastor_compat_v1() {
            config.set_mayastor_compat_v1(legacy_prefix_present);
            store.put_obj(&config).await?;
            warn!("Legacy V1 prefix present, enabling compatibility mode");
        }

        Ok(config)
    }

    /// Disable the v1 compat mode, once all nexus info keys are migrated and update
    /// the config.
    pub(crate) async fn disable_v1_compat<S: Store>(
        &mut self,
        store: &mut S,
    ) -> Result<(), StoreError> {
        let mut config = self.config().deref().clone();
        config.set_mayastor_compat_v1(false);
        store.put_obj(&config).await?;
        self.set_config(config);
        Ok(())
    }

    /// Get the thin provisioning configuration parameters.
    pub(crate) fn thin_args(&self) -> &ThinArgs {
        &self.thin_args
    }

    /// Get the `CoreRegistryConfig`.
    pub(crate) fn config(&self) -> parking_lot::RwLockReadGuard<CoreRegistryConfig> {
        self.config.read()
    }

    /// Set the `CoreRegistryConfig`.
    pub(crate) fn set_config(&mut self, new_config: CoreRegistryConfig) {
        let mut config = self.config.write();
        *config = new_config
    }

    /// reconciliation period when no work is being done.
    pub(crate) fn reconcile_idle_period(&self) -> std::time::Duration {
        self.reconcile_idle_period
    }
    /// reconciliation period when work is pending.
    pub(crate) fn reconcile_period(&self) -> std::time::Duration {
        self.reconcile_period
    }
    /// Wait period before attempting to online a faulted child.
    pub(crate) fn faulted_child_wait_period(&self) -> Option<std::time::Duration> {
        self.faulted_child_wait_period
    }
    /// Allow for this given time before assuming failure and allowing the pool to get deleted.
    pub(crate) fn pool_async_creat_tmo(&self) -> std::time::Duration {
        self.pool_async_creat_tmo
    }
    /// The maximum number of concurrent create volume requests.
    pub(crate) fn create_volume_limit(&self) -> usize {
        self.create_volume_limit
    }

    /// Get a reference to the actual state of the nodes.
    pub(crate) fn nodes(&self) -> &NodesMapLocked {
        &self.nodes
    }
    /// Get a reference to the locked resource specs object.
    pub(crate) fn specs(&self) -> &ResourceSpecsLocked {
        &self.specs
    }

    /// Serialized write to the persistent store.
    pub(crate) async fn store_obj<O: StorableObject>(&self, object: &O) -> Result<(), SvcError> {
        let store = self.store.clone();
        match tokio::time::timeout(self.store_timeout, async move {
            // todo: is it still necessary to sync updates to the store?
            //  otherwise should make methods immutable
            let mut store = store.lock().await;
            Self::op_with_threshold(async move { store.put_obj(object).await }).await
        })
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

    /// Serialized read from the persistent store.
    pub(crate) async fn load_obj<O: StorableObject>(&self, key: &O::Key) -> Result<O, SvcError> {
        let store = self.store.clone();
        match tokio::time::timeout(self.store_timeout, async move {
            let mut store = store.lock().await;
            Self::op_with_threshold(async move { store.get_obj(key).await }).await
        })
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

    /// Serialized delete to the persistent store.
    pub(crate) async fn delete_kv<K: StoreKey>(&self, key: &K) -> Result<(), SvcError> {
        let store = self.store.clone();
        match tokio::time::timeout(self.store_timeout, async move {
            let mut store = store.lock().await;
            Self::op_with_threshold(async move { store.delete_kv(key).await }).await
        })
        .await
        {
            Ok(result) => match result {
                Ok(_) => Ok(()),
                // already deleted, no problem
                Err(StoreError::MissingEntry { .. }) => {
                    tracing::warn!("Entry with key {} missing from store.", key.to_string());
                    Ok(())
                }
                Err(error) => Err(SvcError::from(error)),
            },
            Err(_) => Err(SvcError::from(StoreError::Timeout {
                operation: "Delete".to_string(),
                timeout: self.store_timeout,
            })),
        }
    }

    async fn op_with_threshold<F, O>(future: F) -> O
    where
        F: Future<Output = O>,
    {
        let start = std::time::Instant::now();
        let result = future.await;
        let warn_threshold = std::time::Duration::from_secs(1);
        if start.elapsed() > warn_threshold {
            // todo: ratelimit this warning?
            tracing::warn!("Store operation took longer than {:?}", warn_threshold);
        }
        result
    }

    /// Get a reference to the persistent store
    pub(crate) fn store(&self) -> &Arc<Mutex<Etcd>> {
        &self.store
    }

    /// Check if the persistent store is currently online.
    pub(crate) async fn store_online(&self) -> bool {
        let store = self.store.clone();
        tokio::time::timeout(self.store_timeout, async move {
            store.lock().await.online().await
        })
        .await
        .unwrap_or(false)
    }

    /// Start the worker thread which updates the registry.
    pub(crate) async fn start(&self) {
        let registry = self.clone();
        tokio::spawn(async move {
            registry.poller().await;
        });
        let registry = self.clone();
        self.reconciler.start(registry).await;
    }

    /// Stops the core registry, which at the moment only revokes the persistent store lease.
    pub(crate) async fn stop(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(1), async move {
            let store = self.store.lock().await;
            store.revoke().await
        })
        .await
        .ok();
    }

    /// Initialise the registry with the content of the persistent store.
    async fn init(&self) -> Result<(), SvcError> {
        let mut store = self.store.lock().await;
        self.specs
            .init(
                store.deref_mut(),
                self.legacy_prefix_present,
                self.etcd_max_page_size,
            )
            .await?;
        Ok(())
    }

    /// Send a triggered event signal to the reconciler module.
    pub(crate) async fn notify(&self, event: PollTriggerEvent) {
        self.reconciler.notify(PollEvent::Triggered(event)).await
    }

    /// Poll each node for resource updates.
    async fn poller(&self) {
        loop {
            {
                // Clone the nodes so we don't hold the read lock on the nodes list while
                // we may be busy or waiting on node information being fetched.
                let nodes = self.nodes().read().await.clone();

                let polled = nodes.into_values().map(Self::poll_node).collect::<Vec<_>>();

                // Polls all nodes "concurrently".
                // This means we still have to wait for all of them, would it be better to have
                // a completely separate poll task for each node?
                futures::future::join_all(polled).await;
            }
            tokio::time::sleep(self.cache_period).await;
        }
    }

    /// Poll the given node for resource updates.
    async fn poll_node(node: Arc<RwLock<NodeWrapper>>) {
        let (id, online) = {
            let node = node.read().await;
            (node.id().clone(), node.is_online())
        };
        if online {
            if let Err(error) = node.update_all(false).await {
                tracing::error!(node.id = %id, %error, "Failed to reload node");
            }
        }
    }

    /// Determine if a rebuild is allowed to start.
    /// Constrain the number of system-wide rebuilds to the maximum specified.
    /// If a maximum is not specified, do not limit the number of rebuilds.
    pub(crate) async fn rebuild_allowed(&self) -> Result<(), SvcError> {
        match self.max_rebuilds {
            Some(max_rebuilds) => {
                let mut num_rebuilds = 0;
                for (_id, node_wrapper) in self.nodes.read().await.iter() {
                    num_rebuilds += node_wrapper.read().await.num_rebuilds();
                }

                if num_rebuilds < max_rebuilds {
                    Ok(())
                } else {
                    Err(SvcError::MaxRebuilds { max_rebuilds })
                }
            }
            None => Ok(()),
        }
    }

    /// Returns whether or not the node with the given ID is cordoned.
    pub(crate) fn node_cordoned(&self, node_id: &NodeId) -> Result<bool, SvcError> {
        Ok(self.specs.node(node_id)?.cordoned())
    }

    /// Get the allowed host nqn for the given dataplane node.
    pub(crate) async fn node_nqn(&self, node_id: &NodeId) -> Result<Vec<HostNqn>, SvcError> {
        Ok(match self.host_acl.contains(&HostAccessControl::Replicas) {
            true => {
                let node = self.node_state(node_id).await?.node_nqn;
                node.into_iter().collect::<Vec<_>>()
            }
            false => vec![],
        })
    }

    /// Get the allowed nqn's for the given request, if enabled.
    pub(crate) fn host_acl_nodename(
        &self,
        req: HostAccessControl,
        nodes: &[String],
    ) -> Vec<InitiatorAC> {
        match self.host_acl.contains(&req) {
            true => nodes
                .iter()
                .map(|nodename| {
                    InitiatorAC::new(nodename.clone(), HostNqn::from_nodename(nodename))
                })
                .collect(),
            false => vec![],
        }
    }

    /// Register a app node (ex: a csi node) with the control-plane.
    pub(crate) async fn register_app_node(
        &self,
        app_node_spec: &RegisterAppNode,
    ) -> Result<(), SvcError> {
        self.specs().register_app_node(self, app_node_spec).await?;
        Ok(())
    }

    /// Deregister a app node (ex: a csi node) with the control-plane.
    pub(crate) async fn deregister_app_node(
        &self,
        app_node: &DeregisterAppNode,
    ) -> Result<(), SvcError> {
        self.specs().deregister_app_node(self, app_node).await?;
        Ok(())
    }
}
