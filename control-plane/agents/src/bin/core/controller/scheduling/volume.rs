use crate::controller::{
    registry::Registry,
    resources::ResourceMutex,
    scheduling::{
        affinity_group::{get_pool_ag_replica_count, get_restricted_nodes},
        pool::replica_rebuildable,
        resources::{ChildItem, PoolItem, PoolItemLister, ReplicaItem},
        volume_policy::{node::NodeFilters, SimplePolicy, ThickPolicy},
        AddReplicaFilters, AddReplicaSorters, ChildSorters, ResourceData, ResourceFilter,
    },
    wrapper::PoolWrapper,
};
use agents::errors::SvcError;
use stor_port::types::v0::{
    store::{
        nexus::NexusSpec, nexus_persistence::NexusInfo, snapshots::replica::ReplicaSnapshot,
        volume::VolumeSpec,
    },
    transport::{NodeId, PoolId, VolumeState},
};

use std::{collections::HashMap, ops::Deref};

/// Move replica to another pool.
#[derive(Default, Clone)]
pub(crate) struct MoveReplica {
    /// Current replica node.
    node: NodeId,
    /// Current replica pool.
    pool: PoolId,
}
impl MoveReplica {
    /// Return new `Self` from its node and pool.
    pub(crate) fn new(node: &NodeId, pool: &PoolId) -> Self {
        Self {
            node: node.clone(),
            pool: pool.clone(),
        }
    }
    /// Get the replica node.
    pub(crate) fn node(&self) -> &NodeId {
        &self.node
    }
    /// Get the replica pool.
    pub(crate) fn pool(&self) -> &PoolId {
        &self.pool
    }
}

/// Select suitable pools for volume replica creation or replacement/move.
#[derive(Clone)]
pub(crate) struct GetSuitablePools {
    spec: VolumeSpec,
    move_repl: Option<MoveReplica>,
}

impl GetSuitablePools {
    /// Return a new `Self` given the volume and optional replica move params.
    pub(crate) fn new(spec: &VolumeSpec, move_repl: Option<MoveReplica>) -> Self {
        Self {
            spec: spec.clone(),
            move_repl,
        }
    }
}

/// The context to select suitable pools for volume replica creation.
#[derive(Clone)]
pub(crate) struct GetSuitablePoolsContext {
    registry: Registry,
    spec: VolumeSpec,
    allocated_bytes: Option<u64>,
    move_repl: Option<MoveReplica>,
    snap_repl: bool,
    ag_restricted_nodes: Option<Vec<NodeId>>,
}
impl GetSuitablePoolsContext {
    /// Get the registry.
    pub(crate) fn registry(&self) -> &Registry {
        &self.registry
    }
    /// Get the currently allocated bytes (per replica).
    pub(crate) fn allocated_bytes(&self) -> &Option<u64> {
        &self.allocated_bytes
    }
    /// Get the optional replica move request.
    pub(crate) fn move_repl(&self) -> Option<&MoveReplica> {
        self.move_repl.as_ref()
    }
    /// Check if this is a replica snapshot request.
    pub(crate) fn snap_repl(&self) -> bool {
        self.snap_repl
    }
    pub(crate) fn ag_restricted_nodes(&self) -> &Option<Vec<NodeId>> {
        &self.ag_restricted_nodes
    }
    pub fn as_thin(&self) -> bool {
        self.spec.as_thin() || self.snap_repl()
    }
    /// Helper util for overcommit checks.
    pub(crate) fn overcommit(&self, allowed_commit_percent: u64, pool: &PoolWrapper) -> bool {
        let max_cap_allowed = allowed_commit_percent * pool.capacity;
        (self.size + pool.commitment()) * 100 < max_cap_allowed
    }
}

impl Deref for GetSuitablePoolsContext {
    type Target = VolumeSpec;

    fn deref(&self) -> &Self::Target {
        &self.spec
    }
}
impl Deref for GetSuitablePools {
    type Target = VolumeSpec;

    fn deref(&self) -> &Self::Target {
        &self.spec
    }
}

/// Add replicas to a volume.
/// Selects the best pool candidates to create lvol replicas on.
#[derive(Clone)]
pub(crate) struct AddVolumeReplica {
    data: ResourceData<GetSuitablePoolsContext, PoolItem>,
}

impl AddVolumeReplica {
    // Return max allocated bytes from volume replicas.
    // May return None if the replica nodes are offline.
    async fn allocated_bytes(registry: &Registry, volume: &VolumeSpec) -> Option<u64> {
        let replicas = registry.specs().volume_replicas(&volume.uuid);
        if replicas.is_empty() {
            return Some(0);
        }

        let mut used_bytes = Vec::with_capacity(replicas.len());
        for spec in replicas {
            if let Ok(state) = registry.replica(spec.uuid()).await {
                if let Some(space) = state.space {
                    used_bytes.push(space.allocated_distinct());
                }
            }
        }
        used_bytes.iter().max().cloned()
    }
    async fn builder(request: GetSuitablePools, registry: &Registry) -> Self {
        let volume_spec = request.spec;

        let allocated_bytes = Self::allocated_bytes(registry, &volume_spec).await;

        let mut ag_restricted_nodes: Option<Vec<NodeId>> = None;
        let mut pool_ag_replica_count_map: Option<HashMap<PoolId, u64>> = None;

        if let Some(affinity_group) = &volume_spec.affinity_group {
            if let Ok(affinity_group_spec) =
                registry.specs().affinity_group_spec(affinity_group.id())
            {
                ag_restricted_nodes = Some(get_restricted_nodes(
                    &volume_spec,
                    &affinity_group_spec,
                    registry,
                ));
                pool_ag_replica_count_map =
                    Some(get_pool_ag_replica_count(&affinity_group_spec, registry).await);
            }
        }

        Self {
            data: ResourceData::new(
                GetSuitablePoolsContext {
                    registry: registry.clone(),
                    spec: volume_spec,
                    allocated_bytes,
                    move_repl: request.move_repl,
                    snap_repl: false,
                    ag_restricted_nodes,
                },
                PoolItemLister::list(registry, &pool_ag_replica_count_map).await,
            ),
        }
    }
    fn with_default_policy(self) -> Self {
        match self.data.context.as_thin() {
            true => self.with_simple_policy(),
            false => self.with_thick_policy(),
        }
    }
    fn with_thick_policy(self) -> Self {
        self.policy(ThickPolicy::new())
    }
    fn with_simple_policy(self) -> Self {
        let simple = SimplePolicy::new(&self.data.context().registry);
        self.policy(simple)
    }
    /// Default rules for pool selection when creating replicas for a volume.
    pub(crate) async fn builder_with_defaults(
        request: GetSuitablePools,
        registry: &Registry,
    ) -> Self {
        Self::builder(request, registry).await.with_default_policy()
    }
}

#[async_trait::async_trait(?Send)]
impl ResourceFilter for AddVolumeReplica {
    type Request = GetSuitablePoolsContext;
    type Item = PoolItem;

    fn data(&mut self) -> &mut ResourceData<Self::Request, Self::Item> {
        &mut self.data
    }

    fn collect(self) -> Vec<Self::Item> {
        self.data.list
    }
}

/// Decrease a volume's replicas when it exceeds the required count.
#[derive(Clone)]
pub(crate) struct DecreaseVolumeReplica {
    data: ResourceData<GetChildForRemovalContext, ReplicaItem>,
}

/// Request to decrease volume replicas.
/// Specifies the volume spec, state and whether to only remove currently unused replicas.
#[derive(Clone)]
pub(crate) struct GetChildForRemoval {
    spec: VolumeSpec,
    state: VolumeState,
    /// Used when we have more replicas than we need, so we can be picky and try to remove
    /// unused replicas first (replicas which are not attached to a nexus).
    unused_only: bool,
}

impl GetChildForRemoval {
    /// Return a new `Self` from the provided parameters.
    pub(crate) fn new(spec: &VolumeSpec, state: &VolumeState, unused_only: bool) -> Self {
        Self {
            spec: spec.clone(),
            state: state.clone(),
            unused_only,
        }
    }
}

/// Used to filter nexus children in order to choose the best candidates for removal
/// when the volume's replica count is being reduced.
#[derive(Clone)]
pub(crate) struct GetChildForRemovalContext {
    registry: Registry,
    spec: VolumeSpec,
    state: VolumeState,
    nexus_info: Option<NexusInfo>,
    unused_only: bool,
}
impl std::fmt::Debug for GetChildForRemovalContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GetChildForRemovalContext")
            .field("spec", &self.spec)
            .field("state", &self.state)
            .field("nexus_info", &self.nexus_info)
            .field("unused_only", &self.unused_only)
            .finish()
    }
}

impl GetChildForRemovalContext {
    async fn new(registry: &Registry, request: &GetChildForRemoval) -> Result<Self, SvcError> {
        let nexus_info = registry
            .nexus_info(
                Some(&request.spec.uuid),
                request.spec.health_info_id(),
                true,
            )
            .await?;

        Ok(GetChildForRemovalContext {
            registry: registry.clone(),
            spec: request.spec.clone(),
            state: request.state.clone(),
            nexus_info,
            unused_only: request.unused_only,
        })
    }

    async fn list(&self, pool_ag_rep: &Option<HashMap<PoolId, u64>>) -> Vec<ReplicaItem> {
        let replicas = self.registry.specs().volume_replicas(&self.spec.uuid);
        let nexus = self.registry.specs().volume_target_nexus_rsc(&self.spec);
        let replicas = replicas.iter().map(|r| r.lock().clone());

        let replica_states = self.registry.replicas().await;
        replicas
            .map(|replica_spec| {
                let ag_rep_count = pool_ag_rep
                    .as_ref()
                    .and_then(|map| map.get(replica_spec.pool_name()).cloned());
                ReplicaItem::new(
                    replica_spec.clone(),
                    replica_states.iter().find(|r| r.uuid == replica_spec.uuid),
                    replica_states
                        .iter()
                        .find(|replica_state| replica_state.uuid == replica_spec.uuid)
                        .and_then(|replica_state| {
                            nexus.as_ref().and_then(|nexus_spec| {
                                nexus_spec
                                    .lock()
                                    .children
                                    .iter()
                                    .find(|child| child.uri() == replica_state.uri)
                                    .map(|child| child.uri())
                            })
                        }),
                    replica_states
                        .iter()
                        .find(|replica_state| replica_state.uuid == replica_spec.uuid)
                        .and_then(|replica_state| {
                            self.state.target.as_ref().and_then(|nexus_state| {
                                nexus_state
                                    .children
                                    .iter()
                                    .find(|child| child.uri.as_str() == replica_state.uri)
                                    .cloned()
                            })
                        }),
                    nexus.as_ref().and_then(|nexus_spec| {
                        nexus_spec
                            .lock()
                            .children
                            .iter()
                            .find(|child| {
                                child.as_replica().map(|uri| uri.uuid().clone())
                                    == Some(replica_spec.uuid.clone())
                            })
                            .cloned()
                    }),
                    self.nexus_info.as_ref().and_then(|nexus_info| {
                        nexus_info
                            .children
                            .iter()
                            .find(|child| child.uuid.as_str() == replica_spec.uuid.as_str())
                            .cloned()
                    }),
                    ag_rep_count,
                )
                .with_node_topology({
                    Some(NodeFilters::topology_replica(
                        &self.spec,
                        &self.registry,
                        replica_spec.pool_name(),
                    ))
                })
                .with_pool_topology({
                    use crate::controller::scheduling::volume_policy::pool::PoolBaseFilters;
                    Some(PoolBaseFilters::topology_(
                        &self.spec,
                        &self.registry,
                        replica_spec.pool_name(),
                    ))
                })
            })
            .collect::<Vec<_>>()
    }
}

impl DecreaseVolumeReplica {
    async fn builder(request: &GetChildForRemoval, registry: &Registry) -> Result<Self, SvcError> {
        let mut pool_ag_replica_count_map: Option<HashMap<PoolId, u64>> = None;
        if let Some(affinity_group) = &request.spec.affinity_group {
            let affinity_group_spec = registry.specs().affinity_group_spec(affinity_group.id())?;
            pool_ag_replica_count_map =
                Some(get_pool_ag_replica_count(&affinity_group_spec, registry).await);
        }

        let context = GetChildForRemovalContext::new(registry, request).await?;
        let list = context.list(&pool_ag_replica_count_map).await;
        Ok(Self {
            data: ResourceData::new(context, list),
        })
    }
    /// Create new `Self` from the given arguments with a default list of sorting rules.
    pub(crate) async fn builder_with_defaults(
        request: &GetChildForRemoval,
        registry: &Registry,
    ) -> Result<Self, SvcError> {
        Ok(Self::builder(request, registry)
            .await?
            .sort(ChildSorters::sort))
    }
    /// Get the `ReplicaRemovalCandidates` for this request, which splits the candidates into
    /// healthy and unhealthy candidates.
    pub(crate) fn candidates(self) -> ReplicaRemovalCandidates {
        ReplicaRemovalCandidates::new(self.data.context, self.data.list)
    }
}

/// Replica Removal Candidates with explicit partitioning between the healthy and unhealthy replicas
/// This way we can make sure we do not unintentionally remove "too many" healthy candidates and
/// risk making the volume degraded, or worst faulted.
#[derive(Debug)]
pub(crate) struct ReplicaRemovalCandidates {
    context: GetChildForRemovalContext,
    healthy: Vec<ReplicaItem>,
    unhealthy: Vec<ReplicaItem>,
}

impl ReplicaRemovalCandidates {
    /// Get the next healthy replica removal candidate.
    fn next_healthy(&mut self) -> Option<ReplicaItem> {
        let replica_count = self.context.spec.desired_num_replicas();
        let healthy_online = self.healthy.iter().filter(|replica| match replica.state() {
            None => false,
            Some(state) => state.online(),
        });
        // removing too many healthy_online replicas could compromise the volume's redundancy
        if healthy_online.into_iter().count() > replica_count as usize {
            match self.healthy.pop() {
                Some(replica) if self.context.unused_only & replica.child_spec().is_none() => {
                    Some(replica)
                }
                replica => replica,
            }
        } else {
            None
        }
    }
    /// Get the next unhealthy candidates (any is a good fit).
    fn next_unhealthy(&mut self) -> Option<ReplicaItem> {
        self.unhealthy.pop()
    }
    /// Get the next removal candidate.
    /// Unhealthy replicas are removed before healthy replicas.
    pub(crate) fn next(&mut self) -> Option<ReplicaItem> {
        self.next_unhealthy().or_else(|| self.next_healthy())
    }

    fn new(context: GetChildForRemovalContext, items: Vec<ReplicaItem>) -> Self {
        let has_info = context.nexus_info.is_some();
        let is_healthy = |item: &&ReplicaItem| -> bool {
            match item.child_info() {
                Some(info) => info.healthy,
                None => !has_info,
            }
        };
        let is_not_healthy = |item: &&ReplicaItem| -> bool { !is_healthy(item) };
        Self {
            context,
            // replicas were sorted with the least preferred replica at the front
            // since we're going to "pop" them here, we now need to move the least preferred to the
            // back and that's why we need the "rev"
            healthy: items.iter().filter(is_healthy).rev().cloned().collect(),
            unhealthy: items.iter().filter(is_not_healthy).rev().cloned().collect(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl ResourceFilter for DecreaseVolumeReplica {
    type Request = GetChildForRemovalContext;
    type Item = ReplicaItem;

    fn data(&mut self) -> &mut ResourceData<Self::Request, Self::Item> {
        &mut self.data
    }

    fn collect(self) -> Vec<Self::Item> {
        self.data.list
    }
}

/// `VolumeReplicasForNexusCtx` context used by the filter functions for `AddVolumeNexusReplicas`
/// which is used to add replicas to a volume nexus.
#[derive(Clone)]
pub(crate) struct VolumeReplicasForNexusCtx {
    registry: Registry,
    vol_spec: VolumeSpec,
    nexus_spec: NexusSpec,
    nexus_info: Option<NexusInfo>,
    shutdown_failed_nexuses: Vec<ResourceMutex<NexusSpec>>,
    allocated_bytes: u64,
}

impl VolumeReplicasForNexusCtx {
    /// Get the volume spec.
    pub(crate) fn vol_spec(&self) -> &VolumeSpec {
        &self.vol_spec
    }
    /// Get the nexus spec.
    pub(crate) fn nexus_spec(&self) -> &NexusSpec {
        &self.nexus_spec
    }
    /// Get the current nexus persistent information.
    #[allow(dead_code)]
    pub(crate) fn nexus_info(&self) -> &Option<NexusInfo> {
        &self.nexus_info
    }
    /// Get the registry.
    #[allow(dead_code)]
    pub(crate) fn registry(&self) -> &Registry {
        &self.registry
    }
    /// Get the list of nexuses associated to the volume, which failed shutdown.
    pub(crate) fn shutdown_failed_nexuses(&self) -> &Vec<ResourceMutex<NexusSpec>> {
        &self.shutdown_failed_nexuses
    }
}

impl VolumeReplicasForNexusCtx {
    async fn new(
        registry: &Registry,
        vol_spec: &VolumeSpec,
        nx_spec: &NexusSpec,
    ) -> Result<Self, SvcError> {
        let nexus_info = registry
            .nexus_info(Some(&vol_spec.uuid), Some(&nx_spec.uuid), true)
            .await?;

        let shutdown_pending_nexuses = registry
            .specs()
            .volume_failed_shutdown_nexuses(&vol_spec.uuid)
            .await;

        let mut allocated_bytes = 0;
        for child in nexus_info.as_ref().map(|n| &n.children).unwrap_or(&vec![]) {
            if let Ok(replica) = registry.replica(&child.uuid).await {
                let bytes = replica.space.map(|s| s.allocated_bytes).unwrap_or_default();
                if bytes > allocated_bytes {
                    allocated_bytes = bytes;
                }
            }
        }

        Ok(Self {
            registry: registry.clone(),
            vol_spec: vol_spec.clone(),
            nexus_spec: nx_spec.clone(),
            nexus_info,
            shutdown_failed_nexuses: shutdown_pending_nexuses,
            allocated_bytes,
        })
    }
    async fn list(&self) -> Vec<ChildItem> {
        // find all replica states
        let state_replicas = self.registry.replicas().await;
        // find all replica specs which are not yet part of the nexus
        let spec_replicas = self
            .registry
            .specs()
            .volume_replicas(&self.vol_spec.uuid)
            .into_iter()
            .filter(|r| !self.nexus_spec.contains_replica(&r.lock().uuid));
        let pool_wrappers = self.registry.pool_wrappers().await;

        spec_replicas
            .filter_map(|replica_spec| {
                let replica_spec = replica_spec.lock().clone();
                let replica_state = state_replicas
                    .iter()
                    .find(|state| state.uuid == replica_spec.uuid);
                let child_info = self.nexus_info.as_ref().and_then(|n| {
                    n.children.iter().find(|c| {
                        if let Some(replica_state) = replica_state {
                            replica_state.uuid == c.uuid
                        } else {
                            false
                        }
                    })
                });

                let pool_id = replica_spec.pool.pool_name().clone();
                pool_wrappers
                    .iter()
                    .find(|p| p.id == pool_id)
                    .and_then(|pool| {
                        replica_state.map(|replica_state| {
                            let rebuild = Some(replica_rebuildable(
                                pool,
                                self.allocated_bytes,
                                replica_state,
                            ));

                            ChildItem::new(&replica_spec, replica_state, child_info, pool, rebuild)
                        })
                    })
            })
            .collect()
    }
}

/// Retrieve a list of healthy replicas to add to a volume nexus.
#[derive(Clone)]
pub(crate) struct AddVolumeNexusReplicas {
    data: ResourceData<VolumeReplicasForNexusCtx, ChildItem>,
}

impl AddVolumeNexusReplicas {
    async fn builder(
        vol_spec: &VolumeSpec,
        nx_spec: &NexusSpec,
        registry: &Registry,
    ) -> Result<Self, SvcError> {
        let context = VolumeReplicasForNexusCtx::new(registry, vol_spec, nx_spec).await?;
        let list = context.list().await;
        Ok(Self {
            data: ResourceData::new(context, list),
        })
    }

    /// Builder used to retrieve a list of healthy replicas to add to a volume nexus.
    /// The list follows a set of filters for replicas according to the following
    /// criteria (any order):
    /// 1. replicas which are not part of the given nexus already
    /// 2. use only replicas which report the status of online by their state
    /// 3. use only replicas which are large enough for the volume
    /// Sorted by:
    /// 1. nexus local replicas
    /// 2. replicas which have never been marked as faulted by io-engine
    /// 3. replicas from pools with more free space.
    pub(crate) async fn builder_with_defaults(
        vol_spec: &VolumeSpec,
        nx_spec: &NexusSpec,
        registry: &Registry,
    ) -> Result<Self, SvcError> {
        Ok(Self::builder(vol_spec, nx_spec, registry)
            .await?
            .filter(AddReplicaFilters::online)
            .filter(AddReplicaFilters::size)
            .filter(AddReplicaFilters::reservable)
            .sort_ctx(AddReplicaSorters::sort))
    }
}

#[async_trait::async_trait(?Send)]
impl ResourceFilter for AddVolumeNexusReplicas {
    type Request = VolumeReplicasForNexusCtx;
    type Item = ChildItem;

    fn data(&mut self) -> &mut ResourceData<Self::Request, Self::Item> {
        &mut self.data
    }

    fn collect(self) -> Vec<Self::Item> {
        self.data.list
    }
}

/// Snapshot a volume replica.
#[derive(Clone)]
pub(crate) struct SnapshotVolumeReplica {
    data: ResourceData<GetSuitablePoolsContext, PoolItem>,
}

impl SnapshotVolumeReplica {
    async fn builder(registry: &Registry, volume: &VolumeSpec, items: &[ChildItem]) -> Self {
        let allocated_bytes = AddVolumeReplica::allocated_bytes(registry, volume).await;

        Self {
            data: ResourceData::new(
                GetSuitablePoolsContext {
                    registry: registry.clone(),
                    spec: volume.clone(),
                    allocated_bytes,
                    move_repl: None,
                    snap_repl: true,
                    ag_restricted_nodes: None,
                },
                PoolItemLister::list_for_snaps(registry, items).await,
            ),
        }
    }
    fn with_default_policy(self) -> Self {
        match self.data.context.as_thin() {
            true => self.with_simple_policy(),
            false => self.with_thick_policy(),
        }
    }
    fn with_thick_policy(self) -> Self {
        self.policy(ThickPolicy::new())
    }
    fn with_simple_policy(self) -> Self {
        let simple = SimplePolicy::new(&self.data.context().registry);
        self.policy(simple)
    }
    /// Default rules for replica snapshot selection when snapshot replicas for a volume.
    pub(crate) async fn builder_with_defaults(
        registry: &Registry,
        volume: &VolumeSpec,
        items: &[ChildItem],
    ) -> Self {
        Self::builder(registry, volume, items)
            .await
            .with_default_policy()
    }
}

#[async_trait::async_trait(?Send)]
impl ResourceFilter for SnapshotVolumeReplica {
    type Request = GetSuitablePoolsContext;
    type Item = PoolItem;

    fn data(&mut self) -> &mut ResourceData<Self::Request, Self::Item> {
        &mut self.data
    }

    fn collect(self) -> Vec<Self::Item> {
        self.data.list
    }
}

/// Clone volume snapshot replicas.
#[derive(Clone)]
pub(crate) struct CloneVolumeSnapshot {
    data: ResourceData<GetSuitablePoolsContext, PoolItem>,
}

impl CloneVolumeSnapshot {
    async fn builder(
        registry: &Registry,
        spec: &VolumeSpec,
        snapshots: &[ReplicaSnapshot],
    ) -> Self {
        Self {
            data: ResourceData::new(
                GetSuitablePoolsContext {
                    registry: registry.clone(),
                    spec: spec.clone(),
                    allocated_bytes: Some(0),
                    move_repl: None,
                    snap_repl: false,
                    ag_restricted_nodes: None,
                },
                PoolItemLister::list_for_clones(registry, snapshots).await,
            ),
        }
    }
    fn with_simple_policy(self) -> Self {
        let simple = SimplePolicy::new(&self.data.context().registry);
        self.policy(simple)
    }
    /// Default rules for replica snapshot selection when snapshot replicas for a volume.
    pub(crate) async fn builder_with_defaults(
        registry: &Registry,
        spec: &VolumeSpec,
        snapshots: &[ReplicaSnapshot],
    ) -> Self {
        Self::builder(registry, spec, snapshots)
            .await
            .with_simple_policy()
    }
}

#[async_trait::async_trait(?Send)]
impl ResourceFilter for CloneVolumeSnapshot {
    type Request = GetSuitablePoolsContext;
    type Item = PoolItem;

    fn data(&mut self) -> &mut ResourceData<Self::Request, Self::Item> {
        &mut self.data
    }

    fn collect(self) -> Vec<Self::Item> {
        self.data.list
    }
}

/// The context to check pool capacity for volume replica resize feasibility.
#[derive(Clone)]
pub(crate) struct ReplicaResizePoolsContext {
    registry: Registry,
    spec: VolumeSpec,
    allocated_bytes: Option<u64>,
    required_capacity: u64,
}

impl ReplicaResizePoolsContext {
    /// The additional capacity that we need.
    pub(crate) fn required_capacity(&self) -> u64 {
        self.required_capacity
    }

    /// Spec for the volume undergoing resize.
    pub(crate) fn spec(&self) -> &VolumeSpec {
        &self.spec
    }

    /// Get the currently allocated bytes (per replica).
    pub(crate) fn allocated_bytes(&self) -> &Option<u64> {
        &self.allocated_bytes
    }

    /// Helper util for overcommit checks.
    pub(crate) fn overcommit(&self, allowed_commit_percent: u64, pool: &PoolWrapper) -> bool {
        let max_cap_allowed = allowed_commit_percent * pool.capacity;
        (self.required_capacity + pool.commitment()) * 100 < max_cap_allowed
    }
}

/// Resize the replicas of a volume.
pub(crate) struct ResizeVolumeReplicas {
    data: ResourceData<ReplicaResizePoolsContext, ChildItem>,
}

impl ResizeVolumeReplicas {
    async fn builder(registry: &Registry, spec: &VolumeSpec, required_capacity: u64) -> Self {
        // Reuse the method from AddVolumeReplica even though name doesn't indicate the exact
        // purpose.
        let allocated_bytes = AddVolumeReplica::allocated_bytes(registry, spec).await;
        Self {
            data: ResourceData::new(
                ReplicaResizePoolsContext {
                    registry: registry.clone(),
                    spec: spec.clone(),
                    allocated_bytes,
                    required_capacity,
                },
                PoolItemLister::list_for_resize(registry, spec).await,
            ),
        }
    }

    fn with_default_policy(self) -> Self {
        match self.data.context.spec.as_thin() {
            true => self.with_simple_policy(),
            false => self.with_thick_policy(),
        }
    }
    fn with_thick_policy(self) -> Self {
        self.policy(ThickPolicy::new())
    }
    fn with_simple_policy(self) -> Self {
        let simple = SimplePolicy::new(&self.data.context().registry);
        self.policy(simple)
    }

    /// Default rules for replica filtering when resizing replicas for a volume.
    pub(crate) async fn builder_with_defaults(
        registry: &Registry,
        spec: &VolumeSpec,
        req_capacity: u64,
    ) -> Self {
        Self::builder(registry, spec, req_capacity)
            .await
            .with_default_policy()
    }
}

#[async_trait::async_trait(?Send)]
impl ResourceFilter for ResizeVolumeReplicas {
    type Request = ReplicaResizePoolsContext;
    type Item = ChildItem;

    fn data(&mut self) -> &mut ResourceData<Self::Request, Self::Item> {
        &mut self.data
    }

    fn collect(self) -> Vec<Self::Item> {
        self.data.list
    }
}
