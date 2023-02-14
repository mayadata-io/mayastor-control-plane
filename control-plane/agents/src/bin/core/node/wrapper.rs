use crate::{
    controller::{
        io_engine::{GrpcClient, GrpcClientLocked, GrpcContext, NexusApi, PoolApi, ReplicaApi},
        resources::ResourceUid,
        states::{ResourceStates, ResourceStatesLocked},
    },
    node::{service::NodeCommsTimeout, watchdog::Watchdog},
    pool::wrapper::PoolWrapper,
    NumRebuilds,
};

use agents::errors::SvcError;
use stor_port::{
    transport_api::{Message, MessageId, ResourceKind},
    types::v0::{
        store,
        store::{nexus::NexusState, replica::ReplicaState},
        transport::{
            AddNexusChild, ApiVersion, Child, CreateNexus, CreatePool, CreateReplica, DestroyNexus,
            DestroyPool, DestroyReplica, FaultNexusChild, MessageIdVs, Nexus, NexusId, NodeId,
            NodeState, NodeStatus, PoolId, PoolState, Register, RemoveNexusChild, Replica,
            ReplicaId, ReplicaName, ShareNexus, ShareReplica, ShutdownNexus, UnshareNexus,
            UnshareReplica, VolumeId,
        },
    },
};

use crate::controller::{
    io_engine::{NexusChildApi, NexusShareApi},
    states::Either,
};
use async_trait::async_trait;
use parking_lot::RwLock;
use std::{ops::DerefMut, sync::Arc};
use tracing::{debug, trace, warn};

type NodeResourceStates = (Vec<Replica>, Vec<PoolState>, Vec<Nexus>);

/// Default timeout for GET* gRPC requests (ex: GetPools, GetNexuses, etc..)
const GETS_TIMEOUT: MessageId = MessageId::v0(MessageIdVs::Default);

enum ResourceType {
    All(Vec<PoolState>, Vec<Replica>, Vec<Nexus>),
    Nexuses(Vec<Nexus>),
    Nexus(Either<Nexus, NexusId>),
    Pools(Vec<PoolState>),
    Pool(Either<PoolState, PoolId>),
    Replicas(Vec<Replica>),
    Replica(Either<Replica, ReplicaId>),
}

/// Wrapper over a `Node` plus a few useful methods/properties. Includes:
/// all pools and replicas from the node
/// a watchdog to keep track of the node's liveness
/// a lock to serialize mutating gRPC calls
/// The Node may still be considered online even when the watchdog times out if it still is
/// responding to gRPC liveness probes.
#[derive(Debug, Clone)]
pub(crate) struct NodeWrapper {
    /// The inner Node state.
    node_state: NodeState,
    /// The watchdog to track the node state.
    watchdog: Watchdog,
    /// Indicates whether the node has already missed its deadline and in such case we don't
    /// need to keep posting duplicate error events.
    missed_deadline: bool,
    /// The gRPC CRUD lock
    lock: Arc<tokio::sync::Mutex<()>>,
    /// The node communication timeouts.
    comms_timeouts: NodeCommsTimeout,
    /// The runtime state information.
    states: ResourceStatesLocked,
    /// The number of rebuilds in progress on the node.
    num_rebuilds: Arc<RwLock<NumRebuilds>>,
}

impl NodeWrapper {
    /// Create a new wrapper for a `Node` with a `deadline` for its watchdog.
    pub(crate) fn new(
        node: &NodeState,
        deadline: std::time::Duration,
        comms_timeouts: NodeCommsTimeout,
    ) -> Self {
        tracing::debug!("Creating new node {:?}", node);
        Self {
            node_state: node.clone(),
            watchdog: Watchdog::new(&node.id, deadline),
            missed_deadline: false,
            lock: Default::default(),
            comms_timeouts,
            states: ResourceStatesLocked::new(),
            num_rebuilds: Arc::new(RwLock::new(0)),
        }
    }

    /// Set the node state to the passed argument.
    fn set_state_inner(&mut self, mut node_state: NodeState, creation: bool) -> bool {
        // don't modify the status through the state.
        node_state.status = self.status();
        if self.node_state() != &node_state {
            // don't flag state changes as the first state is inferred so it may be incorrect.
            // Example: we don't know what api versions the node supports yet.
            if !creation {
                trace!(
                    node.id=%node_state.id,
                    "Node state changed from {:?} to {:?}",
                    self.node_state(),
                    node_state
                );
                if self.node_state().api_versions != node_state.api_versions {
                    warn!(
                        node.id=%node_state.id,
                        "Node grpc api versions changed from {:?} to {:?}",
                        self.node_state().api_versions,
                        node_state.api_versions
                    );
                }

                if self.node_state().instance_uuid().is_some()
                    && self.node_state().instance_uuid() != node_state.instance_uuid()
                {
                    warn!(
                        node.id=%node_state.id,
                        "Node restart detected: {:?} to {:?}",
                        self.node_state().instance_uuid(),
                        node_state.instance_uuid()
                    );
                }

                // todo: what to do if this ever happens?
                if self.node_state().node_nqn != node_state.node_nqn {
                    tracing::error!(
                        node.id=%node_state.id,
                        "Node hostnqn changed from {:?} to {:?}",
                        self.node_state().node_nqn,
                        node_state.node_nqn
                    );
                }
            }
            self.node_state = node_state;
            true
        } else {
            false
        }
    }

    /// Set the node state to the passed argument.
    /// No state changes are not logged during creation since we're still constructing the node.
    pub(crate) fn set_startup_creation(&mut self, node_state: NodeState) -> bool {
        self.set_state_inner(node_state, true)
    }
    /// Set the node state to the passed argument.
    pub(crate) fn set_state(&mut self, node_state: NodeState) -> bool {
        self.set_state_inner(node_state, false)
    }

    /// get the latest api version from the list of supported api versions by the dataplane.
    pub(crate) fn latest_api_version(&self) -> Option<ApiVersion> {
        match self.node_state.api_versions.clone() {
            None => None,
            Some(mut api_version) => {
                api_version.sort();
                // get the last element after sort, if it was an empty vec, then
                // return the latest version as V0
                Some(api_version.last().unwrap_or(&ApiVersion::V1).clone())
            }
        }
    }

    /// Get `GrpcClient` for this node.
    async fn grpc_client(&self) -> Result<GrpcClient, SvcError> {
        GrpcClient::new(&self.grpc_context()?).await
    }

    /// Get `GrpcClient` for this node, and specify the comms timeout.
    async fn grpc_client_timeout(&self, timeout: NodeCommsTimeout) -> Result<GrpcClient, SvcError> {
        GrpcClient::new(&self.grpc_context_timeout(timeout)?).await
    }

    /// Get `GrpcContext` for this node.
    /// It will be used to execute the `request` operation.
    pub(crate) fn grpc_context_ext(&self, request: MessageId) -> Result<GrpcContext, SvcError> {
        if let Some(api_version) = self.latest_api_version() {
            Ok(GrpcContext::new(
                self.lock.clone(),
                self.id(),
                self.node_state().grpc_endpoint,
                &self.comms_timeouts,
                Some(request),
                api_version,
            )?)
        } else {
            Err(SvcError::InvalidApiVersion { api_version: None })
        }
    }

    /// Get `GrpcContext` for this node using the specified timeout.
    pub(crate) fn grpc_context_timeout(
        &self,
        timeout: NodeCommsTimeout,
    ) -> Result<GrpcContext, SvcError> {
        if let Some(api_version) = self.latest_api_version() {
            Ok(GrpcContext::new(
                self.lock.clone(),
                self.id(),
                self.node_state().grpc_endpoint,
                &timeout,
                None,
                api_version,
            )?)
        } else {
            Err(SvcError::InvalidApiVersion { api_version: None })
        }
    }

    /// Get `GrpcContext` for this node.
    pub(super) fn grpc_context(&self) -> Result<GrpcContext, SvcError> {
        if let Some(api_version) = self.latest_api_version() {
            Ok(GrpcContext::new(
                self.lock.clone(),
                self.id(),
                self.node_state().grpc_endpoint,
                &self.comms_timeouts,
                None,
                api_version,
            )?)
        } else {
            Err(SvcError::InvalidApiVersion { api_version: None })
        }
    }

    /// Get the `NodeStateFetcher` to fetch information from the data-plane.
    pub(crate) fn fetcher(&self) -> NodeStateFetcher {
        NodeStateFetcher::new(self.node_state.clone())
    }

    /// Whether the watchdog deadline has expired.
    pub(crate) fn registration_expired(&self) -> bool {
        self.watchdog.timestamp().elapsed() > self.watchdog.deadline()
    }

    /// "Pet" the node to meet the node's watchdog timer deadline.
    pub(super) async fn pet(&mut self) {
        self.watchdog.pet().await.ok();
        if self.missed_deadline {
            tracing::info!(node.id=%self.id(), "The node had missed the heartbeat deadline but it's now re-registered itself");
        }
        self.missed_deadline = false;
    }

    /// Update the node liveness if the watchdog's registration expired.
    /// If the node is still responding to gRPC then consider it as online and reset the watchdog.
    pub(super) async fn update_liveness(&mut self) {
        if self.registration_expired() {
            if !self.missed_deadline {
                tracing::error!(
                    node.id = %self.id(),
                    deadline = ?self.watchdog.deadline(),
                    "A node missed its registration deadline",
                );
            }

            if self.is_online()
                && self.liveness_probe().await.is_ok()
                && self.watchdog.pet().await.is_ok()
            {
                if !self.missed_deadline {
                    tracing::warn!(node.id=%self.id(), "A node missed its heartbeat deadline but it's still responding to gRPC so we're considering it online");
                }
            } else {
                if self.missed_deadline {
                    tracing::error!(
                        node.id = %self.id(),
                        deadline = ?self.watchdog.deadline(),
                        "A node missed its registration deadline",
                    );
                }
                self.set_status(NodeStatus::Offline);
            }
            self.missed_deadline = true;
        }
    }

    /// Probe the node for liveness.
    pub(crate) async fn liveness_probe(&mut self) -> Result<Register, SvcError> {
        //use the connect timeout for liveness
        let timeouts = NodeCommsTimeout::new(
            self.comms_timeouts.connect(),
            self.comms_timeouts.connect(),
            true,
        );

        let client = self.grpc_client_timeout(timeouts).await?;
        client
            .liveness_probe()
            .await
            .map_err(|_| SvcError::NodeNotOnline {
                node: self.id().clone(),
            })
    }

    /// Probe the node for liveness with all known api versions, as on startup its not known
    /// which api version to reach.
    pub(super) async fn liveness_probe_all(&mut self) -> Result<Register, SvcError> {
        //use the connect timeout for liveness
        let timeouts = NodeCommsTimeout::new(
            self.comms_timeouts.connect(),
            self.comms_timeouts.connect(),
            true,
        );

        // Set the api version to latest and make a call
        self.node_state.api_versions = Some(vec![ApiVersion::V1]);
        let client = self.grpc_client_timeout(timeouts.clone()).await?;
        match client.liveness_probe().await {
            Ok(data) => return Ok(data),
            Err(SvcError::GrpcRequestError { source, .. })
                if source.code() == tonic::Code::Unimplemented =>
            {
                debug!(
                    node.id = %self.id(),
                    "V1 liveness not implemented, retrying with V0 liveness"
                )
            }
            Err(error) => {
                tracing::error!(
                    node.id = %self.id(),
                    ?error,
                    "V1 liveness probe failed"
                );
                return Err(SvcError::NodeNotOnline {
                    node: self.id().clone(),
                });
            }
        }

        // Set the api version to second latest and make a call
        self.node_state.api_versions = Some(vec![ApiVersion::V0]);
        let client = self.grpc_client_timeout(timeouts).await?;
        client.liveness_probe().await.map_err(|error| {
            tracing::error!(?error, node.id = %self.id(), "V0 liveness probe failed");
            SvcError::NodeNotOnline {
                node: self.id().clone(),
            }
        })
    }

    /// Set the node status and return the previous status.
    pub(super) fn set_status(&mut self, next: NodeStatus) -> NodeStatus {
        let previous = self.status();
        if previous != next {
            if next == NodeStatus::Online {
                tracing::info!(
                    node.id = %self.id(),
                    "Node changing from {} to {}",
                    previous.to_string(),
                    next.to_string(),
                );
            } else {
                tracing::warn!(
                    node.id = %self.id(),
                    "Node changing from {} to {}",
                    previous.to_string(),
                    next.to_string(),
                );
            }

            self.node_state.status = next;
            if self.node_state.status == NodeStatus::Unknown {
                self.watchdog_mut().disarm()
            }
        }
        // Clear the states, otherwise we could temporarily return pools/nexus as online, even
        // though we report the node otherwise.
        // We take the approach that no information is better than inconsistent information.
        if !self.is_online() {
            self.clear_states();
        }
        previous
    }

    /// Clear all states from the node.
    fn clear_states(&mut self) {
        self.resources_mut().clear_all();
    }

    /// Get the inner states.
    fn resources(&self) -> parking_lot::RwLockReadGuard<ResourceStates> {
        self.states.read()
    }

    /// Get the inner resource states.
    fn resources_mut(&self) -> parking_lot::RwLockWriteGuard<ResourceStates> {
        self.states.write()
    }

    /// Get a mutable reference to the node's watchdog.
    pub(crate) fn watchdog_mut(&mut self) -> &mut Watchdog {
        &mut self.watchdog
    }
    /// Get the inner node.
    pub(crate) fn node_state(&self) -> &NodeState {
        &self.node_state
    }
    /// Get the node `NodeId`.
    pub(crate) fn id(&self) -> &NodeId {
        self.node_state().id()
    }
    /// Get the node `NodeStatus`.
    pub(crate) fn status(&self) -> NodeStatus {
        self.node_state().status().clone()
    }

    /// Get the node grpc endpoint as string.
    pub(crate) fn endpoint_str(&self) -> String {
        self.node_state().grpc_endpoint.to_string()
    }
    /// Get all pools
    pub(crate) fn pools(&self) -> Vec<PoolState> {
        self.resources()
            .pool_states()
            .map(|p| p.lock().pool.clone())
            .collect()
    }
    /// Get all pool wrappers.
    pub(crate) fn pool_wrappers(&self) -> Vec<PoolWrapper> {
        let pools = self.resources().pool_states_cloned();
        let resources = self.resources();
        pools
            .into_iter()
            .map(|pool_state| {
                let replicas = resources
                    .replica_states()
                    .filter_map(|replica_state| {
                        let replica_state = replica_state.lock();
                        if &replica_state.replica.pool_id == pool_state.uid() {
                            Some(replica_state.replica.clone())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<Replica>>();
                PoolWrapper::new(pool_state.pool, replicas)
            })
            .collect()
    }
    /// Get all pool states.
    pub(crate) fn pool_states_cloned(&self) -> Vec<store::pool::PoolState> {
        self.resources().pool_states_cloned()
    }
    /// Get pool from `pool_id` or None.
    pub(crate) fn pool(&self, pool_id: &PoolId) -> Option<PoolState> {
        self.resources().pool_state(pool_id).map(|p| p.pool)
    }
    /// Get a PoolWrapper for the pool ID.
    pub(crate) fn pool_wrapper(&self, pool_id: &PoolId) -> Option<PoolWrapper> {
        match self.resources().pool_state(pool_id) {
            Some(pool_state) => {
                let resources = self.resources();
                let replicas = resources
                    .replica_states()
                    .filter_map(|r| {
                        let replica = r.lock();
                        if &replica.replica.pool_id == pool_state.uid() {
                            Some(replica.replica.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                Some(PoolWrapper::new(pool_state.pool, replicas))
            }
            None => None,
        }
    }
    /// Get all replicas.
    pub(crate) fn replicas(&self) -> Vec<Replica> {
        self.resources()
            .replica_states()
            .map(|r| r.lock().replica.clone())
            .collect()
    }
    /// Get all replica states.
    pub(crate) fn replica_states_cloned(&self) -> Vec<ReplicaState> {
        self.resources().replica_states_cloned()
    }
    /// Get all nexuses.
    fn nexuses(&self) -> Vec<Nexus> {
        self.resources()
            .nexus_states()
            .map(|nexus_state| nexus_state.lock().nexus.clone())
            .collect()
    }
    /// Get all nexus states.
    pub(crate) fn nexus_states_cloned(&self) -> Vec<NexusState> {
        self.resources().nexus_states_cloned()
    }
    /// How many nexuses in the node.
    pub(crate) fn nexus_count(&self) -> usize {
        self.resources().nexus_states().count()
    }
    /// Get nexus.
    fn nexus(&self, nexus_id: &NexusId) -> Option<Nexus> {
        self.resources().nexus_state(nexus_id).map(|s| s.nexus)
    }
    /// Get nexus for the given volume.
    fn volume_nexus(&self, volume_id: &VolumeId) -> Option<Nexus> {
        self.resources()
            .nexus_states()
            .find(|n| n.lock().nexus.name == volume_id.as_str())
            .map(|n| n.lock().nexus.clone())
    }
    /// Get replica from `replica_id`.
    pub(crate) fn replica(&self, replica_id: &ReplicaId) -> Option<Replica> {
        self.resources()
            .replica_state(replica_id)
            .map(|r| r.lock().replica.clone())
    }
    /// Is the node online.
    pub(crate) fn is_online(&self) -> bool {
        self.status() == NodeStatus::Online
    }

    /// Load the node by fetching information from io-engine.
    pub(crate) async fn load(&mut self, startup: bool) -> Result<(), SvcError> {
        tracing::info!(
            node.id = %self.id(),
            node.endpoint = self.endpoint_str(),
            startup,
            "Preloading node"
        );

        let mut client = self.grpc_client().await?;
        match self.fetcher().fetch_resources(&mut client).await {
            Ok((replicas, pools, nexuses)) => {
                let mut states = self.resources_mut();
                states.update(pools, replicas, nexuses);
                Ok(())
            }
            Err(error) => {
                self.set_status(NodeStatus::Unknown);
                tracing::error!(
                    node.id = %self.id(),
                    node.endpoint = %self.endpoint_str(),
                    %error,
                    "Failed to preload node"
                );
                Err(error)
            }
        }
    }

    /// Update the node by updating its state from the states fetched from io-engine.
    fn update(
        &mut self,
        setting_online: bool,
        fetch_result: Result<NodeResourceStates, SvcError>,
    ) -> Result<(), SvcError> {
        if self.is_online() || setting_online {
            tracing::trace!(
                node.id = %self.id(),
                node.endpoint = self.endpoint_str(),
                "Reloading node"
            );

            match fetch_result {
                Ok((replicas, pools, nexuses)) => {
                    self.update_resources(ResourceType::All(pools, replicas, nexuses));
                    if setting_online {
                        // we only set it as online after we've updated the resource states
                        // so an online node should be "up-to-date"
                        self.set_status(NodeStatus::Online);
                    }
                    Ok(())
                }
                Err(error) => {
                    self.set_status(NodeStatus::Unknown);
                    tracing::error!(
                        node.id = %self.id(),
                        ?error,
                        "Failed to reload node"
                    );
                    Err(error)
                }
            }
        } else {
            tracing::trace!(
                node.id = %self.id(),
                node.status = self.status().to_string(),
                "Skipping node reload since it's not online",
            );
            // should already be cleared
            self.clear_states();
            Err(SvcError::NodeNotOnline {
                node: self.id().to_owned(),
            })
        }
    }

    /// Update all the nexus states.
    async fn update_nexus_states(
        node: &Arc<tokio::sync::RwLock<NodeWrapper>>,
        client: &mut GrpcClient,
    ) -> Result<(), SvcError> {
        let fetcher = node.read().await.fetcher();
        let nexuses = fetcher.fetch_nexuses(client).await?;
        node.write()
            .await
            .update_resources(ResourceType::Nexuses(nexuses));
        Ok(())
    }

    /// Update the given nexus state.
    async fn update_nexus_state(
        node: &Arc<tokio::sync::RwLock<NodeWrapper>>,
        state: Either<Nexus, NexusId>,
    ) {
        node.write()
            .await
            .update_resources(ResourceType::Nexus(state));
    }

    /// Update all the pool states.
    async fn update_pool_states(
        node: &Arc<tokio::sync::RwLock<NodeWrapper>>,
        client: &mut GrpcClient,
    ) -> Result<(), SvcError> {
        let fetcher = node.read().await.fetcher();
        let pools = fetcher.fetch_pools(client).await?;
        node.write()
            .await
            .update_resources(ResourceType::Pools(pools));
        Ok(())
    }

    /// Update the given pool state.
    async fn update_pool_state(
        node: &Arc<tokio::sync::RwLock<NodeWrapper>>,
        state: Either<PoolState, PoolId>,
    ) {
        node.write()
            .await
            .update_resources(ResourceType::Pool(state));
    }

    /// Update all the replica states.
    async fn update_replica_states(
        node: &Arc<tokio::sync::RwLock<NodeWrapper>>,
        client: &mut GrpcClient,
    ) -> Result<(), SvcError> {
        let fetcher = node.read().await.fetcher();
        let replicas = fetcher.fetch_replicas(client).await?;
        node.write()
            .await
            .update_resources(ResourceType::Replicas(replicas));
        Ok(())
    }

    /// Update the given replica state.
    async fn update_replica_state(
        node: &Arc<tokio::sync::RwLock<NodeWrapper>>,
        state: Either<Replica, ReplicaId>,
    ) {
        node.write()
            .await
            .update_resources(ResourceType::Replica(state));
    }

    /// Update the states of the specified resource type.
    /// Whenever the nexus states are updated the number of rebuilds must be updated.
    fn update_resources(&self, resource_type: ResourceType) {
        match resource_type {
            ResourceType::All(pools, replicas, nexuses) => {
                self.resources_mut().update(pools, replicas, nexuses);
                self.update_num_rebuilds();
            }
            ResourceType::Nexuses(nexuses) => {
                self.resources_mut().update_nexuses(nexuses);
                self.update_num_rebuilds();
            }
            ResourceType::Nexus(nexus) => self.resources_mut().update_nexus(nexus),
            ResourceType::Pools(pools) => {
                self.resources_mut().update_pools(pools);
            }
            ResourceType::Pool(pool) => self.resources_mut().update_pool(pool),
            ResourceType::Replicas(replicas) => {
                self.resources_mut().update_replicas(replicas);
            }
            ResourceType::Replica(replica) => self.resources_mut().update_replica(replica),
        }
    }

    /// Update the number of rebuilds in progress on this node.
    fn update_num_rebuilds(&self) {
        let num_rebuilds = match self.latest_api_version().unwrap_or(ApiVersion::V0) {
            ApiVersion::V0 => {
                // Note: Each nexus returns the total number of rebuilds on the node, **NOT** the
                // number of rebuilds per nexus. Therefore retrieve the number of
                // rebuilds from one nexus only. If there are no nexuses, the number
                // of rebuilds is reset.
                self.resources()
                    .nexus_states()
                    .take(1)
                    .fold(0, |acc, n| acc + n.lock().nexus.rebuilds)
            }
            _ => self
                .resources()
                .nexus_states()
                .fold(0, |acc, n| acc + n.lock().nexus.rebuilds),
        };
        let mut rebuilds = self.num_rebuilds.write();
        *rebuilds = num_rebuilds;
    }

    /// Return the number of rebuilds in progress on this node.
    pub(crate) fn num_rebuilds(&self) -> NumRebuilds {
        *self.num_rebuilds.read()
    }
}

/// Fetches node state from the dataplane.
#[derive(Debug, Clone)]
pub(crate) struct NodeStateFetcher {
    /// inner Node state
    node_state: NodeState,
}

impl NodeStateFetcher {
    /// Get new `Self` from the `NodeState`.
    fn new(node_state: NodeState) -> Self {
        Self { node_state }
    }
    fn id(&self) -> &NodeId {
        self.node_state.id()
    }
    /// Fetch the various resources from the Io Engine.
    async fn fetch_resources(
        &self,
        client: &mut GrpcClient,
    ) -> Result<NodeResourceStates, SvcError> {
        let replicas = self.fetch_replicas(client).await?;
        let pools = self.fetch_pools(client).await?;
        let nexuses = self.fetch_nexuses(client).await?;
        Ok((replicas, pools, nexuses))
    }

    /// Fetch all replicas from this node via gRPC
    pub(crate) async fn fetch_replicas(
        &self,
        client: &mut GrpcClient,
    ) -> Result<Vec<Replica>, SvcError> {
        client.list_replicas(self.id()).await
    }
    /// Fetch all pools from this node via gRPC
    pub(crate) async fn fetch_pools(
        &self,
        client: &mut GrpcClient,
    ) -> Result<Vec<PoolState>, SvcError> {
        client.list_pools(self.id()).await
    }
    /// Fetch all nexuses from the node via gRPC
    pub(crate) async fn fetch_nexuses(
        &self,
        client: &mut GrpcClient,
    ) -> Result<Vec<Nexus>, SvcError> {
        client.list_nexuses(self.id()).await
    }
}

/// CRUD Operations on a locked io-engine `NodeWrapper` such as:
/// pools, replicas, nexuses and their children.
#[async_trait]
pub(crate) trait ClientOps {
    /// Get the grpc lock and client pair to execute the provided `request`
    /// NOTE: Only available when the node status is online.
    async fn grpc_client_locked(&self, request: MessageId) -> Result<GrpcClientLocked, SvcError>;
}

/// Internal Operations on a io-engine locked `NodeWrapper` for the implementor
/// of the `ClientOps` trait and the `Registry` itself.
#[async_trait]
pub(crate) trait InternalOps {
    /// Get the inner lock, typically used to sync mutating gRPC operations.
    async fn grpc_lock(&self) -> Arc<tokio::sync::Mutex<()>>;
    /// Update the node's nexus state information.
    async fn update_nexus_states(&self, ctx: &mut GrpcClient) -> Result<(), SvcError>;
    /// Update a node's nexus state information.
    async fn update_nexus_state(&self, state: Either<Nexus, NexusId>);
    /// Update the node's pool state information.
    async fn update_pool_states(&self, ctx: &mut GrpcClient) -> Result<(), SvcError>;
    /// Update a node's pool state information.
    async fn update_pool_state(&self, state: Either<PoolState, PoolId>);
    /// Update the node's replica state information.
    async fn update_replica_states(&self, ctx: &mut GrpcClient) -> Result<(), SvcError>;
    /// Update a node's replica state information.
    async fn update_replica_state(&self, state: Either<Replica, ReplicaId>);
    /// Update all node state information.
    async fn update_all(&self, setting_online: bool) -> Result<(), SvcError>;
    /// OnRegister callback when a node is re-registered with the registry via its heartbeat
    /// On success returns where it's reset the node as online or not.
    async fn on_register(&self, node_state: NodeState) -> Result<bool, SvcError>;
}

/// Getter operations on a io-engine locked `NodeWrapper` to get copies of its
/// resources, such as pools, replicas and nexuses.
#[async_trait]
pub(crate) trait GetterOps {
    async fn pools(&self) -> Vec<PoolState>;
    async fn pool_wrappers(&self) -> Vec<PoolWrapper>;
    async fn pool(&self, pool_id: &PoolId) -> Option<PoolState>;
    async fn pool_wrapper(&self, pool_id: &PoolId) -> Option<PoolWrapper>;

    async fn replicas(&self) -> Vec<Replica>;
    async fn replica(&self, replica: &ReplicaId) -> Option<Replica>;

    async fn nexuses(&self) -> Vec<Nexus>;
    async fn nexus(&self, nexus_id: &NexusId) -> Option<Nexus>;
    async fn volume_nexus(&self, volume_id: &VolumeId) -> Option<Nexus>;
}

#[async_trait]
impl GetterOps for Arc<tokio::sync::RwLock<NodeWrapper>> {
    async fn pools(&self) -> Vec<PoolState> {
        let node = self.read().await;
        node.pools()
    }
    async fn pool_wrappers(&self) -> Vec<PoolWrapper> {
        let node = self.read().await;
        node.pool_wrappers()
    }
    async fn pool(&self, pool_id: &PoolId) -> Option<PoolState> {
        let node = self.read().await;
        node.pool(pool_id)
    }
    async fn pool_wrapper(&self, pool_id: &PoolId) -> Option<PoolWrapper> {
        let node = self.read().await;
        node.pool_wrapper(pool_id)
    }
    async fn replicas(&self) -> Vec<Replica> {
        let node = self.read().await;
        node.replicas()
    }
    async fn replica(&self, replica: &ReplicaId) -> Option<Replica> {
        let node = self.read().await;
        node.replica(replica)
    }
    async fn nexuses(&self) -> Vec<Nexus> {
        let node = self.read().await;
        node.nexuses()
    }
    async fn nexus(&self, nexus_id: &NexusId) -> Option<Nexus> {
        let node = self.read().await;
        node.nexus(nexus_id)
    }
    async fn volume_nexus(&self, volume_id: &VolumeId) -> Option<Nexus> {
        let node = self.read().await;
        node.volume_nexus(volume_id)
    }
}

#[async_trait]
impl InternalOps for Arc<tokio::sync::RwLock<NodeWrapper>> {
    async fn grpc_lock(&self) -> Arc<tokio::sync::Mutex<()>> {
        self.write().await.lock.clone()
    }

    async fn update_nexus_states(&self, mut ctx: &mut GrpcClient) -> Result<(), SvcError> {
        NodeWrapper::update_nexus_states(self, ctx.deref_mut()).await
    }
    async fn update_nexus_state(&self, state: Either<Nexus, NexusId>) {
        NodeWrapper::update_nexus_state(self, state).await;
    }

    async fn update_pool_states(&self, mut ctx: &mut GrpcClient) -> Result<(), SvcError> {
        NodeWrapper::update_pool_states(self, ctx.deref_mut()).await
    }

    async fn update_pool_state(&self, state: Either<PoolState, PoolId>) {
        NodeWrapper::update_pool_state(self, state).await;
    }

    async fn update_replica_states(&self, mut ctx: &mut GrpcClient) -> Result<(), SvcError> {
        NodeWrapper::update_replica_states(self, ctx.deref_mut()).await
    }

    async fn update_replica_state(&self, state: Either<Replica, ReplicaId>) {
        NodeWrapper::update_replica_state(self, state).await;
    }

    async fn update_all(&self, setting_online: bool) -> Result<(), SvcError> {
        let ctx = self.read().await.grpc_context_ext(GETS_TIMEOUT)?;
        match ctx.connect_locked().await {
            Ok(mut lock) => {
                let node_fetcher = self.read().await.fetcher();
                let results = node_fetcher.fetch_resources(lock.deref_mut()).await;

                let mut node = self.write().await;
                node.update(setting_online, results)
            }
            Err((_guard, error)) => {
                self.write().await.set_status(NodeStatus::Unknown);
                Err(error)
            }
        }
    }

    async fn on_register(&self, node_state: NodeState) -> Result<bool, SvcError> {
        let (not_online, state_changed) = {
            let mut node = self.write().await;
            // if the state changed we should update the node to fetch latest information
            let state_changed = node.set_state(node_state);
            node.pet().await;
            (!node.is_online(), state_changed)
        };
        // if the node was not previously online or the state has changed then let's update all
        // states right away
        if not_online || state_changed {
            self.update_all(not_online).await.map(|_| true)
        } else {
            Ok(false)
        }
    }
}

#[async_trait]
impl ClientOps for Arc<tokio::sync::RwLock<NodeWrapper>> {
    async fn grpc_client_locked(&self, request: MessageId) -> Result<GrpcClientLocked, SvcError> {
        if !self.read().await.is_online() {
            return Err(SvcError::NodeNotOnline {
                node: self.read().await.id().clone(),
            });
        }
        let ctx = self.read().await.grpc_context_ext(request)?;
        ctx.connect_locked().await.map_err(|(_, error)| error)
    }
}

#[async_trait]
impl PoolApi for Arc<tokio::sync::RwLock<NodeWrapper>> {
    /// Create a pool on the node via gRPC.
    async fn create_pool(&self, request: &CreatePool) -> Result<PoolState, SvcError> {
        let dataplane = self.grpc_client_locked(request.id()).await?;
        let create_response = dataplane.create_pool(request).await;

        match create_response {
            Ok(pool) => {
                self.update_pool_state(Either::Insert(pool.clone())).await;
                let mut ctx = dataplane.reconnect(GETS_TIMEOUT).await?;
                self.update_replica_states(ctx.deref_mut()).await?;
                Ok(pool)
            }
            Err(error) => {
                let mut ctx = dataplane.reconnect(GETS_TIMEOUT).await?;
                self.update_pool_states(ctx.deref_mut()).await?;
                self.update_replica_states(ctx.deref_mut()).await?;
                let pool = self.read().await.pool(&request.id);
                match (pool, &error) {
                    (Some(pool), SvcError::GrpcRequestError { source, .. })
                        if source.code() == tonic::Code::AlreadyExists =>
                    {
                        Ok(pool)
                    }
                    _ => Err(error),
                }
            }
        }
    }
    /// Destroy a pool on the node via gRPC.
    async fn destroy_pool(&self, request: &DestroyPool) -> Result<(), SvcError> {
        let dataplane = self.grpc_client_locked(request.id()).await?;
        let destroy_response = dataplane.destroy_pool(request).await;
        match destroy_response {
            Err(SvcError::GrpcRequestError { source, .. })
                if source.code() == tonic::Code::NotFound =>
            {
                let mut ctx = dataplane.reconnect(GETS_TIMEOUT).await?;
                self.update_pool_states(ctx.deref_mut()).await?;
                Ok(())
            }
            Err(error) => Err(error),
            Ok(_) => {
                self.update_pool_state(Either::Remove(request.id.clone()))
                    .await;
                Ok(())
            }
        }
    }
}

#[async_trait]
impl ReplicaApi for Arc<tokio::sync::RwLock<NodeWrapper>> {
    /// Create a replica on the pool via gRPC.
    async fn create_replica(&self, request: &CreateReplica) -> Result<Replica, SvcError> {
        if request.uuid == ReplicaId::default() {
            return Err(SvcError::InvalidUuid {
                uuid: request.uuid.to_string(),
                kind: ResourceKind::Replica,
            });
        }
        let dataplane = self.grpc_client_locked(request.id()).await?;
        let replica = dataplane.create_replica(request).await;

        match replica {
            Ok(replica) => {
                self.update_replica_state(Either::Insert(replica.clone()))
                    .await;

                let mut ctx = dataplane.reconnect(GETS_TIMEOUT).await?;
                self.update_pool_states(ctx.deref_mut()).await?;

                Ok(replica)
            }
            Err(error) => {
                let mut ctx = dataplane.reconnect(GETS_TIMEOUT).await?;
                self.update_replica_states(ctx.deref_mut()).await?;
                self.update_pool_states(ctx.deref_mut()).await?;

                match self.read().await.replicas().into_iter().find(|replica| {
                    replica.uuid == request.uuid
                        || replica.name
                            == ReplicaName::from_opt_uuid(request.name.as_ref(), &request.uuid)
                }) {
                    Some(replica) => Ok(replica),
                    None => Err(error),
                }
            }
        }
    }

    /// Destroy a replica on the pool via gRPC.
    async fn destroy_replica(&self, request: &DestroyReplica) -> Result<(), SvcError> {
        let dataplane = self.grpc_client_locked(request.id()).await?;
        let _ = dataplane.destroy_replica(request).await;
        let mut ctx = dataplane.reconnect(GETS_TIMEOUT).await?;
        self.update_replica_states(ctx.deref_mut()).await?;
        // todo: remove when CAS-1107 is resolved
        if let Some(replica) = self.read().await.replica(&request.uuid) {
            if replica.pool_id == request.pool_id {
                return Err(SvcError::Internal {
                    details: "replica was not destroyed by the io-engine".to_string(),
                });
            }
        }
        self.update_pool_states(ctx.deref_mut()).await.ok();
        Ok(())
    }

    /// Share a replica on the pool via gRPC.
    async fn share_replica(&self, request: &ShareReplica) -> Result<String, SvcError> {
        let dataplane = self.grpc_client_locked(request.id()).await?;
        let share = dataplane.share_replica(request).await?;
        let mut ctx = dataplane.reconnect(GETS_TIMEOUT).await?;
        self.update_replica_states(ctx.deref_mut()).await?;
        Ok(share)
    }

    /// Unshare a replica on the pool via gRPC.
    async fn unshare_replica(&self, request: &UnshareReplica) -> Result<String, SvcError> {
        let dataplane = self.grpc_client_locked(request.id()).await?;
        let local_uri = dataplane.unshare_replica(request).await?;
        let mut ctx = dataplane.reconnect(GETS_TIMEOUT).await?;
        self.update_replica_states(ctx.deref_mut()).await?;
        Ok(local_uri)
    }
}

#[async_trait]
impl NexusApi<()> for Arc<tokio::sync::RwLock<NodeWrapper>> {
    /// Create a nexus on the node via gRPC.
    async fn create_nexus(&self, request: &CreateNexus) -> Result<Nexus, SvcError> {
        if request.uuid == NexusId::default() {
            return Err(SvcError::InvalidUuid {
                uuid: request.uuid.to_string(),
                kind: ResourceKind::Nexus,
            });
        }
        let dataplane = self.grpc_client_locked(request.id()).await?;
        match dataplane.create_nexus(request).await {
            Ok(nexus) => {
                self.update_nexus_state(Either::Insert(nexus.clone())).await;
                Ok(nexus)
            }
            Err(error) => {
                let mut ctx = dataplane.reconnect(GETS_TIMEOUT).await?;
                self.update_nexus_states(ctx.deref_mut()).await?;
                let nexus_name = request.name();
                let nexuses = self.read().await.nexuses();
                match nexuses
                    .iter()
                    .find(|nexus| nexus.uuid == request.uuid && nexus.name == nexus_name)
                {
                    Some(nexus) => {
                        tracing::warn!(
                            node.id = request.node.as_str(),
                            nexus.uuid = request.uuid.as_str(),
                            nexus.name = nexus_name,
                            "Trying to create a nexus which already exists"
                        );
                        Ok(nexus.clone())
                    }
                    None if nexuses
                        .iter()
                        .any(|nexus| nexus.uuid != request.uuid && nexus.name == nexus_name) =>
                    {
                        tracing::error!(
                            node.id = request.node.as_str(),
                            nexus.uuid = request.uuid.as_str(),
                            nexus.name = nexus_name,
                            "Trying to create a nexus with a name which already exists"
                        );
                        Err(SvcError::AlreadyExists {
                            kind: ResourceKind::Nexus,
                            id: nexus_name,
                        })
                    }
                    None => Err(error),
                }
            }
        }
    }

    /// Destroy a nexus on the node via gRPC.
    async fn destroy_nexus(&self, request: &DestroyNexus) -> Result<(), SvcError> {
        let dataplane = self.grpc_client_locked(request.id()).await?;
        match dataplane.destroy_nexus(request).await {
            Ok(()) => {
                self.update_nexus_state(Either::Remove(request.uuid.clone()))
                    .await;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    async fn shutdown_nexus(&self, request: &ShutdownNexus) -> Result<(), SvcError> {
        let dataplane = self.grpc_client_locked(request.id()).await?;
        let result = dataplane.shutdown_nexus(request).await;
        let mut ctx = dataplane.reconnect(GETS_TIMEOUT).await?;
        self.update_nexus_states(ctx.deref_mut()).await?;
        match result {
            Ok(()) => Ok(()),
            Err(error) if error.tonic_code() == tonic::Code::NotFound => {
                Err(SvcError::NexusNotFound {
                    nexus_id: request.uuid().to_string(),
                })
            }
            Err(error) => Err(error),
        }
    }
}

#[async_trait]
impl NexusShareApi<String, ()> for Arc<tokio::sync::RwLock<NodeWrapper>> {
    /// Share a nexus on the node via gRPC.
    async fn share_nexus(&self, request: &ShareNexus) -> Result<String, SvcError> {
        let dataplane = self.grpc_client_locked(request.id()).await?;
        match dataplane.share_nexus(request).await {
            Ok(nexus) => {
                self.update_nexus_state(Either::Insert(nexus.clone())).await;
                Ok(nexus.device_uri)
            }
            Err(error) => Err(error),
        }
    }

    /// Unshare a nexus on the node via gRPC.
    async fn unshare_nexus(&self, request: &UnshareNexus) -> Result<(), SvcError> {
        let dataplane = self.grpc_client_locked(request.id()).await?;
        match dataplane.unshare_nexus(request).await {
            Ok(nexus) => {
                self.update_nexus_state(Either::Insert(nexus.clone())).await;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}

#[async_trait]
impl NexusChildApi<Child, (), ()> for Arc<tokio::sync::RwLock<NodeWrapper>> {
    /// Add a child to a nexus via gRPC.
    async fn add_child(&self, request: &AddNexusChild) -> Result<Child, SvcError> {
        let dataplane = self.grpc_client_locked(request.id()).await?;
        match dataplane.add_child(request).await {
            Ok(nexus) => {
                let child = nexus.children.iter().find(|c| c.uri == request.uri);
                let result = child.cloned().ok_or(SvcError::ChildNotFound {
                    nexus: request.nexus.to_string(),
                    child: request.uri.to_string(),
                });
                self.update_nexus_state(Either::Insert(nexus)).await;
                result
            }
            Err(error) => Err(error),
        }
    }

    /// Remove a child from its parent nexus via gRPC.
    async fn remove_child(&self, request: &RemoveNexusChild) -> Result<(), SvcError> {
        let dataplane = self.grpc_client_locked(request.id()).await?;
        match dataplane.remove_child(request).await {
            Ok(nexus) => {
                let removed = !nexus.children.iter().any(|c| c.uri == request.uri);
                self.update_nexus_state(Either::Insert(nexus)).await;
                match removed {
                    true => Ok(()),
                    false => Err(SvcError::ChildNotFound {
                        nexus: request.nexus.to_string(),
                        child: request.uri.to_string(),
                    }),
                }
            }
            Err(error) => Err(error),
        }
    }

    async fn fault_child(&self, request: &FaultNexusChild) -> Result<(), SvcError> {
        let dataplane = self.grpc_client_locked(request.id()).await?;
        let result = dataplane.fault_child(request).await;
        // todo: v1 api should return a Nexus as well.
        let mut ctx = dataplane.reconnect(GETS_TIMEOUT).await?;
        self.update_nexus_states(ctx.deref_mut()).await?;
        match result {
            Ok(_) => Ok(()),
            Err(error) => {
                let child =
                    self.read().await.nexus(&request.nexus).and_then(|nexus| {
                        nexus.children.into_iter().find(|c| c.uri == request.uri)
                    });
                match child {
                    Some(child) if child.state.faulted() => Ok(()),
                    _ => Err(error),
                }
            }
        }
    }
}

impl From<&NodeWrapper> for NodeState {
    fn from(node: &NodeWrapper) -> Self {
        node.node_state().clone()
    }
}