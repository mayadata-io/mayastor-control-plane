pub use super::traits_snapshots::*;
use crate::{
    common,
    context::Context,
    misc::traits::{StringValue, ValidateRequestTypes},
    nexus,
    operations::{Event, Pagination},
    replica, volume,
    volume::{
        get_volumes_request, CreateSnapshotVolumeRequest, CreateVolumeRequest,
        DestroyShutdownTargetRequest, DestroyVolumeRequest, PublishVolumeRequest,
        RegisteredTargets, RepublishVolumeRequest, ResizeVolumeRequest, SetVolumePropertyRequest,
        SetVolumeReplicaRequest, ShareVolumeRequest, UnpublishVolumeRequest, UnshareVolumeRequest,
    },
};
use events_api::event::{EventAction, EventCategory, EventMessage, EventMeta, EventSource};
use std::{borrow::Borrow, collections::HashMap, convert::TryFrom};
use stor_port::{
    transport_api::{v0::Volumes, ReplyError, ResourceKind},
    types::v0::{
        store::volume::{
            AffinityGroupSpec, FrontendConfig, InitiatorAC, TargetConfig, VolumeContentSource,
            VolumeMetadata, VolumeSpec, VolumeTarget,
        },
        transport::{
            AffinityGroup, CreateSnapshotVolume, CreateVolume, DestroyShutdownTargets,
            DestroyVolume, ExplicitNodeTopology, Filter, LabelledTopology, Nexus, NexusId,
            NexusNvmfConfig, NodeId, NodeTopology, NvmeNqn, PoolTopology, PublishVolume, ReplicaId,
            ReplicaStatus, ReplicaTopology, ReplicaUsage, RepublishVolume, ResizeVolume,
            SetVolumeProperty, SetVolumeReplica, ShareVolume, SnapshotId, Topology,
            UnpublishVolume, UnshareVolume, Volume, VolumeId, VolumeLabels, VolumePolicy,
            VolumeProperty, VolumeShareProtocol, VolumeState, VolumeUsage,
        },
    },
    IntoOption, IntoVec, TryIntoOption,
};

/// All volume crud operations to be a part of the VolumeOperations trait.
#[tonic::async_trait]
pub trait VolumeOperations: Send + Sync {
    /// Create a volume
    async fn create(
        &self,
        req: &dyn CreateVolumeInfo,
        ctx: Option<Context>,
    ) -> Result<Volume, ReplyError>;
    /// Get volumes
    async fn get(
        &self,
        filter: Filter,
        ignore_notfound: bool,
        pagination: Option<Pagination>,
        ctx: Option<Context>,
    ) -> Result<Volumes, ReplyError>;
    /// Destroy a volume
    async fn destroy(
        &self,
        req: &dyn DestroyVolumeInfo,
        ctx: Option<Context>,
    ) -> Result<(), ReplyError>;
    /// Share a volume
    async fn share(
        &self,
        req: &dyn ShareVolumeInfo,
        ctx: Option<Context>,
    ) -> Result<String, ReplyError>;
    /// Unshare a volume
    async fn unshare(
        &self,
        req: &dyn UnshareVolumeInfo,
        ctx: Option<Context>,
    ) -> Result<(), ReplyError>;
    /// Publish a volume
    async fn publish(
        &self,
        req: &dyn PublishVolumeInfo,
        ctx: Option<Context>,
    ) -> Result<Volume, ReplyError>;
    /// Republish a volume, by shutting down previous dependents
    async fn republish(
        &self,
        req: &dyn RepublishVolumeInfo,
        ctx: Option<Context>,
    ) -> Result<Volume, ReplyError>;
    /// Unpublish a volume
    async fn unpublish(
        &self,
        req: &dyn UnpublishVolumeInfo,
        ctx: Option<Context>,
    ) -> Result<Volume, ReplyError>;
    /// Increase or decrease volume replica
    async fn set_replica(
        &self,
        req: &dyn SetVolumeReplicaInfo,
        ctx: Option<Context>,
    ) -> Result<Volume, ReplyError>;
    /// Set volume property.
    async fn set_property(
        &self,
        req: &dyn SetVolumePropertyInfo,
        ctx: Option<Context>,
    ) -> Result<Volume, ReplyError>;
    /// Liveness probe for volume service
    async fn probe(&self, ctx: Option<Context>) -> Result<bool, ReplyError>;
    /// Destroy shutdown targets
    async fn destroy_shutdown_target(
        &self,
        req: &dyn DestroyShutdownTargetsInfo,
        ctx: Option<Context>,
    ) -> Result<(), ReplyError>;
    /// Create a volume snapshot.
    async fn create_snapshot(
        &self,
        request: &dyn CreateVolumeSnapshotInfo,
        ctx: Option<Context>,
    ) -> Result<VolumeSnapshot, ReplyError>;
    /// Delete a volume snapshot.
    async fn destroy_snapshot(
        &self,
        request: &dyn DestroyVolumeSnapshotInfo,
        ctx: Option<Context>,
    ) -> Result<(), ReplyError>;
    /// List volume snapshots.
    async fn get_snapshots(
        &self,
        filter: Filter,
        ignore_notfound: bool,
        pagination: Option<Pagination>,
        ctx: Option<Context>,
    ) -> Result<VolumeSnapshots, ReplyError>;
    /// Create a new volume from a volume snapshot source.
    async fn create_snapshot_volume(
        &self,
        req: &dyn CreateSnapshotVolumeInfo,
        ctx: Option<Context>,
    ) -> Result<Volume, ReplyError>;
    /// Resize a volume
    async fn resize(
        &self,
        req: &dyn ResizeVolumeInfo,
        ctx: Option<Context>,
    ) -> Result<Volume, ReplyError>;
}

impl From<VolumeSpec> for volume::VolumeDefinition {
    fn from(volume_spec: VolumeSpec) -> Self {
        let nexus_id = volume_spec.health_info_id().cloned();
        let target = volume_spec.active_config().into_opt();
        let target_config = volume_spec.config().clone().into_opt();
        let as_thin = volume_spec.snapshot_as_thin();
        let spec_status: common::SpecStatus = volume_spec.status.into();

        Self {
            spec: Some(volume::VolumeSpec {
                uuid: Some(volume_spec.uuid.to_string()),
                size: volume_spec.size,
                labels: volume_spec
                    .labels
                    .map(|labels| crate::common::StringMapValue { value: labels }),
                num_replicas: volume_spec.num_replicas.into(),
                target,
                policy: Some(volume_spec.policy.into()),
                topology: volume_spec.topology.map(|topology| topology.into()),
                last_nexus_id: nexus_id.map(|id| id.to_string()),
                thin: volume_spec.thin,
                affinity_group: volume_spec.affinity_group.into_opt(),
                content_source: volume_spec.content_source.into_opt(),
                num_snapshots: volume_spec.metadata.num_snapshots() as u32,
                max_snapshots: volume_spec.max_snapshots,
            }),
            metadata: Some(volume::Metadata {
                spec_status: spec_status as i32,
                target_config,
                publish_context: volume_spec
                    .publish_context
                    .map(|map| common::MapWrapper { map }),
                as_thin,
            }),
        }
    }
}

impl From<Volume> for volume::Volume {
    fn from(volume: Volume) -> Self {
        let volume_definition = volume.spec().into();
        let status: nexus::NexusStatus = volume.state().status.into();

        let volume_state = volume::VolumeState {
            uuid: Some(volume.state().uuid.to_string()),
            size: volume.state().size,
            status: status as i32,
            target: volume.state().target.map(|target| target.into()),
            replica_topology: to_grpc_replica_topology_map(volume.state().replica_topology),
            usage: volume.state().usage.into_opt(),
        };
        volume::Volume {
            definition: Some(volume_definition),
            state: Some(volume_state),
        }
    }
}

impl From<volume::VolumeUsage> for VolumeUsage {
    fn from(value: volume::VolumeUsage) -> Self {
        Self::new(
            value.capacity,
            value.allocated,
            value.allocated_replica,
            value.allocated_snapshots,
            value.allocated_all_snapshots,
            value.total_allocated,
            value.total_allocated_replicas,
            value.total_allocated_snapshots,
        )
    }
}
impl From<VolumeUsage> for volume::VolumeUsage {
    fn from(value: VolumeUsage) -> Self {
        Self {
            capacity: value.capacity(),
            allocated: value.allocated(),
            total_allocated: value.total_allocated(),
            allocated_replica: value.allocated_replica(),
            allocated_snapshots: value.allocated_snapshots(),
            allocated_all_snapshots: value.allocated_all_snapshots(),
            total_allocated_replicas: value.total_allocated_replicas(),
            total_allocated_snapshots: value.total_allocated_snapshots(),
        }
    }
}
impl From<volume::FrontendNode> for InitiatorAC {
    fn from(value: volume::FrontendNode) -> Self {
        InitiatorAC::new(
            value.name,
            NvmeNqn::try_from(&value.nqn).unwrap_or(NvmeNqn::Invalid { nqn: value.nqn }),
        )
    }
}
impl From<volume::FrontendConfig> for FrontendConfig {
    fn from(value: volume::FrontendConfig) -> Self {
        FrontendConfig::from_acls(value.nodes.into_vec())
    }
}

impl TryFrom<volume::VolumeDefinition> for VolumeSpec {
    type Error = ReplyError;

    fn try_from(volume_definition: volume::VolumeDefinition) -> Result<Self, Self::Error> {
        let volume_spec = match volume_definition.spec {
            Some(spec) => spec,
            None => {
                return Err(ReplyError::missing_argument(
                    ResourceKind::Volume,
                    "volume.definition.spec",
                ))
            }
        };
        let volume_meta = match volume_definition.metadata {
            Some(meta) => meta,
            None => {
                return Err(ReplyError::missing_argument(
                    ResourceKind::Volume,
                    "volume.definition.metadata",
                ))
            }
        };

        let volume_spec_status = match common::SpecStatus::try_from(volume_meta.spec_status) {
            Ok(status) => status.into(),
            Err(error) => {
                return Err(ReplyError::invalid_argument(
                    ResourceKind::Volume,
                    "volume.metadata.spec_status",
                    error.to_string(),
                ))
            }
        };
        let volume_spec = VolumeSpec {
            uuid: VolumeId::try_from(StringValue(volume_spec.uuid))?,
            size: volume_spec.size,
            labels: match volume_spec.labels {
                Some(labels) => Some(labels.value),
                None => None,
            },
            num_replicas: volume_spec.num_replicas as u8,
            status: volume_spec_status,
            target_config: match volume_spec.target {
                Some(target) => {
                    let target = VolumeTarget::try_from(target).map_err(|err| {
                        ReplyError::invalid_argument(
                            ResourceKind::Volume,
                            "volume.definition.spec.target",
                            err.to_string(),
                        )
                    })?;
                    let frontend = volume_meta
                        .target_config
                        .and_then(|c| c.frontend)
                        .unwrap_or_default();

                    Some(TargetConfig::new(
                        target,
                        NexusNvmfConfig::default(),
                        frontend.into(),
                    ))
                }
                None => None,
            },
            policy: match volume_spec.policy {
                Some(policy) => policy.into(),
                None => {
                    return Err(ReplyError::missing_argument(
                        ResourceKind::Volume,
                        "volume.definition.spec.policy",
                    ))
                }
            },
            topology: match volume_spec.topology {
                Some(topology) => match Topology::try_from(topology) {
                    Ok(topology) => Some(topology),
                    Err(err) => {
                        return Err(ReplyError::invalid_argument(
                            ResourceKind::Volume,
                            "volume.definition.spec.topology",
                            err.to_string(),
                        ))
                    }
                },
                None => None,
            },
            sequencer: Default::default(),
            last_nexus_id: match volume_spec.last_nexus_id {
                Some(id) => match NexusId::try_from(id) {
                    Ok(volume_id) => Some(volume_id),
                    Err(err) => {
                        return Err(ReplyError::invalid_argument(
                            ResourceKind::Volume,
                            "volume.definition.spec.last_nexus_id",
                            err.to_string(),
                        ))
                    }
                },
                None => None,
            },
            operation: None,
            thin: volume_spec.thin,
            publish_context: volume_meta
                .publish_context
                .map(|map_wrapper| map_wrapper.map),
            affinity_group: volume_spec.affinity_group.into_opt(),
            metadata: VolumeMetadata::new(volume_meta.as_thin),
            content_source: volume_spec.content_source.try_into_opt()?,
            num_snapshots: volume_spec.num_snapshots,
            max_snapshots: volume_spec.max_snapshots,
        };
        Ok(volume_spec)
    }
}

impl TryFrom<volume::Volume> for Volume {
    type Error = ReplyError;
    fn try_from(from: volume::Volume) -> Result<Self, Self::Error> {
        let definition = match from.definition {
            Some(definition) => definition,
            None => {
                return Err(ReplyError::missing_argument(
                    ResourceKind::Volume,
                    "volume.definition",
                ))
            }
        };
        let volume_spec = VolumeSpec::try_from(definition)?;
        let grpc_state = match from.state {
            Some(state) => state,
            None => {
                return Err(ReplyError::missing_argument(
                    ResourceKind::Volume,
                    "volume.state",
                ))
            }
        };
        let volume_state = VolumeState {
            uuid: VolumeId::try_from(StringValue(grpc_state.uuid))?,
            size: grpc_state.size,
            status: match nexus::NexusStatus::try_from(grpc_state.status) {
                Ok(status) => status.into(),
                Err(error) => {
                    return Err(ReplyError::invalid_argument(
                        ResourceKind::Volume,
                        "volume.state.status",
                        error,
                    ))
                }
            },
            target: match grpc_state.target {
                Some(target) => match Nexus::try_from(target) {
                    Ok(target) => Some(target),
                    Err(err) => {
                        return Err(ReplyError::invalid_argument(
                            ResourceKind::Volume,
                            "volume.state.target",
                            err.to_string(),
                        ))
                    }
                },
                None => None,
            },
            replica_topology: match to_replica_topology_map(grpc_state.replica_topology) {
                Ok(replica_topology_map) => replica_topology_map,
                Err(err) => {
                    return Err(ReplyError::invalid_argument(
                        ResourceKind::Volume,
                        "volume.state.replica_topology",
                        err.to_string(),
                    ))
                }
            },
            usage: grpc_state.usage.into_opt(),
        };
        Ok(Volume::new(volume_spec, volume_state))
    }
}

impl TryFrom<volume::Volumes> for Volumes {
    type Error = ReplyError;
    fn try_from(grpc_volumes: volume::Volumes) -> Result<Self, Self::Error> {
        let mut volumes: Vec<Volume> = Vec::with_capacity(grpc_volumes.entries.len());
        for volume in grpc_volumes.entries {
            volumes.push(Volume::try_from(volume)?)
        }
        Ok(Volumes {
            entries: volumes,
            next_token: grpc_volumes.next_token,
        })
    }
}

impl From<Volumes> for volume::Volumes {
    fn from(volumes: Volumes) -> Self {
        volume::Volumes {
            entries: volumes
                .entries
                .iter()
                .map(|volume| volume.clone().into())
                .collect(),
            next_token: volumes.next_token,
        }
    }
}

impl TryFrom<volume::ReplicaTopology> for ReplicaTopology {
    type Error = ReplyError;
    fn try_from(topology: volume::ReplicaTopology) -> Result<Self, Self::Error> {
        let node = topology.node.map(|node| node.into());
        let pool = topology.pool.map(|pool| pool.into());
        #[allow(clippy::unnecessary_fallible_conversions)]
        let status = match ReplicaStatus::try_from(topology.status) {
            Ok(status) => status,
            Err(err) => {
                return Err(ReplyError::invalid_argument(
                    ResourceKind::Volume,
                    "replica_topology.status",
                    err.to_string(),
                ))
            }
        };
        Ok(ReplicaTopology::new(
            node,
            pool,
            status,
            topology.usage.into_opt(),
            topology
                .child_status
                .and_then(|s| nexus::ChildState::try_from(s).ok().into_opt()),
            topology
                .child_status_reason
                .and_then(|s| nexus::ChildStateReason::try_from(s).ok().into_opt()),
            topology.rebuild_progress.and_then(|r| u8::try_from(r).ok()),
        ))
    }
}

impl From<volume::ReplicaUsage> for ReplicaUsage {
    fn from(value: volume::ReplicaUsage) -> Self {
        Self::new(
            value.capacity,
            value.allocated,
            value.allocated_snaps,
            value.allocated_all_snaps,
        )
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
            usage: replica_topology.usage().into_opt(),
            child_status: replica_topology
                .child_status()
                .map(|s| crate::nexus::ChildState::from(s).into()),
            child_status_reason: replica_topology
                .child_status_reason()
                .map(|s| crate::nexus::ChildStateReason::from(s).into()),
            rebuild_progress: replica_topology.rebuild_progress().map(|r| r as u32),
        }
    }
}

impl From<&ReplicaUsage> for volume::ReplicaUsage {
    fn from(value: &ReplicaUsage) -> Self {
        Self {
            capacity: value.capacity(),
            allocated: value.allocated(),
            allocated_snaps: value.allocated_snapshots(),
            allocated_all_snaps: value.allocated_all_snapshots(),
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
                    Err(err) => {
                        return Err(ReplyError::invalid_argument(
                            ResourceKind::Volume,
                            "topology.node_topology",
                            err.to_string(),
                        ))
                    }
                },
                None => None,
            },
            pool: match topology_grpc_type.pool {
                Some(pool_topo) => match PoolTopology::try_from(pool_topo) {
                    Ok(pool_topo) => Some(pool_topo),
                    Err(err) => {
                        return Err(ReplyError::invalid_argument(
                            ResourceKind::Volume,
                            "topology.pool_topology",
                            err.to_string(),
                        ))
                    }
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
            None => {
                return Err(ReplyError::missing_argument(
                    ResourceKind::Volume,
                    "node_topology.topology",
                ))
            }
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
            None => {
                return Err(ReplyError::missing_argument(
                    ResourceKind::Volume,
                    "pool_topology.topology",
                ))
            }
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
            creation_affinity_keys: Some(Vec::new()),
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
    fn try_from(target: volume::VolumeTarget) -> Result<Self, Self::Error> {
        let target = VolumeTarget::new(
            target.node_id.into(),
            NexusId::try_from(StringValue(target.nexus_id))?,
            match target.protocol {
                Some(i) => match volume::VolumeShareProtocol::try_from(i) {
                    Ok(protocol) => Some(protocol.into()),
                    Err(error) => {
                        return Err(ReplyError::invalid_argument(
                            ResourceKind::Volume,
                            "target.protocol",
                            error,
                        ))
                    }
                },
                None => None,
            },
        );
        Ok(target)
    }
}
impl From<&TargetConfig> for volume::VolumeTarget {
    fn from(config: &TargetConfig) -> Self {
        let target = config.target();
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

impl TryFrom<volume::TargetConfig> for TargetConfig {
    type Error = ReplyError;
    fn try_from(src: volume::TargetConfig) -> Result<Self, Self::Error> {
        Ok(Self::new(
            match src.target {
                Some(t) => t.try_into(),
                None => Err(ReplyError::missing_argument(
                    ResourceKind::Volume,
                    "target_config.target",
                )),
            }?,
            match src.config {
                Some(c) => c.try_into(),
                None => Err(ReplyError::missing_argument(
                    ResourceKind::Volume,
                    "target_config.config",
                )),
            }?,
            FrontendConfig::default(),
        ))
    }
}
impl From<TargetConfig> for volume::TargetConfig {
    fn from(src: TargetConfig) -> Self {
        volume::TargetConfig {
            target: Some((&src).into()),
            config: Some(src.config().clone().into()),
            frontend: Some(src.frontend().into()),
        }
    }
}
impl From<&FrontendConfig> for volume::FrontendConfig {
    fn from(src: &FrontendConfig) -> Self {
        volume::FrontendConfig {
            nodes: src
                .nodes_info()
                .iter()
                .map(|n| volume::FrontendNode {
                    name: n.node_name().to_string(),
                    nqn: n.node_nqn().to_string(),
                })
                .collect::<Vec<_>>(),
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
            get_volumes_request::Filter::Volume(volume_filter) => Filter::Volume(
                VolumeId::try_from(StringValue(Some(volume_filter.volume_id)))?,
            ),
        })
    }
}

/// Trait to be implemented for CreateVolume from Snapshot operation.
pub trait CreateSnapshotVolumeInfo: Send + Sync + std::fmt::Debug {
    /// Get the restore source snapshot uuid.
    fn source_snapshot(&self) -> &SnapshotId;
    /// Get the generic volume create parameters.
    fn volume(&self) -> &dyn CreateVolumeInfo;
}

impl CreateSnapshotVolumeInfo for CreateSnapshotVolume {
    fn source_snapshot(&self) -> &SnapshotId {
        self.snapshot_uuid()
    }

    fn volume(&self) -> &dyn CreateVolumeInfo {
        self.params()
    }
}

impl CreateSnapshotVolumeInfo for ValidatedCreateSnapshotVolumeRequest {
    fn source_snapshot(&self) -> &SnapshotId {
        &self.snapshot_id
    }
    fn volume(&self) -> &dyn CreateVolumeInfo {
        &self.inner
    }
}
impl ValidateRequestTypes for CreateSnapshotVolumeRequest {
    type Validated = ValidatedCreateSnapshotVolumeRequest;
    fn validated(self) -> Result<Self::Validated, ReplyError> {
        let Some(volume) = self.volume else {
            return Err(ReplyError::missing_argument(ResourceKind::Volume, "volume"));
        };

        Ok(ValidatedCreateSnapshotVolumeRequest {
            snapshot_id: match &self.source_snapshot {
                Some(id) => SnapshotId::try_from(id.as_str()).map_err(|e| {
                    ReplyError::invalid_argument(ResourceKind::VolumeSnapshot, "source_snapshot", e)
                }),
                None => Err(ReplyError::missing_argument(
                    ResourceKind::Volume,
                    "source_snapshot",
                )),
            }?,
            inner: volume.validated()?,
        })
    }
}

/// Intermediate structure that validates the conversion to CreateVolumeRequest type.
#[derive(Debug)]
pub struct ValidatedCreateSnapshotVolumeRequest {
    snapshot_id: SnapshotId,
    inner: ValidatedCreateVolumeRequest,
}

/// Trait to be implemented for CreateVolume operation.
pub trait CreateVolumeInfo: Send + Sync + std::fmt::Debug {
    /// Uuid of the volume
    fn uuid(&self) -> VolumeId;
    /// Size in bytes of the volume
    fn size(&self) -> u64;
    /// No of replicas of the volume
    fn replicas(&self) -> u64;
    /// Volume policy of the volume, i.e self_heal
    fn policy(&self) -> VolumePolicy;
    /// Topology configuration of the volume
    fn topology(&self) -> Option<Topology>;
    /// Labels to be added to the volumes for topology based scheduling
    fn labels(&self) -> Option<VolumeLabels>;
    /// Flag indicating whether the volume should be thin provisioned
    fn thin(&self) -> bool;
    /// Affinity Group related information.
    fn affinity_group(&self) -> Option<AffinityGroup>;
    /// Capacity Limit.
    fn cluster_capacity_limit(&self) -> Option<u64>;
    /// Max snapshot limit per volume.
    fn max_snapshots(&self) -> Option<u32>;
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

    fn thin(&self) -> bool {
        self.thin
    }

    fn affinity_group(&self) -> Option<AffinityGroup> {
        self.affinity_group.clone()
    }

    fn cluster_capacity_limit(&self) -> Option<u64> {
        self.cluster_capacity_limit
    }

    fn max_snapshots(&self) -> Option<u32> {
        self.max_snapshots
    }
}

/// Intermediate structure that validates the conversion to CreateVolumeRequest type.
#[derive(Debug)]
pub struct ValidatedCreateVolumeRequest {
    inner: CreateVolumeRequest,
    uuid: VolumeId,
    topology: Option<Topology>,
}

impl CreateVolumeInfo for ValidatedCreateVolumeRequest {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }

    fn size(&self) -> u64 {
        self.inner.size
    }

    fn replicas(&self) -> u64 {
        self.inner.replicas
    }

    fn policy(&self) -> VolumePolicy {
        match self.inner.policy {
            Some(policy) => policy.into(),
            None => VolumePolicy::default(),
        }
    }

    fn topology(&self) -> Option<Topology> {
        self.topology.clone()
    }

    fn labels(&self) -> Option<VolumeLabels> {
        match self.inner.labels.clone() {
            None => None,
            Some(labels) => Some(labels.value),
        }
    }

    fn thin(&self) -> bool {
        self.inner.thin
    }

    fn affinity_group(&self) -> Option<AffinityGroup> {
        self.inner.affinity_group.clone().map(|ag| ag.into())
    }

    fn cluster_capacity_limit(&self) -> Option<u64> {
        self.inner.cluster_capacity_limit
    }

    fn max_snapshots(&self) -> Option<u32> {
        self.inner.max_snapshots
    }
}

impl ValidateRequestTypes for CreateVolumeRequest {
    type Validated = ValidatedCreateVolumeRequest;
    fn validated(self) -> Result<Self::Validated, ReplyError> {
        Ok(ValidatedCreateVolumeRequest {
            uuid: VolumeId::try_from(StringValue(self.uuid.clone()))?,
            topology: match self.topology.clone() {
                Some(topology) => match Topology::try_from(topology) {
                    Ok(topology) => Some(topology),
                    Err(err) => {
                        return Err(ReplyError::invalid_argument(
                            ResourceKind::Volume,
                            "create_volume_request.topology",
                            err.to_string(),
                        ))
                    }
                },
                None => None,
            },
            inner: self,
        })
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
            thin: data.thin(),
            affinity_group: data.affinity_group(),
            cluster_capacity_limit: data.cluster_capacity_limit(),
            max_snapshots: data.max_snapshots(),
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
            thin: data.thin(),
            affinity_group: data.affinity_group().map(|ag| ag.into()),
            cluster_capacity_limit: data.cluster_capacity_limit(),
            max_snapshots: data.max_snapshots(),
        }
    }
}

impl From<&dyn CreateSnapshotVolumeInfo> for CreateSnapshotVolume {
    fn from(data: &dyn CreateSnapshotVolumeInfo) -> Self {
        Self::new(data.source_snapshot().clone(), data.volume().into())
    }
}

impl From<&dyn CreateSnapshotVolumeInfo> for CreateSnapshotVolumeRequest {
    fn from(data: &dyn CreateSnapshotVolumeInfo) -> Self {
        Self {
            source_snapshot: Some(data.source_snapshot().to_string()),
            volume: Some(data.volume().into()),
        }
    }
}

/// Trait to be implemented for DestroyVolume operation.
pub trait DestroyVolumeInfo: Send + Sync + std::fmt::Debug {
    /// Uuid of the volume to be destroyed
    fn uuid(&self) -> VolumeId;
}

impl DestroyVolumeInfo for DestroyVolume {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }
}
impl DestroyVolumeInfo for Volume {
    fn uuid(&self) -> VolumeId {
        self.spec().uuid
    }
}

/// Intermediate structure that validates the conversion to DestroyVolumeRequest type.
#[derive(Debug)]
pub struct ValidatedDestroyVolumeRequest {
    uuid: VolumeId,
}

impl DestroyVolumeInfo for ValidatedDestroyVolumeRequest {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }
}

impl ValidateRequestTypes for DestroyVolumeRequest {
    type Validated = ValidatedDestroyVolumeRequest;
    fn validated(self) -> Result<Self::Validated, ReplyError> {
        Ok(ValidatedDestroyVolumeRequest {
            uuid: VolumeId::try_from(StringValue(self.uuid))?,
        })
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

/// Intermediate structure that validates the conversion to ResizeVolumeRequest type.
#[derive(Debug)]
pub struct ValidatedResizeVolumeRequest {
    uuid: VolumeId,
    requested_size: u64,
    cluster_capacity_limit: Option<u64>,
}
/// Trait to be implemented for ResizeVolume operation.
pub trait ResizeVolumeInfo: Send + Sync + std::fmt::Debug {
    /// Uuid of the volume to be resized
    fn uuid(&self) -> VolumeId;
    /// Requested new size of the volume, in bytes
    fn req_size(&self) -> u64;
    /// Total capacity limit for all volumes, in bytes
    fn cluster_capacity_limit(&self) -> Option<u64>;
}

impl ResizeVolumeInfo for ResizeVolume {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }

    fn req_size(&self) -> u64 {
        self.requested_size
    }

    fn cluster_capacity_limit(&self) -> Option<u64> {
        self.cluster_capacity_limit
    }
}

impl ValidateRequestTypes for ResizeVolumeRequest {
    type Validated = ValidatedResizeVolumeRequest;
    fn validated(self) -> Result<Self::Validated, ReplyError> {
        Ok(ValidatedResizeVolumeRequest {
            uuid: VolumeId::try_from(StringValue(Some(self.uuid)))?,
            requested_size: self.requested_size,
            cluster_capacity_limit: self.capacity_limit,
        })
    }
}

impl From<&dyn ResizeVolumeInfo> for ResizeVolume {
    fn from(data: &dyn ResizeVolumeInfo) -> Self {
        Self {
            uuid: data.uuid(),
            requested_size: data.req_size(),
            cluster_capacity_limit: data.cluster_capacity_limit(),
        }
    }
}

impl From<&dyn ResizeVolumeInfo> for ResizeVolumeRequest {
    fn from(data: &dyn ResizeVolumeInfo) -> Self {
        Self {
            uuid: data.uuid().to_string(),
            requested_size: data.req_size(),
            capacity_limit: data.cluster_capacity_limit(),
        }
    }
}

impl ResizeVolumeInfo for ValidatedResizeVolumeRequest {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }

    fn req_size(&self) -> u64 {
        self.requested_size
    }

    fn cluster_capacity_limit(&self) -> Option<u64> {
        self.cluster_capacity_limit
    }
}

/// Trait to be implemented for ShareVolume operation.
pub trait ShareVolumeInfo: Send + Sync + std::fmt::Debug {
    /// Uuid of the volume to be shared
    fn uuid(&self) -> VolumeId;
    /// Protocol over which the volume be shared
    fn share(&self) -> VolumeShareProtocol;
    /// Hosts allowed to access nexus.
    fn frontend_hosts(&self) -> Vec<String>;
}

impl ShareVolumeInfo for ShareVolume {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }

    fn share(&self) -> VolumeShareProtocol {
        self.protocol
    }

    fn frontend_hosts(&self) -> Vec<String> {
        self.frontend_hosts.clone()
    }
}

/// Intermediate structure that validates the conversion to ShareVolumeRequest type.
#[derive(Debug)]
pub struct ValidatedShareVolumeRequest {
    uuid: VolumeId,
    share: VolumeShareProtocol,
    frontend_hosts: Vec<String>,
}

impl ShareVolumeInfo for ValidatedShareVolumeRequest {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }

    fn share(&self) -> VolumeShareProtocol {
        self.share
    }

    fn frontend_hosts(&self) -> Vec<String> {
        self.frontend_hosts.clone()
    }
}

impl ValidateRequestTypes for ShareVolumeRequest {
    type Validated = ValidatedShareVolumeRequest;
    fn validated(self) -> Result<Self::Validated, ReplyError> {
        Ok(ValidatedShareVolumeRequest {
            uuid: VolumeId::try_from(StringValue(self.uuid))?,
            share: match volume::VolumeShareProtocol::try_from(self.share) {
                Ok(share) => share.into(),
                Err(error) => {
                    return Err(ReplyError::invalid_argument(
                        ResourceKind::Volume,
                        "share_volume_request.share",
                        error,
                    ))
                }
            },
            frontend_hosts: self.frontend_hosts,
        })
    }
}

impl From<&dyn ShareVolumeInfo> for ShareVolume {
    fn from(data: &dyn ShareVolumeInfo) -> Self {
        Self {
            uuid: data.uuid(),
            protocol: data.share(),
            frontend_hosts: data.frontend_hosts(),
        }
    }
}

impl From<&dyn ShareVolumeInfo> for ShareVolumeRequest {
    fn from(data: &dyn ShareVolumeInfo) -> Self {
        let share: volume::VolumeShareProtocol = data.share().into();
        Self {
            uuid: Some(data.uuid().to_string()),
            share: share as i32,
            frontend_hosts: data.frontend_hosts(),
        }
    }
}

/// Trait to be implemented for UnshareVolume operation.
pub trait UnshareVolumeInfo: Send + Sync + std::fmt::Debug {
    /// Uuid of the volume to be unshared
    fn uuid(&self) -> VolumeId;
}

impl UnshareVolumeInfo for UnshareVolume {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }
}

/// Intermediate structure that validates the conversion to UnshareVolumeRequest type.
#[derive(Debug)]
pub struct ValidatedUnshareVolumeRequest {
    uuid: VolumeId,
}

impl UnshareVolumeInfo for ValidatedUnshareVolumeRequest {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }
}

impl ValidateRequestTypes for UnshareVolumeRequest {
    type Validated = ValidatedUnshareVolumeRequest;
    fn validated(self) -> Result<Self::Validated, ReplyError> {
        Ok(ValidatedUnshareVolumeRequest {
            uuid: VolumeId::try_from(StringValue(self.uuid))?,
        })
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

/// Trait to be implemented for PublishVolume operation
pub trait PublishVolumeInfo: Send + Sync + std::fmt::Debug {
    /// Uuid of the volume to be published
    fn uuid(&self) -> VolumeId;
    /// The node where front-end IO will be sent to
    fn target_node(&self) -> Option<NodeId>;
    /// The protocol over which volume be published
    fn share(&self) -> Option<VolumeShareProtocol>;
    /// The Publish context
    fn publish_context(&self) -> HashMap<String, String>;
    /// Hosts allowed to access the nexus.
    fn frontend_nodes(&self) -> Vec<String>;
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

    fn publish_context(&self) -> HashMap<String, String> {
        self.publish_context.clone()
    }

    fn frontend_nodes(&self) -> Vec<String> {
        self.frontend_nodes.clone()
    }
}

impl PublishVolumeInfo for RepublishVolume {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }

    fn target_node(&self) -> Option<NodeId> {
        self.target_node.clone()
    }

    fn share(&self) -> Option<VolumeShareProtocol> {
        Some(self.share)
    }

    fn publish_context(&self) -> HashMap<String, String> {
        unimplemented!()
    }

    fn frontend_nodes(&self) -> Vec<String> {
        unimplemented!()
    }
}

/// Intermediate structure that validates the conversion to PublishVolumeRequest type.
#[derive(Debug)]
pub struct ValidatedPublishVolumeRequest {
    inner: PublishVolumeRequest,
    uuid: VolumeId,
    share: Option<VolumeShareProtocol>,
    frontend_nodes: Vec<String>,
}

impl PublishVolumeInfo for ValidatedPublishVolumeRequest {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }

    fn target_node(&self) -> Option<NodeId> {
        self.inner
            .target_node
            .clone()
            .map(|target_node| target_node.into())
    }

    fn share(&self) -> Option<VolumeShareProtocol> {
        self.share
    }

    fn publish_context(&self) -> HashMap<String, String> {
        self.inner.publish_context.clone()
    }

    fn frontend_nodes(&self) -> Vec<String> {
        self.frontend_nodes.clone()
    }
}

impl ValidateRequestTypes for PublishVolumeRequest {
    type Validated = ValidatedPublishVolumeRequest;
    fn validated(self) -> Result<Self::Validated, ReplyError> {
        Ok(ValidatedPublishVolumeRequest {
            uuid: VolumeId::try_from(StringValue(self.uuid.clone()))?,
            share: match self.share {
                Some(share) => match volume::VolumeShareProtocol::try_from(share) {
                    Ok(share) => Some(share.into()),
                    Err(error) => {
                        return Err(ReplyError::invalid_argument(
                            ResourceKind::Volume,
                            "publish_volume_request.share",
                            error,
                        ))
                    }
                },
                None => None,
            },
            inner: self.clone(),
            frontend_nodes: self.frontend_nodes,
        })
    }
}

impl From<&dyn PublishVolumeInfo> for PublishVolume {
    fn from(data: &dyn PublishVolumeInfo) -> Self {
        Self {
            uuid: data.uuid(),
            target_node: data.target_node(),
            share: data.share(),
            publish_context: data.publish_context(),
            frontend_nodes: data.frontend_nodes(),
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
            publish_context: data.publish_context(),
            frontend_nodes: data.frontend_nodes(),
        }
    }
}

/// Trait to be implemented for Republish operation.
pub trait RepublishVolumeInfo: Send + Sync + std::fmt::Debug {
    /// Uuid of the volume to be published.
    fn uuid(&self) -> VolumeId;
    /// The node where front-end IO will be sent to.
    fn target_node(&self) -> Option<NodeId>;
    /// The nodename where front-end IO will be sent from.
    fn frontend_node(&self) -> NodeId;
    /// The protocol over which volume be published.
    fn share(&self) -> VolumeShareProtocol;
    /// Republish reusing current target.
    fn reuse_existing(&self) -> bool;
    /// Allows reusing the existing target, but prefers a target move.
    fn reuse_existing_fallback(&self) -> bool;
}

impl RepublishVolumeInfo for RepublishVolume {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }

    fn target_node(&self) -> Option<NodeId> {
        self.target_node.clone()
    }

    fn frontend_node(&self) -> NodeId {
        self.frontend_node.clone()
    }

    fn share(&self) -> VolumeShareProtocol {
        self.share
    }

    fn reuse_existing(&self) -> bool {
        self.reuse_existing
    }

    fn reuse_existing_fallback(&self) -> bool {
        self.reuse_existing_fallback
    }
}

impl From<&dyn RepublishVolumeInfo> for RepublishVolume {
    fn from(data: &dyn RepublishVolumeInfo) -> Self {
        Self {
            uuid: data.uuid(),
            target_node: data.target_node(),
            frontend_node: data.frontend_node(),
            share: data.share(),
            reuse_existing: data.reuse_existing(),
            reuse_existing_fallback: data.reuse_existing_fallback(),
        }
    }
}

impl From<&dyn RepublishVolumeInfo> for RepublishVolumeRequest {
    fn from(data: &dyn RepublishVolumeInfo) -> Self {
        let protocol: volume::VolumeShareProtocol = data.share().into();
        Self {
            uuid: Some(data.uuid().to_string()),
            target_node: data.target_node().map(|node_id| node_id.to_string()),
            share: protocol as i32,
            reuse_existing: data.reuse_existing(),
            frontend_node: data.frontend_node().to_string(),
            reuse_existing_fallback: data.reuse_existing_fallback(),
        }
    }
}

/// Intermediate structure that validates the conversion to RepublishVolumeRequest type.
#[derive(Debug)]
pub struct ValidatedRepublishVolumeRequest {
    inner: RepublishVolumeRequest,
    uuid: VolumeId,
    share: VolumeShareProtocol,
}

impl RepublishVolumeInfo for ValidatedRepublishVolumeRequest {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }

    fn target_node(&self) -> Option<NodeId> {
        self.inner
            .target_node
            .clone()
            .map(|target_node| target_node.into())
    }

    fn frontend_node(&self) -> NodeId {
        NodeId::from(&self.inner.frontend_node)
    }

    fn share(&self) -> VolumeShareProtocol {
        self.share
    }

    fn reuse_existing(&self) -> bool {
        self.inner.reuse_existing
    }

    fn reuse_existing_fallback(&self) -> bool {
        self.inner.reuse_existing_fallback
    }
}

impl ValidateRequestTypes for RepublishVolumeRequest {
    type Validated = ValidatedRepublishVolumeRequest;
    fn validated(self) -> Result<Self::Validated, ReplyError> {
        Ok(ValidatedRepublishVolumeRequest {
            uuid: VolumeId::try_from(StringValue(self.uuid.clone()))?,
            share: match volume::VolumeShareProtocol::try_from(self.share) {
                Ok(share) => share.into(),
                Err(error) => {
                    return Err(ReplyError::invalid_argument(
                        ResourceKind::Volume,
                        "republish_volume_request.share",
                        error,
                    ))
                }
            },
            inner: self,
        })
    }
}

/// Trait to be implemented for UnpublishVolume operation.
pub trait UnpublishVolumeInfo: Send + Sync + std::fmt::Debug {
    /// Uuid of the volume to unpublish
    fn uuid(&self) -> VolumeId;
    /// Force unpublish
    fn force(&self) -> bool;
    /// Frontend node requesting unpublish.
    fn frontend_nodes(&self) -> Vec<String>;
}

impl UnpublishVolumeInfo for UnpublishVolume {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }

    fn force(&self) -> bool {
        self.force()
    }

    fn frontend_nodes(&self) -> Vec<String> {
        self.frontend_nodes.clone()
    }
}

/// Intermediate structure that validates the conversion to UnpublishVolumeRequest type.
#[derive(Debug)]
pub struct ValidatedUnpublishVolumeRequest {
    inner: UnpublishVolumeRequest,
    uuid: VolumeId,
}

impl UnpublishVolumeInfo for ValidatedUnpublishVolumeRequest {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }
    fn force(&self) -> bool {
        self.inner.force
    }

    fn frontend_nodes(&self) -> Vec<String> {
        self.inner.frontend_nodes.clone()
    }
}

impl ValidateRequestTypes for UnpublishVolumeRequest {
    type Validated = ValidatedUnpublishVolumeRequest;
    fn validated(self) -> Result<Self::Validated, ReplyError> {
        Ok(ValidatedUnpublishVolumeRequest {
            uuid: VolumeId::try_from(StringValue(self.uuid.clone()))?,
            inner: self,
        })
    }
}

impl From<&dyn UnpublishVolumeInfo> for UnpublishVolume {
    fn from(data: &dyn UnpublishVolumeInfo) -> Self {
        UnpublishVolume::new(&data.uuid(), data.force(), data.frontend_nodes())
    }
}

impl From<&dyn UnpublishVolumeInfo> for UnpublishVolumeRequest {
    fn from(data: &dyn UnpublishVolumeInfo) -> Self {
        Self {
            uuid: Some(data.uuid().to_string()),
            force: data.force(),
            frontend_nodes: data.frontend_nodes().iter().map(|n| n.into()).collect(),
        }
    }
}

/// Trait to be implemented for SetVolumeReplica operation.
pub trait SetVolumeReplicaInfo: Send + Sync + std::fmt::Debug {
    /// Uuid of the concerned volume
    fn uuid(&self) -> VolumeId;
    /// No of replicas we want to set for the volume
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

/// Intermediate structure that validates the conversion to SetVolumeReplicaRequest type.
#[derive(Debug)]
pub struct ValidatedSetVolumeReplicaRequest {
    inner: SetVolumeReplicaRequest,
    uuid: VolumeId,
}

impl SetVolumeReplicaInfo for ValidatedSetVolumeReplicaRequest {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }
    fn replicas(&self) -> u8 {
        self.inner.replicas as u8
    }
}

impl ValidateRequestTypes for SetVolumeReplicaRequest {
    type Validated = ValidatedSetVolumeReplicaRequest;
    fn validated(self) -> Result<Self::Validated, ReplyError> {
        Ok(ValidatedSetVolumeReplicaRequest {
            uuid: VolumeId::try_from(StringValue(self.uuid.clone()))?,
            inner: self,
        })
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
/// Trait to be implemented for SetVolumeProperty operation.
pub trait SetVolumePropertyInfo: Send + Sync + std::fmt::Debug {
    /// Uuid of the concerned volume.
    fn uuid(&self) -> VolumeId;
    /// Property to be set for the volume.
    fn property(&self) -> Option<VolumeProperty>;
}

impl SetVolumePropertyInfo for SetVolumeProperty {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }

    fn property(&self) -> Option<VolumeProperty> {
        Some(self.property.clone())
    }
}

/// Intermediate structure that validates the conversion to SetVolumePropertyRequest type.
#[derive(Debug)]
pub struct ValidatedSetVolumePropertyRequest {
    inner: SetVolumePropertyRequest,
    uuid: VolumeId,
}

impl SetVolumePropertyInfo for ValidatedSetVolumePropertyRequest {
    fn uuid(&self) -> VolumeId {
        self.uuid.clone()
    }
    fn property(&self) -> Option<VolumeProperty> {
        self.inner.clone().into()
    }
}

impl From<SetVolumePropertyRequest> for Option<VolumeProperty> {
    fn from(req: SetVolumePropertyRequest) -> Self {
        req.property.and_then(|property| {
            property.attr.map(|attr| match attr {
                volume::volume_property::Attr::MaxSnapshots(volume::MaxSnapshotValue { value }) => {
                    VolumeProperty::MaxSnapshots(value)
                }
            })
        })
    }
}

impl From<VolumeProperty> for volume::VolumeProperty {
    fn from(req: VolumeProperty) -> Self {
        match req {
            VolumeProperty::MaxSnapshots(value) => volume::VolumeProperty {
                attr: Some(volume::volume_property::Attr::MaxSnapshots(
                    volume::MaxSnapshotValue { value },
                )),
            },
        }
    }
}
impl ValidateRequestTypes for SetVolumePropertyRequest {
    type Validated = ValidatedSetVolumePropertyRequest;
    fn validated(self) -> Result<Self::Validated, ReplyError> {
        Ok(ValidatedSetVolumePropertyRequest {
            uuid: VolumeId::try_from(StringValue(Some(self.uuid.clone())))?,
            inner: self,
        })
    }
}

impl TryFrom<&dyn SetVolumePropertyInfo> for SetVolumeProperty {
    type Error = ReplyError;
    fn try_from(data: &dyn SetVolumePropertyInfo) -> Result<Self, Self::Error> {
        if let Some(property) = data.property() {
            Ok(Self {
                uuid: data.uuid(),
                property,
            })
        } else {
            Err(ReplyError::missing_argument(
                ResourceKind::Volume,
                "set_volume_property_request.property",
            ))
        }
    }
}

impl From<&dyn SetVolumePropertyInfo> for SetVolumePropertyRequest {
    fn from(data: &dyn SetVolumePropertyInfo) -> Self {
        Self {
            uuid: data.uuid().to_string(),
            property: data.property().into_opt(),
        }
    }
}
/// A helper to convert the replica topology map form grpc type to corresponding control plane type.
fn to_replica_topology_map(
    map: HashMap<String, volume::ReplicaTopology>,
) -> Result<HashMap<ReplicaId, ReplicaTopology>, ReplyError> {
    let mut replica_topology_map: HashMap<ReplicaId, ReplicaTopology> = HashMap::new();
    for (k, v) in map {
        let replica_id = ReplicaId::try_from(StringValue(Some(k)))?;
        let replica_topology = match ReplicaTopology::try_from(v) {
            Ok(topology) => topology,
            Err(err) => {
                return Err(ReplyError::invalid_argument(
                    ResourceKind::Volume,
                    "replica_toplogy",
                    err.to_string(),
                ))
            }
        };
        replica_topology_map.insert(replica_id, replica_topology);
    }
    Ok(replica_topology_map)
}

/// A helper to convert the replica topology map form control plane type to corresponding grpc type.
fn to_grpc_replica_topology_map(
    map: HashMap<ReplicaId, ReplicaTopology>,
) -> HashMap<String, volume::ReplicaTopology> {
    let mut replica_topology_map: HashMap<String, volume::ReplicaTopology> = HashMap::new();
    for (k, v) in map {
        replica_topology_map.insert(k.to_string(), v.into());
    }
    replica_topology_map
}

impl TryFrom<StringValue> for VolumeId {
    type Error = ReplyError;

    fn try_from(value: StringValue) -> Result<Self, Self::Error> {
        match value.0 {
            Some(id) => match VolumeId::try_from(id) {
                Ok(volume_id) => Ok(volume_id),
                Err(err) => Err(ReplyError::invalid_argument(
                    ResourceKind::Volume,
                    "volume.definition.spec.uuid",
                    err.to_string(),
                )),
            },
            None => Err(ReplyError::missing_argument(
                ResourceKind::Volume,
                "volume.definition.spec.uuid",
            )),
        }
    }
}

/// DestroyShutdownTargetInfo trait for the shutdown targets deletion to be implemented by entities
/// which want to use this operation.
pub trait DestroyShutdownTargetsInfo: Send + Sync + std::fmt::Debug {
    /// uuid of the volume, i.e the owner
    fn uuid(&self) -> &VolumeId;
    /// List of all targets registered in the Application node for the volume.
    fn registered_targets(&self) -> Option<Vec<String>>;
}

impl DestroyShutdownTargetsInfo for DestroyShutdownTargets {
    fn uuid(&self) -> &VolumeId {
        self.uuid()
    }
    fn registered_targets(&self) -> Option<Vec<String>> {
        self.registered_targets().clone()
    }
}

/// Intermediate structure that validates the conversion to DestroyShutdownNexusRequest type.
#[derive(Debug)]
pub struct ValidatedDestroyShutdownTargetRequest {
    uuid: VolumeId,
    registered_targets: Option<Vec<String>>,
}

impl DestroyShutdownTargetsInfo for ValidatedDestroyShutdownTargetRequest {
    fn uuid(&self) -> &VolumeId {
        self.uuid.borrow()
    }
    fn registered_targets(&self) -> Option<Vec<String>> {
        self.registered_targets.clone()
    }
}

impl ValidateRequestTypes for DestroyShutdownTargetRequest {
    type Validated = ValidatedDestroyShutdownTargetRequest;
    fn validated(self) -> Result<Self::Validated, ReplyError> {
        Ok(ValidatedDestroyShutdownTargetRequest {
            uuid: VolumeId::try_from(StringValue(self.volume_id))?,
            registered_targets: match self.registered_targets {
                Some(val) => Some(val.target_list),
                None => None,
            },
        })
    }
}

impl From<&dyn DestroyShutdownTargetsInfo> for DestroyShutdownTargetRequest {
    fn from(data: &dyn DestroyShutdownTargetsInfo) -> Self {
        Self {
            volume_id: Some(data.uuid().to_string()),
            registered_targets: data
                .registered_targets()
                .map(|val| RegisteredTargets { target_list: val }),
        }
    }
}

impl From<&dyn DestroyShutdownTargetsInfo> for DestroyShutdownTargets {
    fn from(data: &dyn DestroyShutdownTargetsInfo) -> Self {
        Self::new(data.uuid().clone(), data.registered_targets())
    }
}

impl From<volume::AffinityGroup> for AffinityGroup {
    fn from(value: volume::AffinityGroup) -> Self {
        AffinityGroup::new(value.name)
    }
}

impl From<AffinityGroup> for volume::AffinityGroup {
    fn from(value: AffinityGroup) -> Self {
        Self {
            name: value.id().clone(),
        }
    }
}

impl From<AffinityGroupSpec> for volume::AffinityGroupSpec {
    fn from(value: AffinityGroupSpec) -> Self {
        Self {
            id: value.id().clone(),
            volumes: value.volumes().iter().map(|id| id.to_string()).collect(),
        }
    }
}

impl TryFrom<volume::AffinityGroupSpec> for AffinityGroupSpec {
    type Error = ReplyError;

    fn try_from(value: volume::AffinityGroupSpec) -> Result<Self, Self::Error> {
        let mut volumes: Vec<VolumeId> = Vec::with_capacity(value.volumes.len());
        for volume in value.volumes {
            let volume_id = VolumeId::try_from(volume).map_err(|error| {
                ReplyError::invalid_argument(ResourceKind::Volume, "volume_id", error.to_string())
            })?;
            volumes.push(volume_id)
        }
        Ok(AffinityGroupSpec::new(value.id, volumes))
    }
}

impl TryFrom<volume::volume_spec::VolumeContentSource> for VolumeContentSource {
    type Error = ReplyError;

    fn try_from(value: volume::volume_spec::VolumeContentSource) -> Result<Self, Self::Error> {
        match value.volume_content_source {
            None => Err(ReplyError::missing_argument(
                ResourceKind::Volume,
                "volume_content_source",
            )),
            Some(content_source) => match content_source {
                volume::volume_spec::volume_content_source::VolumeContentSource::Snapshot(
                    snap_source,
                ) => Ok(Self::Snapshot(
                    SnapshotId::try_from(StringValue(snap_source.snapshot))?,
                    VolumeId::try_from(StringValue(snap_source.snap_source_vol))?,
                )),
            },
        }
    }
}

impl From<VolumeContentSource> for volume::volume_spec::VolumeContentSource {
    fn from(value: VolumeContentSource) -> Self {
        match value {
            VolumeContentSource::Snapshot(snap, vol) => volume::volume_spec::VolumeContentSource {
                volume_content_source: Some(
                    volume::volume_spec::volume_content_source::VolumeContentSource::Snapshot(
                        volume::volume_spec::SnapshotAsSource {
                            snapshot: Some(snap.to_string()),
                            snap_source_vol: Some(vol.to_string()),
                        },
                    ),
                ),
            },
        }
    }
}

// Create volume creation event message from volume data.
impl Event for Volume {
    fn event(&self) -> EventMessage {
        let node_id = match self.spec().target() {
            Some(target) => target.node().to_string(),
            None => "".to_string(),
        };
        let event_source = EventSource::new(node_id);
        EventMessage {
            category: EventCategory::Volume as i32,
            action: EventAction::Create as i32,
            target: self.uuid().to_string(),
            metadata: Some(EventMeta::from_source(event_source)),
        }
    }
}

// Create volume delete event message.
impl Event for ValidatedDestroyVolumeRequest {
    fn event(&self) -> EventMessage {
        let event_source = EventSource::new("".to_string()); // TODO: add node_name
        EventMessage {
            category: EventCategory::Volume as i32,
            action: EventAction::Delete as i32,
            target: self.uuid.to_string(),
            metadata: Some(EventMeta::from_source(event_source)),
        }
    }
}

#[cfg(test)]
mod test {
    use crate::{
        misc::traits::ValidateRequestTypes, operations::Event, volume::DestroyVolumeRequest,
    };
    use events_api::event::{EventAction, EventCategory};
    use stor_port::types::v0::{
        store::volume::VolumeSpec,
        transport::{Volume, VolumeId, VolumeState},
    };

    #[test]
    fn volume_creation_event() {
        let vol = Volume::new(
            VolumeSpec {
                uuid: VolumeId::new(),
                target_config: None,
                ..Default::default()
            },
            VolumeState::default(),
        );
        let event = vol.event();
        assert_eq!(event.category(), EventCategory::Volume);
        assert_eq!(event.action(), EventAction::Create);
    }

    #[test]
    fn volume_deletion_event() {
        let vol = DestroyVolumeRequest {
            uuid: Some(VolumeId::new().to_string()),
        };
        let req = tonic::Request::new(vol);
        let event = req.into_inner().validated().unwrap().event();
        assert_eq!(event.category(), EventCategory::Volume);
        assert_eq!(event.action(), EventAction::Delete);
    }
}
