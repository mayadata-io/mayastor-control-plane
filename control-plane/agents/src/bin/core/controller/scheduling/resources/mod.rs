use crate::controller::{
    registry::Registry,
    wrapper::{GetterOps, NodeWrapper, PoolWrapper},
};

use stor_port::types::v0::{
    store::{
        nexus_child::NexusChild,
        nexus_persistence::{ChildInfo, NexusInfo},
        replica::ReplicaSpec,
        snapshots::replica::ReplicaSnapshot,
        volume::VolumeSpec,
    },
    transport::{Child, ChildUri, NodeId, PoolId, Replica},
};

use std::{collections::HashMap, ops::Deref};

/// Item for pool scheduling logic.
#[derive(Debug, Clone)]
pub(crate) struct PoolItem {
    /// The node where this pools lives.
    pub(crate) node: NodeWrapper,
    /// The pool.
    pub(crate) pool: PoolWrapper,
    /// Number of replicas of a ag present on the pool.
    /// This would have a value if the volume is a part of
    /// a Affinity Group and the already created volumes have replicas
    /// on this pool.
    pub(crate) ag_replica_count: Option<u64>,
}

impl PoolItem {
    /// Create a new `Self`.
    pub(crate) fn new(node: NodeWrapper, pool: PoolWrapper, ag_replica_count: Option<u64>) -> Self {
        Self {
            node,
            pool,
            ag_replica_count,
        }
    }
    /// Get the number of replicas in the pool.
    pub(crate) fn len(&self) -> u64 {
        self.pool.replicas().len() as u64
    }
    /// Get the number of Affinity Group replicas in the pool.
    pub(crate) fn ag_replica_count(&self) -> u64 {
        self.ag_replica_count.unwrap_or(u64::MIN)
    }
    /// Get a reference to the inner `PoolWrapper`.
    pub(crate) fn pool(&self) -> &PoolWrapper {
        &self.pool
    }
    /// Collect the item into a pool.
    pub(crate) fn collect(self) -> PoolWrapper {
        self.pool
    }
}

/// A pool lister.
pub(crate) struct PoolItemLister {}
impl PoolItemLister {
    async fn nodes(registry: &Registry) -> Vec<NodeWrapper> {
        let nodes = registry.node_wrappers().await;
        let mut raw_nodes = vec![];
        for node in nodes {
            let node = node.read().await;
            raw_nodes.push(node.clone());
        }
        raw_nodes
    }
    /// Get a list of pool items.
    pub(crate) async fn list(
        registry: &Registry,
        pool_ag_rep: &Option<HashMap<PoolId, u64>>,
    ) -> Vec<PoolItem> {
        let pools = Self::nodes(registry)
            .await
            .iter()
            .flat_map(|n| {
                n.pool_wrappers()
                    .iter()
                    .filter(|p| registry.specs().pool(&p.id).is_ok())
                    .map(|p| {
                        let ag_rep_count =
                            pool_ag_rep.as_ref().and_then(|map| map.get(&p.id).cloned());

                        PoolItem::new(n.clone(), p.clone(), ag_rep_count)
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        pools
    }
    /// Get a list of pool items to create a snapshot on.
    pub(crate) async fn list_for_snaps(registry: &Registry, items: &[ChildItem]) -> Vec<PoolItem> {
        let nodes = Self::nodes(registry).await;
        let pool_items = items
            .iter()
            .filter_map(|item| {
                nodes
                    .iter()
                    .find(|node| node.id() == item.node())
                    .map(|node| PoolItem::new(node.clone(), item.pool().clone(), None))
            })
            .collect();
        pool_items
    }
    /// Get a list of replicas wrapped as ChildItem, for resize.
    pub(crate) async fn list_for_resize(registry: &Registry, spec: &VolumeSpec) -> Vec<ChildItem> {
        let replicas = registry.specs().volume_replicas(&spec.uuid);
        let mut state_replicas = Vec::with_capacity(replicas.len());
        for replica in &replicas {
            if let Ok(replica) = registry.replica(replica.uuid()).await {
                state_replicas.push(replica);
            }
        }
        let pool_wrappers = registry.pool_wrappers().await;

        replicas
            .iter()
            .filter_map(|replica_spec| {
                let replica_spec = replica_spec.lock().clone();
                let replica_state = state_replicas
                    .iter()
                    .find(|state| state.uuid == replica_spec.uuid);

                let pool_id = replica_spec.pool.pool_name();
                pool_wrappers
                    .iter()
                    .find(|p| &p.id == pool_id)
                    .and_then(|pool| {
                        replica_state.map(|replica_state| {
                            ChildItem::new(&replica_spec, replica_state, None, pool, None)
                        })
                    })
            })
            .collect()
    }

    /// Get a list of pool items to create a snapshot clone on.
    pub(crate) async fn list_for_clones(
        registry: &Registry,
        snapshots: &[ReplicaSnapshot],
    ) -> Vec<PoolItem> {
        let mut pool_items = vec![];
        for snapshot in snapshots {
            let pool_id = snapshot.spec().source_id().pool_id();
            let Ok(pool_spec) = registry.specs().pool(pool_id) else {
                continue;
            };
            let Ok(node) = registry.node_wrapper(&pool_spec.node).await else {
                continue;
            };
            let Some(pool) = node.pool_wrapper(pool_id).await else {
                continue;
            };
            let node = node.read().await.deref().clone();
            pool_items.push(PoolItem::new(node, pool, None));
        }
        pool_items
    }
}

/// A replica item used for scheduling.
#[derive(Debug, Clone)]
pub(crate) struct ReplicaItem {
    replica_spec: ReplicaSpec,
    replica_state: Option<Replica>,
    child_uri: Option<ChildUri>,
    child_state: Option<Child>,
    child_spec: Option<NexusChild>,
    child_info: Option<ChildInfo>,
    /// Number of replicas of a ag present on the pool of this replica.
    /// This would have a value if the volume is a part of
    /// a Affinity Group and the already created volumes have replicas
    /// on this pool.
    ag_replicas_on_pool: Option<u64>,
    valid_pool_topology: Option<bool>,
    valid_node_topology: Option<bool>,
}

impl ReplicaItem {
    /// Create new `Self` from the provided arguments.
    pub(crate) fn new(
        replica: ReplicaSpec,
        replica_state: Option<&Replica>,
        child_uri: Option<ChildUri>,
        child_state: Option<Child>,
        child_spec: Option<NexusChild>,
        child_info: Option<ChildInfo>,
        ag_replicas_on_pool: Option<u64>,
    ) -> Self {
        Self {
            replica_spec: replica,
            replica_state: replica_state.cloned(),
            child_uri,
            child_state,
            child_spec,
            child_info,
            ag_replicas_on_pool,
            valid_pool_topology: None,
            valid_node_topology: None,
        }
    }
    /// Set the validity of the replica's node topology.
    /// If set to false, this means the replica is not in sync with the volume topology and
    /// therefore should be replaced by another replica which is in sync.
    pub(crate) fn with_node_topology(mut self, valid_node_topology: Option<bool>) -> ReplicaItem {
        self.valid_node_topology = valid_node_topology;
        self
    }
    /// Set the validity of the replica's pool topology.
    /// If set to false, this means the replica is not in sync with the volume topology and
    /// therefore should be replaced by another replica which is in sync.
    pub(crate) fn with_pool_topology(mut self, valid_pool_topology: Option<bool>) -> ReplicaItem {
        self.valid_pool_topology = valid_pool_topology;
        self
    }
    /// Get a reference to the node topology validity flag.
    pub(crate) fn valid_node_topology(&self) -> &Option<bool> {
        &self.valid_node_topology
    }
    /// Get a reference to the pool topology validity flag.
    pub(crate) fn valid_pool_topology(&self) -> &Option<bool> {
        &self.valid_pool_topology
    }
    /// Get a reference to the replica spec.
    pub(crate) fn spec(&self) -> &ReplicaSpec {
        &self.replica_spec
    }
    /// Get a reference to the replica state.
    pub(crate) fn state(&self) -> Option<&Replica> {
        self.replica_state.as_ref()
    }
    /// Get a reference to the child spec.
    #[allow(dead_code)]
    pub(crate) fn uri(&self) -> &Option<ChildUri> {
        &self.child_uri
    }
    /// Get a reference to the child state.
    pub(crate) fn child_state(&self) -> &Option<Child> {
        &self.child_state
    }
    /// Get a reference to the child spec.
    pub(crate) fn child_spec(&self) -> Option<&NexusChild> {
        self.child_spec.as_ref()
    }
    /// Get a reference to the child info.
    pub(crate) fn child_info(&self) -> Option<&ChildInfo> {
        self.child_info.as_ref()
    }
    /// Count of ag replicas on the replica's pool.
    pub(crate) fn ag_replicas_on_pool(&self) -> u64 {
        self.ag_replicas_on_pool.unwrap_or(u64::MIN)
    }
}

/// Individual nexus child (replicas) which can be used for nexus creation.
#[derive(Clone)]
pub(crate) struct ChildItem {
    replica_spec: ReplicaSpec,
    replica_state: Replica,
    pool_state: PoolWrapper,
    child_info: Option<ChildInfo>,
    rebuildable: Option<bool>,
}

// todo: keep original Debug and use valuable trait from tracing instead
impl std::fmt::Debug for ChildItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChildItem")
            .field("replica_spec", &self.replica_spec)
            .field("replica_state", &self.replica_state)
            .field("pool_state", self.pool_state.state())
            .field("child_info", &self.child_info.as_ref())
            .finish()
    }
}

/// If the nexus is shutdown uncleanly, only one child/replica may be used and it must be healthy
/// This is to avoid inconsistent data between the healthy replicas.
#[derive(Debug, Clone)]
pub(crate) enum HealthyChildItems {
    /// One with multiple healthy candidates.
    One(Option<NexusInfo>, Vec<ChildItem>),
    /// All the healthy replicas can be used.
    All(Option<NexusInfo>, Vec<ChildItem>),
}
impl HealthyChildItems {
    /// Check if there are no healthy children.
    pub(crate) fn is_empty(&self) -> bool {
        match self {
            HealthyChildItems::One(_, items) => items.is_empty(),
            HealthyChildItems::All(_, items) => items.is_empty(),
        }
    }
    /// Get a reference to the list of candidates.
    pub(crate) fn candidates(&self) -> &Vec<ChildItem> {
        match self {
            HealthyChildItems::One(_, items) => items,
            HealthyChildItems::All(_, items) => items,
        }
    }
    /// Get a reference to the list of candidates.
    pub(crate) fn nexus_info(&self) -> &Option<NexusInfo> {
        match self {
            HealthyChildItems::One(info, _) => info,
            HealthyChildItems::All(info, _) => info,
        }
    }
}

impl ChildItem {
    /// Create a new `Self` from the replica and the persistent child information.
    pub(crate) fn new(
        replica_spec: &ReplicaSpec,
        replica_state: &Replica,
        child_info: Option<&ChildInfo>,
        pool_state: &PoolWrapper,
        rebuildable: Option<bool>,
    ) -> Self {
        Self {
            replica_spec: replica_spec.clone(),
            replica_state: replica_state.clone(),
            child_info: child_info.cloned(),
            pool_state: pool_state.clone(),
            rebuildable,
        }
    }
    /// Get the replica spec.
    pub(crate) fn spec(&self) -> &ReplicaSpec {
        &self.replica_spec
    }
    /// Get the replica state.
    pub(crate) fn state(&self) -> &Replica {
        &self.replica_state
    }
    /// Get the persisted nexus child information.
    pub(crate) fn info(&self) -> &Option<ChildInfo> {
        &self.child_info
    }
    /// Get the pool wrapper.
    pub(crate) fn pool(&self) -> &PoolWrapper {
        &self.pool_state
    }
    /// Get a reference to the node id.
    pub(crate) fn node(&self) -> &NodeId {
        &self.pool_state.node
    }
    /// Check if we can rebuild this child.
    pub(crate) fn rebuildable(&self) -> &Option<bool> {
        &self.rebuildable
    }
}

/// Individual Node candidate which is a wrapper over nodewrapper used for filtering.
#[derive(Clone, Debug)]
pub(crate) struct NodeItem {
    pub(crate) node_wrapper: NodeWrapper,
    ag_nexus_count: Option<u64>,
    ag_preferred: bool,
}

impl NodeItem {
    /// Create a new node item with given `node_wrapper`.
    pub(crate) fn new(
        node_wrapper: NodeWrapper,
        ag_nexus_count: Option<u64>,
        ag_preferred: bool,
    ) -> Self {
        Self {
            node_wrapper,
            ag_nexus_count,
            ag_preferred,
        }
    }
    /// Get the internal `node_wrapper` from `NodeItem`.
    pub(crate) fn node_wrapper(&self) -> &NodeWrapper {
        &self.node_wrapper
    }
    /// Convert into internal `node_wrapper` from `NodeItem`.
    pub(crate) fn into_node_wrapper(self) -> NodeWrapper {
        self.node_wrapper
    }
    /// The number of nexuses present on the node.
    pub(crate) fn ag_nexus_count(&self) -> u64 {
        self.ag_nexus_count.unwrap_or(u64::MIN)
    }
    /// If a node is specified for target placement, mark it as preferred.
    pub(crate) fn ag_preferred(&self) -> bool {
        self.ag_preferred
    }
}
