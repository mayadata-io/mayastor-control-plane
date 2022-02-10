use crate::{
    common,
    grpc_opts::Context,
    nexus, replica, volume,
    volume::{
        get_volumes_request, CreateVolumeRequest, DestroyVolumeRequest, PublishVolumeRequest,
        SetVolumeReplicaRequest, ShareVolumeRequest, UnpublishVolumeRequest, UnshareVolumeRequest,
    },
};
use common_lib::{
    mbus_api::{v0::Volumes, ReplyError, ResourceKind},
    types::v0::{
        message_bus::{
            CreateVolume, DestroyVolume, ExplicitNodeTopology, Filter, LabelledTopology, Nexus,
            NexusId, NodeId, NodeTopology, PoolTopology, PublishVolume, ReplicaId, ReplicaStatus,
            ReplicaTopology, SetVolumeReplica, ShareVolume, Topology, UnpublishVolume,
            UnshareVolume, Volume, VolumeId, VolumeLabels, VolumePolicy, VolumeShareProtocol,
            VolumeState,
        },
        store::volume::{VolumeSpec, VolumeSpecStatus, VolumeTarget},
    },
};
use std::{collections::HashMap, convert::TryFrom};

/// all volume crud operations to be a part of the VolumeOperations trait
#[tonic::async_trait]
pub trait VolumeOperations: Send + Sync {
    /// create a volume
    async fn create(
        &self,
        req: &dyn CreateVolumeInfo,
        ctx: Option<Context>,
    ) -> Result<Volume, ReplyError>;
    /// get volumes
    async fn get(&self, filter: Filter, ctx: Option<Context>) -> Result<Volumes, ReplyError>;
    /// destroy a volume
    async fn destroy(
        &self,
        req: &dyn DestroyVolumeInfo,
        ctx: Option<Context>,
    ) -> Result<(), ReplyError>;
    /// share a volume
    async fn share(
        &self,
        req: &dyn ShareVolumeInfo,
        ctx: Option<Context>,
    ) -> Result<String, ReplyError>;
    /// unshare a volume
    async fn unshare(
        &self,
        req: &dyn UnshareVolumeInfo,
        ctx: Option<Context>,
    ) -> Result<(), ReplyError>;
    /// publish a volume
    async fn publish(
        &self,
        req: &dyn PublishVolumeInfo,
        ctx: Option<Context>,
    ) -> Result<Volume, ReplyError>;
    /// unpublish a volume
    async fn unpublish(
        &self,
        req: &dyn UnpublishVolumeInfo,
        ctx: Option<Context>,
    ) -> Result<Volume, ReplyError>;
    /// increase or decrease volume replica
    async fn set_volume_replica(
        &self,
        req: &dyn SetVolumeReplicaInfo,
        ctx: Option<Context>,
    ) -> Result<Volume, ReplyError>;
    /// liveness probe for volume service
    async fn probe(&self, ctx: Option<Context>) -> Result<bool, ReplyError>;
}

impl From<Volume> for volume::Volume {
    fn from(volume: Volume) -> Self {
        let spec_status: common::SpecStatus = volume.spec().status.into();
        let volume_definition = volume::VolumeDefinition {
            spec: Some(volume::VolumeSpec {
                uuid: Some(volume.spec().uuid.to_string()),
                size: volume.spec().size,
                labels: volume
                    .spec()
                    .labels
                    .map(|labels| crate::common::StringMapValue { value: labels }),
                num_replicas: volume.spec().num_replicas.into(),
                target: volume.spec().target.map(|target| target.into()),
                policy: Some(volume.spec().policy.into()),
                topology: volume.spec().topology.map(|topology| topology.into()),
                last_nexus_id: volume.spec().last_nexus_id.map(|id| id.to_string()),
            }),
            metadata: Some(volume::Metadata {
                status: spec_status as i32,
            }),
        };
        let status: nexus::NexusStatus = volume.state().status.into();
        let volume_state = volume::VolumeState {
            uuid: Some(volume.state().uuid.to_string()),
            size: volume.state().size,
            status: status as i32,
            target: volume.state().target.map(|target| target.into()),
            replica_topology: to_grpc_replica_topology_map(volume.state().replica_topology),
        };
        volume::Volume {
            definition: Some(volume_definition),
            state: Some(volume_state),
        }
    }
}

impl TryFrom<volume::Volume> for Volume {
    type Error = ReplyError;
    fn try_from(volume_grpc_type: volume::Volume) -> Result<Self, Self::Error> {
        let grpc_volume_definition = match volume_grpc_type.definition {
            Some(definition) => definition,
            None => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
        };
        let grpc_volume_spec = match grpc_volume_definition.spec {
            Some(spec) => spec,
            None => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
        };
        let grpc_volume_meta = match grpc_volume_definition.metadata {
            Some(meta) => meta,
            None => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
        };
        let grpc_volume_state = match volume_grpc_type.state {
            Some(state) => state,
            None => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
        };
        let volume_spec_status = match common::SpecStatus::from_i32(grpc_volume_meta.status) {
            Some(status) => match status {
                common::SpecStatus::Created => {
                    VolumeSpecStatus::Created(grpc_volume_meta.status.into())
                }
                _ => match common::SpecStatus::from_i32(grpc_volume_meta.status) {
                    Some(status) => status.into(),
                    None => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
                },
            },
            None => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
        };
        let volume_spec = VolumeSpec {
            uuid: match grpc_volume_spec.uuid {
                Some(id) => match VolumeId::try_from(id) {
                    Ok(volume_id) => volume_id,
                    Err(_) => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
                },
                None => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
            },
            size: grpc_volume_spec.size,
            labels: match grpc_volume_spec.labels {
                Some(labels) => Some(labels.value),
                None => None,
            },
            num_replicas: grpc_volume_spec.num_replicas as u8,
            status: volume_spec_status,
            target: match grpc_volume_spec.target {
                Some(target) => match VolumeTarget::try_from(target) {
                    Ok(target) => Some(target),
                    Err(_) => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
                },
                None => None,
            },
            policy: match grpc_volume_spec.policy {
                Some(policy) => policy.into(),
                None => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
            },
            topology: match grpc_volume_spec.topology {
                Some(topology) => match Topology::try_from(topology) {
                    Ok(topology) => Some(topology),
                    Err(_) => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
                },
                None => None,
            },
            sequencer: Default::default(),
            last_nexus_id: match grpc_volume_spec.last_nexus_id {
                Some(id) => match NexusId::try_from(id) {
                    Ok(volume_id) => Some(volume_id),
                    Err(_) => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
                },
                None => None,
            },
            operation: None,
        };
        let volume_state = VolumeState {
            uuid: match grpc_volume_state.uuid {
                Some(id) => match VolumeId::try_from(id) {
                    Ok(volume_id) => volume_id,
                    Err(_) => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
                },
                None => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
            },
            size: grpc_volume_state.size,
            status: match nexus::NexusStatus::from_i32(grpc_volume_state.status) {
                Some(status) => status.into(),
                None => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
            },
            target: match grpc_volume_state.target {
                Some(target) => match Nexus::try_from(target) {
                    Ok(target) => Some(target),
                    Err(_) => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
                },
                None => None,
            },
            replica_topology: match to_replica_topology_map(grpc_volume_state.replica_topology) {
                Ok(replica_topology_map) => replica_topology_map,
                Err(_) => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
            },
        };
        Ok(Volume::new(volume_spec, volume_state))
    }
}

impl TryFrom<volume::Volumes> for Volumes {
    type Error = ReplyError;
    fn try_from(grpc_volume_type: volume::Volumes) -> Result<Self, Self::Error> {
        let mut volumes: Vec<Volume> = vec![];
        for volume in grpc_volume_type.volumes {
            volumes.push(Volume::try_from(volume.clone())?)
        }
        Ok(Volumes(volumes))
    }
}

impl From<Volumes> for volume::Volumes {
    fn from(volumes: Volumes) -> Self {
        volume::Volumes {
            volumes: volumes
                .into_inner()
                .iter()
                .map(|volume| volume.clone().into())
                .collect(),
        }
    }
}

impl TryFrom<volume::ReplicaTopology> for ReplicaTopology {
    type Error = ReplyError;
    fn try_from(replica_topology_grpc_type: volume::ReplicaTopology) -> Result<Self, Self::Error> {
        let node = replica_topology_grpc_type.node.map(|node| node.into());
        let pool = replica_topology_grpc_type.pool.map(|pool| pool.into());
        let status = match ReplicaStatus::try_from(replica_topology_grpc_type.status) {
            Ok(status) => status,
            Err(_) => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
        };
        Ok(ReplicaTopology::new(node, pool, status))
    }
}

impl From<ReplicaTopology> for volume::ReplicaTopology {
    fn from(replica_topology: ReplicaTopology) -> Self {
        let node = replica_topology.node().as_ref().map(|id| id.to_string());
        let pool = replica_topology.pool().as_ref().map(|id| id.to_string());
        let status: replica::ReplicaStatus = replica_topology.status().clone().into();
        volume::ReplicaTopology {
            node,
            pool,
            status: status as i32,
        }
    }
}

impl TryFrom<volume::Topology> for Topology {
    type Error = ReplyError;
    fn try_from(topology_grpc_type: volume::Topology) -> Result<Self, Self::Error> {
        let topo = Topology {
            node: match topology_grpc_type.node {
                Some(node_topo) => match NodeTopology::try_from(node_topo) {
                    Ok(node_topo) => Some(node_topo),
                    Err(_) => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
                },
                None => None,
            },
            pool: match topology_grpc_type.pool {
                Some(pool_topo) => match PoolTopology::try_from(pool_topo) {
                    Ok(pool_topo) => Some(pool_topo),
                    Err(_) => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
                },
                None => None,
            },
        };
        Ok(topo)
    }
}

impl From<Topology> for volume::Topology {
    fn from(topology: Topology) -> Self {
        volume::Topology {
            node: topology.node.map(|topo| topo.into()),
            pool: topology.pool.map(|topo| topo.into()),
        }
    }
}

impl TryFrom<volume::NodeTopology> for NodeTopology {
    type Error = ReplyError;
    fn try_from(node_topology_grpc_type: volume::NodeTopology) -> Result<Self, Self::Error> {
        let node_topo = match node_topology_grpc_type.topology {
            Some(topo) => match topo {
                volume::node_topology::Topology::Labelled(labels) => {
                    NodeTopology::Labelled(labels.into())
                }
                volume::node_topology::Topology::Explicit(explicit) => {
                    NodeTopology::Explicit(explicit.into())
                }
            },
            None => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
        };
        Ok(node_topo)
    }
}

impl From<NodeTopology> for volume::NodeTopology {
    fn from(src: NodeTopology) -> Self {
        match src {
            NodeTopology::Labelled(labels) => volume::NodeTopology {
                topology: Some(volume::node_topology::Topology::Labelled(labels.into())),
            },
            NodeTopology::Explicit(explicit) => volume::NodeTopology {
                topology: Some(volume::node_topology::Topology::Explicit(explicit.into())),
            },
        }
    }
}

impl TryFrom<volume::PoolTopology> for PoolTopology {
    type Error = ReplyError;
    fn try_from(pool_topology_grpc_type: volume::PoolTopology) -> Result<Self, Self::Error> {
        let pool_topo = match pool_topology_grpc_type.topology {
            Some(topology) => match topology {
                volume::pool_topology::Topology::Labelled(labels) => {
                    PoolTopology::Labelled(labels.into())
                }
            },
            None => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
        };
        Ok(pool_topo)
    }
}

impl From<PoolTopology> for volume::PoolTopology {
    fn from(src: PoolTopology) -> Self {
        match src {
            PoolTopology::Labelled(labels) => volume::PoolTopology {
                topology: Some(volume::pool_topology::Topology::Labelled(labels.into())),
            },
        }
    }
}

impl From<volume::LabelledTopology> for LabelledTopology {
    fn from(labelled_topology_grpc_type: volume::LabelledTopology) -> Self {
        LabelledTopology {
            exclusion: match labelled_topology_grpc_type.exclusion {
                Some(labels) => labels.value,
                None => HashMap::new(),
            },
            inclusion: match labelled_topology_grpc_type.inclusion {
                Some(labels) => labels.value,
                None => HashMap::new(),
            },
        }
    }
}

impl From<LabelledTopology> for volume::LabelledTopology {
    fn from(topo: LabelledTopology) -> Self {
        volume::LabelledTopology {
            exclusion: Some(crate::common::StringMapValue {
                value: topo.exclusion,
            }),
            inclusion: Some(crate::common::StringMapValue {
                value: topo.inclusion,
            }),
        }
    }
}

impl From<volume::ExplicitNodeTopology> for ExplicitNodeTopology {
    fn from(explicit_topology_grpc_type: volume::ExplicitNodeTopology) -> Self {
        ExplicitNodeTopology {
            allowed_nodes: explicit_topology_grpc_type
                .allowed_nodes
                .into_iter()
                .map(|node| node.into())
                .collect(),
            preferred_nodes: explicit_topology_grpc_type
                .preferred_nodes
                .into_iter()
                .map(|node| node.into())
                .collect(),
        }
    }
}

impl From<ExplicitNodeTopology> for volume::ExplicitNodeTopology {
    fn from(explicit_topo: ExplicitNodeTopology) -> Self {
        volume::ExplicitNodeTopology {
            allowed_nodes: explicit_topo
                .allowed_nodes
                .into_iter()
                .map(|node| node.to_string())
                .collect(),
            preferred_nodes: explicit_topo
                .preferred_nodes
                .into_iter()
                .map(|node| node.to_string())
                .collect(),
        }
    }
}

impl From<volume::VolumePolicy> for VolumePolicy {
    fn from(policy_grpc_type: volume::VolumePolicy) -> Self {
        VolumePolicy {
            self_heal: policy_grpc_type.self_heal,
        }
    }
}

impl From<VolumePolicy> for volume::VolumePolicy {
    fn from(policy: VolumePolicy) -> Self {
        volume::VolumePolicy {
            self_heal: policy.self_heal,
        }
    }
}

impl TryFrom<volume::VolumeTarget> for VolumeTarget {
    type Error = ReplyError;
    fn try_from(volume_target_grpc_type: volume::VolumeTarget) -> Result<Self, Self::Error> {
        let target = VolumeTarget::new(
            volume_target_grpc_type.node_id.into(),
            match volume_target_grpc_type.nexus_id {
                Some(id) => match NexusId::try_from(id) {
                    Ok(nexusid) => nexusid,
                    Err(_) => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
                },
                None => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
            },
            match volume_target_grpc_type.protocol {
                Some(i) => match volume::VolumeShareProtocol::from_i32(i) {
                    Some(protocol) => Some(protocol.into()),
                    None => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
                },
                None => None,
            },
        );
        Ok(target)
    }
}

impl From<VolumeTarget> for volume::VolumeTarget {
    fn from(target: VolumeTarget) -> Self {
        volume::VolumeTarget {
            node_id: target.node().to_string(),
            nexus_id: Some(target.nexus().to_string()),
            protocol: match target.protocol() {
                None => None,
                Some(protocol) => {
                    let protocol: volume::VolumeShareProtocol = (*protocol).into();
                    Some(protocol as i32)
                }
            },
        }
    }
}

impl From<volume::VolumeShareProtocol> for VolumeShareProtocol {
    fn from(src: volume::VolumeShareProtocol) -> Self {
        match src {
            volume::VolumeShareProtocol::Nvmf => Self::Nvmf,
            volume::VolumeShareProtocol::Iscsi => Self::Iscsi,
        }
    }
}

impl From<VolumeShareProtocol> for volume::VolumeShareProtocol {
    fn from(src: VolumeShareProtocol) -> Self {
        match src {
            VolumeShareProtocol::Nvmf => Self::Nvmf,
            VolumeShareProtocol::Iscsi => Self::Iscsi,
        }
    }
}

impl TryFrom<get_volumes_request::Filter> for Filter {
    type Error = ReplyError;
    fn try_from(filter: get_volumes_request::Filter) -> Result<Self, Self::Error> {
        Ok(match filter {
            get_volumes_request::Filter::Volume(volume_filter) => {
                Filter::Volume(match VolumeId::try_from(volume_filter.volume_id) {
                    Ok(volumeid) => volumeid,
                    Err(_) => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
                })
            }
        })
    }
}

/// trait to be implemented for CreateVolume operation
pub trait CreateVolumeInfo: Send + Sync {
    /// uuid of the volume
    fn uuid(&self) -> VolumeId;
    /// size in bytes of the volume
    fn size(&self) -> u64;
    /// no of replicas of the volume
    fn replicas(&self) -> u64;
    /// volume policy of the volume, i.e self_heal
    fn policy(&self) -> VolumePolicy;
    /// topology configuration of the volume
    fn topology(&self) -> Option<Topology>;
    /// labels to be added to the volumes for topology based scheduling
    fn labels(&self) -> Option<VolumeLabels>;
}

impl CreateVolumeInfo for CreateVolume {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn replicas(&self) -> u64 {
        self.replicas
    }

    fn policy(&self) -> VolumePolicy {
        self.policy.clone()
    }

    fn topology(&self) -> Option<Topology> {
        self.topology.clone()
    }

    fn labels(&self) -> Option<VolumeLabels> {
        self.labels.clone()
    }
}

impl CreateVolumeInfo for CreateVolumeRequest {
    fn uuid(&self) -> VolumeId {
        VolumeId::try_from(self.uuid.clone().unwrap()).unwrap()
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn replicas(&self) -> u64 {
        self.replicas
    }

    fn policy(&self) -> VolumePolicy {
        match self.policy.clone() {
            Some(policy) => policy.into(),
            None => VolumePolicy::default(),
        }
    }

    fn topology(&self) -> Option<Topology> {
        match self.topology.clone() {
            Some(topology) => match Topology::try_from(topology) {
                Ok(topology) => Some(topology),
                // TODO:: Figure out some way to handle this situation
                Err(_) => None,
            },
            None => None,
        }
    }

    fn labels(&self) -> Option<VolumeLabels> {
        match self.labels.clone() {
            None => None,
            Some(labels) => Some(labels.value),
        }
    }
}

impl From<&dyn CreateVolumeInfo> for CreateVolume {
    fn from(data: &dyn CreateVolumeInfo) -> Self {
        Self {
            uuid: data.uuid(),
            size: data.size(),
            replicas: data.replicas(),
            policy: data.policy(),
            topology: data.topology(),
            labels: data.labels(),
        }
    }
}

impl From<&dyn CreateVolumeInfo> for CreateVolumeRequest {
    fn from(data: &dyn CreateVolumeInfo) -> Self {
        Self {
            uuid: Some(data.uuid().to_string()),
            size: data.size(),
            replicas: data.replicas(),
            policy: Some(data.policy().into()),
            topology: data.topology().map(|topo| topo.into()),
            labels: data
                .labels()
                .map(|labels| crate::common::StringMapValue { value: labels }),
        }
    }
}

/// trait to be implemented for DestroyVolume operation
pub trait DestroyVolumeInfo: Send + Sync {
    /// uuid of the volume to be destroyed
    fn uuid(&self) -> VolumeId;
}

impl DestroyVolumeInfo for DestroyVolume {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }
}

impl DestroyVolumeInfo for DestroyVolumeRequest {
    fn uuid(&self) -> VolumeId {
        VolumeId::try_from(self.uuid.clone().unwrap()).unwrap()
    }
}

impl From<&dyn DestroyVolumeInfo> for DestroyVolume {
    fn from(data: &dyn DestroyVolumeInfo) -> Self {
        Self { uuid: data.uuid() }
    }
}

impl From<&dyn DestroyVolumeInfo> for DestroyVolumeRequest {
    fn from(data: &dyn DestroyVolumeInfo) -> Self {
        Self {
            uuid: Some(data.uuid().to_string()),
        }
    }
}

/// trait to be implemented for ShareVolume operation
pub trait ShareVolumeInfo: Send + Sync {
    /// uuid of the volume to be shared
    fn uuid(&self) -> VolumeId;
    /// protocol over which the volume be shared
    fn share(&self) -> VolumeShareProtocol;
}

impl ShareVolumeInfo for ShareVolume {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }

    fn share(&self) -> VolumeShareProtocol {
        self.protocol
    }
}

impl ShareVolumeInfo for ShareVolumeRequest {
    fn uuid(&self) -> VolumeId {
        VolumeId::try_from(self.uuid.clone().unwrap()).unwrap()
    }

    fn share(&self) -> VolumeShareProtocol {
        self.share.into()
    }
}

impl From<&dyn ShareVolumeInfo> for ShareVolume {
    fn from(data: &dyn ShareVolumeInfo) -> Self {
        Self {
            uuid: data.uuid(),
            protocol: data.share(),
        }
    }
}

impl From<&dyn ShareVolumeInfo> for ShareVolumeRequest {
    fn from(data: &dyn ShareVolumeInfo) -> Self {
        let share: volume::VolumeShareProtocol = data.share().into();
        Self {
            uuid: Some(data.uuid().to_string()),
            share: share as i32,
        }
    }
}

/// trait to be implemented for UnshareVolume operation
pub trait UnshareVolumeInfo: Send + Sync {
    /// uuid of the volume to be unshared
    fn uuid(&self) -> VolumeId;
}

impl UnshareVolumeInfo for UnshareVolume {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }
}

impl UnshareVolumeInfo for UnshareVolumeRequest {
    fn uuid(&self) -> VolumeId {
        VolumeId::try_from(self.uuid.clone().unwrap()).unwrap()
    }
}

impl From<&dyn UnshareVolumeInfo> for UnshareVolume {
    fn from(data: &dyn UnshareVolumeInfo) -> Self {
        Self { uuid: data.uuid() }
    }
}

impl From<&dyn UnshareVolumeInfo> for UnshareVolumeRequest {
    fn from(data: &dyn UnshareVolumeInfo) -> Self {
        Self {
            uuid: Some(data.uuid().to_string()),
        }
    }
}

/// trait to be implemented for PublishVolume operation
pub trait PublishVolumeInfo: Send + Sync {
    /// uuid of the volume to be published
    fn uuid(&self) -> VolumeId;
    /// the node where front-end IO will be sent to
    fn target_node(&self) -> Option<NodeId>;
    /// the protocol over which volume be published
    fn share(&self) -> Option<VolumeShareProtocol>;
}

impl PublishVolumeInfo for PublishVolume {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }

    fn target_node(&self) -> Option<NodeId> {
        self.target_node.clone()
    }

    fn share(&self) -> Option<VolumeShareProtocol> {
        self.share
    }
}

impl PublishVolumeInfo for PublishVolumeRequest {
    fn uuid(&self) -> VolumeId {
        VolumeId::try_from(self.uuid.clone().unwrap()).unwrap()
    }

    fn target_node(&self) -> Option<NodeId> {
        self.target_node
            .clone()
            .map(|target_node| target_node.into())
    }

    fn share(&self) -> Option<VolumeShareProtocol> {
        self.share.map(|protocol| {
            volume::VolumeShareProtocol::from_i32(protocol)
                .unwrap()
                .into()
        })
    }
}

impl From<&dyn PublishVolumeInfo> for PublishVolume {
    fn from(data: &dyn PublishVolumeInfo) -> Self {
        Self {
            uuid: data.uuid(),
            target_node: data.target_node(),
            share: data.share(),
        }
    }
}

impl From<&dyn PublishVolumeInfo> for PublishVolumeRequest {
    fn from(data: &dyn PublishVolumeInfo) -> Self {
        let share: Option<i32> = match data.share() {
            None => None,
            Some(protocol) => {
                let protocol: volume::VolumeShareProtocol = protocol.into();
                Some(protocol as i32)
            }
        };
        Self {
            uuid: Some(data.uuid().to_string()),
            target_node: data.target_node().map(|node_id| node_id.to_string()),
            share,
        }
    }
}

/// trait to be implemented for PublishVolume operation
pub trait UnpublishVolumeInfo: Send + Sync {
    /// uuid of the volume to unpublish
    fn uuid(&self) -> VolumeId;
    /// force unpublish
    fn force(&self) -> bool;
}

impl UnpublishVolumeInfo for UnpublishVolume {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }

    fn force(&self) -> bool {
        self.force()
    }
}
impl UnpublishVolumeInfo for UnpublishVolumeRequest {
    fn uuid(&self) -> VolumeId {
        VolumeId::try_from(self.uuid.clone().unwrap()).unwrap()
    }

    fn force(&self) -> bool {
        self.force
    }
}

impl From<&dyn UnpublishVolumeInfo> for UnpublishVolume {
    fn from(data: &dyn UnpublishVolumeInfo) -> Self {
        UnpublishVolume::new(&data.uuid(), data.force())
    }
}

impl From<&dyn UnpublishVolumeInfo> for UnpublishVolumeRequest {
    fn from(data: &dyn UnpublishVolumeInfo) -> Self {
        Self {
            uuid: Some(data.uuid().to_string()),
            force: data.force(),
        }
    }
}

/// trait to be implemented for SetVolumeReplica operation
pub trait SetVolumeReplicaInfo: Send + Sync {
    /// uuid of the concerned volume
    fn uuid(&self) -> VolumeId;
    /// no of replicas we want to set for the volume
    fn replicas(&self) -> u8;
}

impl SetVolumeReplicaInfo for SetVolumeReplica {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }

    fn replicas(&self) -> u8 {
        self.replicas
    }
}
impl SetVolumeReplicaInfo for SetVolumeReplicaRequest {
    fn uuid(&self) -> VolumeId {
        VolumeId::try_from(self.uuid.clone().unwrap()).unwrap()
    }

    fn replicas(&self) -> u8 {
        self.replicas as u8
    }
}

impl From<&dyn SetVolumeReplicaInfo> for SetVolumeReplica {
    fn from(data: &dyn SetVolumeReplicaInfo) -> Self {
        Self {
            uuid: data.uuid(),
            replicas: data.replicas(),
        }
    }
}

impl From<&dyn SetVolumeReplicaInfo> for SetVolumeReplicaRequest {
    fn from(data: &dyn SetVolumeReplicaInfo) -> Self {
        Self {
            uuid: Some(data.uuid().to_string()),
            replicas: data.replicas().into(),
        }
    }
}

/// a helper to convert the replica topology map form grpc type to corresponding control plane type
fn to_replica_topology_map(
    map: HashMap<String, volume::ReplicaTopology>,
) -> Result<HashMap<ReplicaId, ReplicaTopology>, ReplyError> {
    let mut replica_topology_map: HashMap<ReplicaId, ReplicaTopology> = HashMap::new();
    for (k, v) in map {
        let replica_id = match ReplicaId::try_from(k) {
            Ok(id) => id,
            Err(_) => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
        };
        let replica_topology = match ReplicaTopology::try_from(v) {
            Ok(topology) => topology,
            Err(_) => return Err(ReplyError::unwrap_err(ResourceKind::Volume)),
        };
        replica_topology_map.insert(replica_id, replica_topology);
    }
    Ok(replica_topology_map)
}

/// a helper to convert the replica topology map form control plane type to corresponding grpc type
fn to_grpc_replica_topology_map(
    map: HashMap<ReplicaId, ReplicaTopology>,
) -> HashMap<String, volume::ReplicaTopology> {
    let mut replica_topology_map: HashMap<String, volume::ReplicaTopology> = HashMap::new();
    for (k, v) in map {
        replica_topology_map.insert(k.to_string(), v.into());
    }
    replica_topology_map
}