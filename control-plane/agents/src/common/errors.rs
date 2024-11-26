use snafu::{Error, Snafu};
use stor_port::{
    transport_api::{ErrorChain, ReplyError, ReplyErrorKind, ResourceKind},
    types::v0::{
        store::definitions::StoreError,
        transport::{
            pool::PoolDeviceUri, ApiVersion, Filter, NodeId, NvmeNqnParseError, PoolId, ReplicaId,
        },
    },
};
use tonic::Code;

/// Common error type for send/receive
#[derive(Debug, Snafu)]
#[snafu(visibility(pub), context(suffix(false)))]
#[allow(missing_docs)]
pub enum SvcError {
    #[snafu(display("Failed to get node '{}' from the node agent", node))]
    GetNode { node: String, source: ReplyError },
    #[snafu(display("Failed to get nodes from the node agent"))]
    GetNodes { source: ReplyError },
    #[snafu(display("Node '{}' is not online", node))]
    NodeNotOnline { node: NodeId },
    #[snafu(display("Node {} has invalid socket address {}", node, socket))]
    NodeGrpcEndpoint {
        node: NodeId,
        socket: String,
        error: std::net::AddrParseError,
    },
    #[snafu(display("No available online nodes"))]
    NoNodes {},
    #[snafu(display("Node {} is cordoned", node_id))]
    CordonedNode { node_id: String },
    #[snafu(display("{resource} {id} is already labelled with labels '{labels}'"))]
    LabelsExists {
        resource: ResourceKind,
        id: String,
        labels: String,
        conflict: bool,
    },
    #[snafu(display("{resource} {id} doesn't have the label key '{label_key}'"))]
    LabelNotFound {
        resource: ResourceKind,
        id: String,
        label_key: String,
    },
    #[snafu(display("Node {node_id} is already cordoned with label '{label}'"))]
    CordonLabel { node_id: String, label: String },
    #[snafu(display("Node {node_id} does not have a cordon label '{label}'"))]
    UncordonLabel { node_id: String, label: String },
    #[snafu(display(
        "Timed out after '{:?}' attempting to connect to node '{}' via gRPC endpoint '{}'",
        timeout,
        node_id,
        endpoint
    ))]
    GrpcConnectTimeout {
        node_id: String,
        endpoint: String,
        timeout: std::time::Duration,
    },
    #[snafu(display(
        "Failed to connect to node '{}' via gRPC endpoint '{}'",
        node_id,
        endpoint
    ))]
    GrpcConnect {
        node_id: String,
        endpoint: String,
        source: tonic::transport::Error,
    },
    #[snafu(display("Node '{}' has invalid gRPC URI '{}'", node_id, uri))]
    GrpcConnectUri {
        node_id: String,
        uri: String,
        source: http::Error,
    },
    #[snafu(display(
        "gRPC request '{}' for '{}' failed with '{}'",
        request,
        resource.to_string(),
        source
    ))]
    GrpcRequestError {
        resource: ResourceKind,
        request: String,
        source: tonic::Status,
    },
    #[snafu(display("Failed to connect to grpc sever via uds socket path '{}'", path))]
    GrpcUdsConnect {
        path: String,
        source: tonic::transport::Error,
    },
    #[snafu(display("Node '{}' not found", node_id))]
    NodeNotFound { node_id: NodeId },
    #[snafu(display("Pool '{}' not loaded", pool_id))]
    PoolNotLoaded { pool_id: PoolId },
    #[snafu(display("Pool '{}' not found", pool_id))]
    PoolNotFound { pool_id: PoolId },
    #[snafu(display("Disk list should have only 1 device. Received :{:?}", disks))]
    InvalidPoolDeviceNum { disks: Vec<PoolDeviceUri> },
    #[snafu(display("Nexus '{}' not found", nexus_id))]
    NexusNotFound { nexus_id: String },
    #[snafu(display(
        "Nexus '{nexus_id}' resize status not determined. Requested size {requested_size}",
    ))]
    NexusResizeStatusUnknown {
        nexus_id: String,
        requested_size: u64,
    },
    #[snafu(display("VolumeSnapshot '{}' for volume `{:?}` not found", snap_id, source_id))]
    VolSnapshotNotFound {
        snap_id: String,
        source_id: Option<String>,
    },
    #[snafu(display(
        "Invalid source Volume: {} for VolumeSnapshot: {}, expected source Volume: {}",
        invalid_source_id,
        snap_id,
        correct_source_id
    ))]
    InvalidSnapshotSource {
        snap_id: String,
        invalid_source_id: String,
        correct_source_id: String,
    },
    #[snafu(display("{} '{}' not found", kind.to_string(), id))]
    NotFound { kind: ResourceKind, id: String },
    #[snafu(display("{} '{}' is still being created..", kind.to_string(), id))]
    PendingCreation { kind: ResourceKind, id: String },
    #[snafu(display("{} '{}' is being deleted..", kind.to_string(), id))]
    PendingDeletion { kind: ResourceKind, id: String },
    #[snafu(display("Child '{}' not found in Nexus '{}'", child, nexus))]
    ChildNotFound { nexus: String, child: String },
    #[snafu(display("Child '{}' already exists in Nexus '{}'", child, nexus))]
    ChildAlreadyExists { nexus: String, child: String },
    #[snafu(display("Volume '{}' not found", vol_id))]
    VolumeNotFound { vol_id: String },
    #[snafu(display("Affinity Group '{}' not found", vol_grp_id))]
    AffinityGroupNotFound { vol_grp_id: String },
    #[snafu(display("Volume '{}' not published", vol_id))]
    VolumeNotPublished { vol_id: String },
    #[snafu(display("Node '{}' not allowed to access target for volume '{}'", node, vol_id))]
    FrontendNodeNotAllowed { node: String, vol_id: String },
    #[snafu(display("{} {} cannot be shared over invalid protocol '{}'", kind.to_string(), id, share))]
    InvalidShareProtocol {
        kind: ResourceKind,
        id: String,
        share: String,
    },
    #[snafu(display(
        "Volume '{}' is already published on node '{}' with protocol '{}'",
        vol_id,
        node,
        protocol
    ))]
    VolumeAlreadyPublished {
        vol_id: String,
        node: String,
        protocol: String,
    },
    #[snafu(display(
        "Volume '{}' - required size is invalid. Current size: '{}', requested size: '{}'",
        vol_id,
        current_size,
        requested_size
    ))]
    VolumeResizeSize {
        vol_id: String,
        requested_size: u64,
        current_size: u64,
    },
    #[snafu(display("Replica '{}' not found", replica_id))]
    ReplicaNotFound { replica_id: ReplicaId },
    #[snafu(display("{} '{}' is already shared over {}", kind.to_string(), id, share))]
    AlreadyShared {
        kind: ResourceKind,
        id: String,
        share: String,
    },
    #[snafu(display("{} '{}' is not shared", kind.to_string(), id))]
    NotShared { kind: ResourceKind, id: String },
    #[snafu(display("Invalid filter value: {:?}", filter))]
    InvalidFilter { filter: Filter },
    #[snafu(display("Operation failed due to insufficient resources"))]
    NotEnoughResources { source: NotEnough },
    #[snafu(display("Failed to deserialise JsonRpc response"))]
    JsonRpcDeserialise { source: serde_json::Error },
    #[snafu(display(
        "Json RPC call failed for method '{}' with parameters '{}'. Error {}",
        method,
        params,
        error,
    ))]
    JsonRpc {
        method: String,
        params: String,
        error: String,
    },
    #[snafu(display("Internal error: {}", details))]
    Internal { details: String },
    #[snafu(display("Invalid Arguments"))]
    InvalidArguments {},
    #[snafu(display("IoEngine upgrade is required to rebuild volume with snapshots"))]
    UpgradeRequiredToRebuild {},
    #[snafu(display("Invalid {}, labels: {} ", resource_kind, labels))]
    InvalidLabel {
        labels: String,
        resource_kind: ResourceKind,
    },
    #[snafu(display(
        "Forbidden {}, cannot delete internal labels: {} ",
        resource_kind,
        labels
    ))]
    ForbiddenUnlabelKey {
        labels: String,
        resource_kind: ResourceKind,
    },
    #[snafu(display("Multiple nexuses not supported"))]
    MultipleNexuses {},
    #[snafu(display("Storage Error: {}", source))]
    Store { source: StoreError },
    #[snafu(display("Storage Error: {} Config for Resource id {} not committed to the store", kind.to_string(), id))]
    StoreDirty { kind: ResourceKind, id: String },
    #[snafu(display("Watch Config Not Found"))]
    WatchNotFound {},
    #[snafu(display("{} Resource to be watched does not exist", kind.to_string()))]
    WatchResourceNotFound { kind: ResourceKind },
    #[snafu(display("Watch Already Exists"))]
    WatchAlreadyExists {},
    #[snafu(display("Conflicts with existing operation - please retry"))]
    Conflict {},
    #[snafu(display("{} Resource pending deletion - please retry", kind.to_string()))]
    Deleting { kind: ResourceKind },
    #[snafu(display(
        "Retried creation of resource id {} kind {} with different parameters. Existing resource: {}, Request: {}",
        id,
        kind.to_string(),
        resource,
        request
    ))]
    ReCreateMismatch {
        id: String,
        kind: ResourceKind,
        resource: String,
        request: String,
    },
    #[snafu(display("{} Resource id {} needs to be reconciled. Please retry", kind.to_string(), id))]
    NotReady { kind: ResourceKind, id: String },
    #[snafu(display("{} Resource id {} still in use", kind.to_string(), id))]
    InUse { kind: ResourceKind, id: String },
    #[snafu(display("{} Resource id {} already exists", kind.to_string(), id))]
    AlreadyExists { kind: ResourceKind, id: String },
    #[snafu(display("Cannot remove the last replica '{}' of volume '{}'", replica, volume))]
    LastReplica { replica: String, volume: String },
    #[snafu(display(
        "Cannot remove the last healthy replica '{}' of volume '{}'",
        replica,
        volume
    ))]
    LastHealthyReplica { replica: String, volume: String },
    #[snafu(display("Replica count of Volume '{}' is already '{}'", id, count))]
    ReplicaCountAchieved { id: String, count: u8 },
    #[snafu(display("Replica count only allowed to change by a maximum of one at a time"))]
    ReplicaChangeCount {},
    #[snafu(display(
        "Unable to increase replica count due to volume '{}' in state '{}'",
        volume_id,
        volume_state
    ))]
    ReplicaIncrease {
        volume_id: String,
        volume_state: String,
    },
    #[snafu(display("Could not get rebuild history for nexus'{}'", nexus_id))]
    RebuildHistoryNotFound { nexus_id: String },
    #[snafu(display("No suitable replica removal candidates found for Volume '{}'", id))]
    ReplicaRemovalNoCandidates { id: String },
    #[snafu(display("Failed to create the desired number of replicas for Volume '{}'", id))]
    ReplicaCreateNumber { id: String },
    #[snafu(display("No online replicas are available for Volume '{}'", id))]
    NoOnlineReplicas { id: String },
    #[snafu(display("No healthy replicas are available for Volume '{}'", id))]
    NoHealthyReplicas { id: String },
    #[snafu(display("Pool not ready to take clone of snapshot '{}'", id))]
    NoSnapshotPools { id: String },
    #[snafu(display(
        "Replica Count of {} is not attainable for {}",
        count,
        resource.to_string()
    ))]
    RestrictedReplicaCount { resource: ResourceKind, count: u8 },
    #[snafu(display("Entry with key '{}' not found in the persistent store.", key))]
    StoreMissingEntry { key: String },
    #[snafu(display("The uuid '{}' for kind '{}' is not valid.", uuid, kind.to_string()))]
    InvalidUuid { uuid: String, kind: ResourceKind },
    #[snafu(display(
        "Unable to start rebuild. Maximum number of rebuilds permitted is {}",
        max_rebuilds
    ))]
    MaxRebuilds { max_rebuilds: u32 },
    #[snafu(display("The api version: {:?} is not valid", api_version))]
    InvalidApiVersion { api_version: Option<ApiVersion> },
    #[snafu(display("The subsystem with nqn: {} is not found, {}", nqn, details))]
    SubsystemNotFound { nqn: String, details: String },
    #[snafu(display("Can't add new path to subsystem with nqn: {nqn} as it is not present"))]
    ReplaceNqnNotFound { nqn: String },
    #[snafu(display(
        "The subsystem {} has unexpected Nqn ({}), expected ({})",
        path,
        nqn,
        expected_nqn
    ))]
    UnexpectedSubsystemNqn {
        nqn: String,
        expected_nqn: String,
        path: String,
    },
    #[snafu(display("The nqn couldnt be parsed"))]
    NvmeParseError {},
    #[snafu(display("Nvme connect failed: {}", details))]
    NvmeConnectError { details: String },
    #[snafu(display(
        "Remaining pool {} free space {} not enough to online child '{}' as we required {}",
        pool_id,
        free_space,
        child,
        required
    ))]
    NoCapacityToOnline {
        pool_id: String,
        child: String,
        free_space: u64,
        required: u64,
    },
    #[snafu(display(
        "Replicas {:?}, can't be resized due to required capacity ({}) or state(Online) not met.",
        replica_ids,
        required
    ))]
    ResizeReplError {
        replica_ids: Vec<String>,
        required: u64,
    },
    #[snafu(display(
        "Service for request '{}' for '{}' is unimplemented with '{}'",
        request,
        resource.to_string(),
        source
    ))]
    Unimplemented {
        resource: ResourceKind,
        request: String,
        source: tonic::Status,
    },
    #[snafu(display("Replica's {} snapshot was unexpectedly skipped", replica))]
    ReplicaSnapSkipped { replica: String },
    #[snafu(display("Replica's {} snapshot was unexpectedly not taken", replica))]
    ReplicaSnapMiss { replica: String },
    #[snafu(display("Failed to snapshot replicas {:?}", failed_replicas))]
    ReplicaSnapError {
        failed_replicas: Vec<(String, nix::errno::Errno)>,
    },
    #[snafu(display("The service is busy, cannot process request"))]
    ServiceBusy {},
    #[snafu(display("The service is shutdown, cannot process request"))]
    ServiceShutdown {},
    #[snafu(display("The snapshot is not created, and its parent volume is gone"))]
    SnapshotNotCreatedNoVolume {},
    #[snafu(display(
        "Reached maximum transactions for snapshot: {}, needs to be reconciled",
        snap_id
    ))]
    SnapshotMaxTransactions { snap_id: String },
    #[snafu(display("Cloned snapshot volumes must be thin provisioned"))]
    ClonedSnapshotVolumeThin {},
    #[snafu(display("Cloned snapshot volume must match the snapshot size"))]
    ClonedSnapshotVolumeSize {},
    #[snafu(display("Cloned snapshot volume only supported for 1 replica"))]
    ClonedSnapshotVolumeRepl {},
    #[snafu(display("The source snapshot is not created"))]
    SnapshotNotCreated {},
    #[snafu(display("Draining is not allowed without HA"))]
    DrainNotAllowedWhenHAisDisabled {},
    #[snafu(display("Target switchover is not allowed without HA"))]
    SwitchoverNotAllowedWhenHAisDisabled {},
    #[snafu(display(
        "The volume would exceed the cluster capacity limit of {}B by {}B",
        cluster_capacity_limit,
        excess
    ))]
    CapacityLimitExceeded {
        cluster_capacity_limit: u64,
        excess: u64,
    },
    #[snafu(display(
        "The number of healthy replicas does not match the expected replica count of volume '{id}'"
    ))]
    InsufficientHealthyReplicas { id: String },
    #[snafu(display(
        "Reached maximum snapshots limit {max_snapshots} for volume {volume_id}, delete unused snapshots to continue"))]
    SnapshotMaxLimit {
        max_snapshots: u32,
        volume_id: String,
    },
    #[snafu(display("Invalid property name '{property_name}' for the volume '{id}'"))]
    InvalidSetProperty { property_name: String, id: String },
    #[snafu(display(
        "The number of available snapshots for cloning ({snapshots}) does not match the number of replicas ({replicas}) for the clone volume '{id}'"
    ))]
    InsufficientSnapshotsForClone {
        snapshots: u8,
        replicas: u8,
        id: String,
    },
    #[snafu(display("Cannot set '{attribute}' on Replica '{replica}: {source}'"))]
    ReplicaSetPropertyFailed {
        attribute: String,
        replica: String,
        source: tonic::Status,
    },
}

impl SvcError {
    /// Get comparable `tonic::Code`.
    /// todo: use existing conversion Self->ReplyError->tonic instead.
    pub fn tonic_code(&self) -> tonic::Code {
        match self {
            Self::NotFound { .. } => tonic::Code::NotFound,
            Self::NexusNotFound { .. } => tonic::Code::NotFound,
            Self::NexusResizeStatusUnknown { .. } => tonic::Code::Unknown,
            Self::PoolNotFound { .. } => tonic::Code::NotFound,
            Self::ReplicaNotFound { .. } => tonic::Code::NotFound,
            Self::PoolNotLoaded { .. } => tonic::Code::FailedPrecondition,
            Self::ChildNotFound { .. } => tonic::Code::NotFound,
            Self::AlreadyExists { .. } => tonic::Code::AlreadyExists,
            Self::GrpcRequestError { source, .. } => source.code(),
            Self::GrpcConnectTimeout { .. } => tonic::Code::DeadlineExceeded,
            Self::GrpcConnect { .. } => tonic::Code::Unavailable,
            Self::GrpcUdsConnect { .. } => tonic::Code::Unavailable,
            Self::Internal { .. } => tonic::Code::Internal,
            Self::Unimplemented { .. } => tonic::Code::Unimplemented,
            Self::RestrictedReplicaCount { .. } => tonic::Code::FailedPrecondition,
            Self::ReplicaSetPropertyFailed { .. } => tonic::Code::DataLoss,
            Self::ReplaceNqnNotFound { .. } => tonic::Code::FailedPrecondition,
            _ => tonic::Code::Internal,
        }
    }
}

impl From<StoreError> for SvcError {
    fn from(source: StoreError) -> Self {
        match source {
            StoreError::MissingEntry { key } => SvcError::StoreMissingEntry { key },
            _ => SvcError::Store { source },
        }
    }
}

impl From<NvmeNqnParseError> for SvcError {
    fn from(_: NvmeNqnParseError) -> Self {
        Self::NvmeParseError {}
    }
}

impl From<NotEnough> for SvcError {
    fn from(source: NotEnough) -> Self {
        Self::NotEnoughResources { source }
    }
}

impl From<SvcError> for ReplyError {
    fn from(error: SvcError) -> Self {
        #[allow(deprecated)]
        let source = error.description();
        let source = format!("{source}: {error}");
        let extra = error.parent_full_string();

        match error {
            SvcError::StoreDirty { kind, .. } => ReplyError {
                kind: ReplyErrorKind::FailedPersist,
                resource: kind,
                source,
                extra,
            },
            SvcError::NotShared { kind, .. } => ReplyError {
                kind: ReplyErrorKind::NotShared,
                resource: kind,
                source,
                extra,
            },
            SvcError::AlreadyShared { kind, .. } => ReplyError {
                kind: ReplyErrorKind::AlreadyShared,
                resource: kind,
                source,
                extra,
            },
            SvcError::InvalidShareProtocol { kind, .. } => ReplyError {
                kind: ReplyErrorKind::InvalidArgument,
                resource: kind,
                source,
                extra,
            },
            SvcError::ChildNotFound { .. } => ReplyError {
                kind: ReplyErrorKind::NotFound,
                resource: ResourceKind::Child,
                source,
                extra,
            },
            SvcError::ChildAlreadyExists { .. } => ReplyError {
                kind: ReplyErrorKind::AlreadyExists,
                resource: ResourceKind::Child,
                source,
                extra,
            },
            SvcError::InUse { kind, id } => ReplyError {
                kind: ReplyErrorKind::InUse,
                resource: kind,
                source,
                extra: format!("id: {id}"),
            },
            SvcError::AlreadyExists { kind, id } => ReplyError {
                kind: ReplyErrorKind::AlreadyExists,
                resource: kind,
                source,
                extra: format!("id: {id}"),
            },
            SvcError::NotReady { ref kind, .. } => ReplyError {
                kind: ReplyErrorKind::Unavailable,
                resource: kind.clone(),
                source,
                extra,
            },
            SvcError::Conflict { .. } => ReplyError {
                kind: ReplyErrorKind::Conflict,
                resource: ResourceKind::Unknown,
                source,
                extra,
            },
            SvcError::Deleting { kind } => ReplyError {
                kind: ReplyErrorKind::Deleting,
                resource: kind,
                source,
                extra,
            },
            SvcError::ReCreateMismatch {
                id: _, ref kind, ..
            } => ReplyError {
                kind: ReplyErrorKind::Conflict,
                resource: kind.clone(),
                source,
                extra,
            },
            SvcError::GetNode { source, .. } => source,
            SvcError::GetNodes { source } => source,
            SvcError::GrpcRequestError {
                source,
                request,
                resource,
            } => grpc_to_reply_error(SvcError::GrpcRequestError {
                source,
                request,
                resource,
            }),

            SvcError::InvalidArguments { .. } => ReplyError {
                kind: ReplyErrorKind::InvalidArgument,
                resource: ResourceKind::Unknown,
                source,
                extra,
            },

            SvcError::UpgradeRequiredToRebuild { .. } => ReplyError {
                kind: ReplyErrorKind::FailedPrecondition,
                resource: ResourceKind::Unknown,
                source,
                extra,
            },

            SvcError::NodeNotOnline { .. } => ReplyError {
                kind: ReplyErrorKind::FailedPrecondition,
                resource: ResourceKind::Node,
                source,
                extra,
            },
            SvcError::NodeGrpcEndpoint { .. } => ReplyError {
                kind: ReplyErrorKind::Internal,
                resource: ResourceKind::Node,
                source,
                extra,
            },

            SvcError::NoNodes { .. } => ReplyError {
                kind: ReplyErrorKind::FailedPrecondition,
                resource: ResourceKind::Node,
                source,
                extra,
            },

            SvcError::CordonedNode { .. } => ReplyError {
                kind: ReplyErrorKind::FailedPrecondition,
                resource: ResourceKind::Node,
                source,
                extra,
            },

            SvcError::CordonLabel { .. } => ReplyError {
                kind: ReplyErrorKind::FailedPrecondition,
                resource: ResourceKind::Node,
                source,
                extra,
            },

            SvcError::LabelsExists {
                resource, conflict, ..
            } => ReplyError {
                kind: if conflict {
                    ReplyErrorKind::FailedPrecondition
                } else {
                    ReplyErrorKind::AlreadyExists
                },
                resource,
                source,
                extra,
            },

            SvcError::LabelNotFound { resource, .. } => ReplyError {
                kind: ReplyErrorKind::FailedPrecondition,
                resource,
                source,
                extra,
            },

            SvcError::UncordonLabel { .. } => ReplyError {
                kind: ReplyErrorKind::FailedPrecondition,
                resource: ResourceKind::Node,
                source,
                extra,
            },

            SvcError::GrpcConnectTimeout { .. } => ReplyError {
                kind: ReplyErrorKind::Timeout,
                resource: ResourceKind::Node,
                source,
                extra,
            },

            SvcError::GrpcConnectUri { .. } => ReplyError {
                kind: ReplyErrorKind::Internal,
                resource: ResourceKind::Node,
                source,
                extra,
            },

            SvcError::GrpcConnect { .. } => ReplyError {
                kind: ReplyErrorKind::Unavailable,
                resource: ResourceKind::Node,
                source,
                extra,
            },

            SvcError::NotEnoughResources { source: rsource } => ReplyError {
                kind: ReplyErrorKind::ResourceExhausted,
                resource: match rsource {
                    NotEnough::OfPools { .. } => ResourceKind::Pool,
                    NotEnough::OfReplicas { .. } => ResourceKind::Replica,
                    NotEnough::OfNexuses { .. } => ResourceKind::Nexus,
                    NotEnough::OfNodes { .. } => ResourceKind::Node,
                    NotEnough::PoolFree {} => ResourceKind::Pool,
                },
                source,
                extra,
            },
            SvcError::JsonRpcDeserialise { .. } => ReplyError {
                kind: ReplyErrorKind::Internal,
                resource: ResourceKind::JsonGrpc,
                source,
                extra,
            },
            SvcError::Store { .. } => ReplyError {
                kind: ReplyErrorKind::FailedPersist,
                resource: ResourceKind::Unknown,
                source,
                extra,
            },
            SvcError::StoreMissingEntry { .. } => ReplyError {
                kind: ReplyErrorKind::NotFound,
                resource: ResourceKind::Unknown,
                source,
                extra,
            },
            SvcError::JsonRpc { .. } => ReplyError {
                kind: ReplyErrorKind::Internal,
                resource: ResourceKind::JsonGrpc,
                source,
                extra,
            },
            SvcError::NodeNotFound { .. } => ReplyError {
                kind: ReplyErrorKind::NotFound,
                resource: ResourceKind::Node,
                source,
                extra,
            },
            SvcError::PoolNotFound { .. } => ReplyError {
                kind: ReplyErrorKind::NotFound,
                resource: ResourceKind::Pool,
                source,
                extra,
            },
            SvcError::PoolNotLoaded { .. } => ReplyError {
                kind: ReplyErrorKind::FailedPrecondition,
                resource: ResourceKind::Pool,
                source,
                extra,
            },
            SvcError::InvalidPoolDeviceNum { .. } => ReplyError {
                kind: ReplyErrorKind::InvalidArgument,
                resource: ResourceKind::Pool,
                source,
                extra,
            },
            SvcError::ReplicaNotFound { .. } => ReplyError {
                kind: ReplyErrorKind::NotFound,
                resource: ResourceKind::Replica,
                source,
                extra,
            },
            SvcError::NexusNotFound { .. } => ReplyError {
                kind: ReplyErrorKind::NotFound,
                resource: ResourceKind::Nexus,
                source,
                extra,
            },
            SvcError::NotFound { ref kind, .. } => ReplyError {
                kind: ReplyErrorKind::NotFound,
                resource: kind.clone(),
                source,
                extra,
            },
            SvcError::PendingCreation { ref kind, .. } => ReplyError {
                kind: ReplyErrorKind::FailedPrecondition,
                resource: kind.clone(),
                source,
                extra,
            },
            SvcError::PendingDeletion { ref kind, .. } => ReplyError {
                kind: ReplyErrorKind::FailedPrecondition,
                resource: kind.clone(),
                source,
                extra,
            },
            SvcError::VolumeNotFound { .. } => ReplyError {
                kind: ReplyErrorKind::NotFound,
                resource: ResourceKind::Volume,
                source,
                extra,
            },
            SvcError::VolumeNotPublished { .. } => ReplyError {
                kind: ReplyErrorKind::NotPublished,
                resource: ResourceKind::Volume,
                source,
                extra,
            },
            SvcError::VolumeAlreadyPublished { .. } => ReplyError {
                kind: ReplyErrorKind::AlreadyPublished,
                resource: ResourceKind::Volume,
                source,
                extra,
            },
            SvcError::FrontendNodeNotAllowed { .. } => ReplyError {
                kind: ReplyErrorKind::PermissionDenied,
                resource: ResourceKind::Volume,
                source,
                extra,
            },
            SvcError::WatchResourceNotFound { kind } => ReplyError {
                kind: ReplyErrorKind::NotFound,
                resource: kind,
                source,
                extra,
            },
            SvcError::WatchNotFound { .. } => ReplyError {
                kind: ReplyErrorKind::NotFound,
                resource: ResourceKind::Watch,
                source,
                extra,
            },
            SvcError::WatchAlreadyExists { .. } => ReplyError {
                kind: ReplyErrorKind::AlreadyExists,
                resource: ResourceKind::Watch,
                source,
                extra,
            },
            SvcError::InvalidFilter { .. } => ReplyError {
                kind: ReplyErrorKind::Internal,
                resource: ResourceKind::Unknown,
                source,
                extra,
            },
            SvcError::Internal { .. } => ReplyError {
                kind: ReplyErrorKind::Internal,
                resource: ResourceKind::Unknown,
                source,
                extra,
            },
            SvcError::InvalidLabel { resource_kind, .. } => ReplyError {
                kind: ReplyErrorKind::InvalidArgument,
                resource: resource_kind,
                source,
                extra,
            },
            SvcError::ForbiddenUnlabelKey { resource_kind, .. } => ReplyError {
                kind: ReplyErrorKind::InvalidArgument,
                resource: resource_kind,
                source,
                extra,
            },
            SvcError::MultipleNexuses { .. } => ReplyError {
                kind: ReplyErrorKind::InvalidArgument,
                resource: ResourceKind::Unknown,
                source,
                extra,
            },
            SvcError::LastReplica { .. } => ReplyError {
                kind: ReplyErrorKind::FailedPrecondition,
                resource: ResourceKind::Volume,
                source,
                extra,
            },
            SvcError::LastHealthyReplica { .. } => ReplyError {
                kind: ReplyErrorKind::FailedPrecondition,
                resource: ResourceKind::Volume,
                source,
                extra,
            },
            SvcError::ReplicaCountAchieved { .. } => ReplyError {
                kind: ReplyErrorKind::ReplicaCountAchieved,
                resource: ResourceKind::Volume,
                source,
                extra,
            },
            SvcError::ReplicaChangeCount { .. } => ReplyError {
                kind: ReplyErrorKind::ReplicaChangeCount,
                resource: ResourceKind::Volume,
                source,
                extra,
            },
            SvcError::ReplicaIncrease { .. } => ReplyError {
                kind: ReplyErrorKind::ReplicaIncrease,
                resource: ResourceKind::Volume,
                source,
                extra,
            },
            SvcError::RebuildHistoryNotFound { .. } => ReplyError {
                kind: ReplyErrorKind::NotFound,
                resource: ResourceKind::Nexus,
                source,
                extra,
            },
            SvcError::ReplicaRemovalNoCandidates { .. } => ReplyError {
                kind: ReplyErrorKind::ReplicaChangeCount,
                resource: ResourceKind::Volume,
                source,
                extra,
            },
            SvcError::NoOnlineReplicas { .. } => ReplyError {
                kind: ReplyErrorKind::VolumeNoReplicas,
                resource: ResourceKind::Volume,
                source,
                extra,
            },
            SvcError::NoHealthyReplicas { .. } => ReplyError {
                kind: ReplyErrorKind::VolumeNoReplicas,
                resource: ResourceKind::Volume,
                source,
                extra,
            },
            SvcError::NoSnapshotPools { .. } => ReplyError {
                kind: ReplyErrorKind::FailedPrecondition,
                resource: ResourceKind::VolumeSnapshotClone,
                source,
                extra,
            },
            SvcError::ReplicaCreateNumber { .. } => ReplyError {
                kind: ReplyErrorKind::ReplicaCreateNumber,
                resource: ResourceKind::Volume,
                source,
                extra,
            },
            SvcError::InvalidUuid { ref kind, .. } => ReplyError {
                kind: ReplyErrorKind::InvalidArgument,
                resource: kind.clone(),
                source,
                extra,
            },
            SvcError::MaxRebuilds { .. } => ReplyError {
                kind: ReplyErrorKind::ResourceExhausted,
                resource: ResourceKind::Volume,
                source,
                extra,
            },
            SvcError::InvalidApiVersion { .. } => ReplyError {
                kind: ReplyErrorKind::InvalidArgument,
                resource: ResourceKind::Unknown,
                source,
                extra,
            },
            SvcError::SubsystemNotFound { .. } => ReplyError {
                kind: ReplyErrorKind::NotFound,
                resource: ResourceKind::NvmeSubsystem,
                source,
                extra,
            },
            SvcError::ReplaceNqnNotFound { .. } => ReplyError {
                kind: ReplyErrorKind::FailedPrecondition,
                resource: ResourceKind::NvmePath,
                source,
                extra,
            },
            SvcError::UnexpectedSubsystemNqn { .. } => ReplyError {
                kind: ReplyErrorKind::InvalidArgument,
                resource: ResourceKind::NvmeSubsystem,
                source,
                extra,
            },
            SvcError::NvmeParseError { .. } => ReplyError {
                kind: ReplyErrorKind::Internal,
                resource: ResourceKind::NvmePath,
                source,
                extra,
            },
            SvcError::GrpcUdsConnect { .. } => ReplyError {
                kind: ReplyErrorKind::Unavailable,
                resource: ResourceKind::Unknown,
                source,
                extra,
            },
            SvcError::NvmeConnectError { .. } => ReplyError {
                kind: ReplyErrorKind::Aborted,
                resource: ResourceKind::NvmeSubsystem,
                source,
                extra,
            },
            SvcError::NoCapacityToOnline { .. } => ReplyError {
                kind: ReplyErrorKind::ResourceExhausted,
                resource: ResourceKind::Pool,
                source,
                extra,
            },
            // We won't ever return this error code back though?
            SvcError::NexusResizeStatusUnknown { .. } => ReplyError {
                kind: ReplyErrorKind::Internal,
                resource: ResourceKind::Nexus,
                source,
                extra,
            },
            SvcError::ResizeReplError { .. } => ReplyError {
                kind: ReplyErrorKind::ResourceExhausted,
                resource: ResourceKind::Replica,
                source,
                extra,
            },
            SvcError::VolumeResizeSize { .. } => ReplyError {
                kind: ReplyErrorKind::NotAcceptable,
                resource: ResourceKind::Volume,
                source,
                extra,
            },
            SvcError::Unimplemented { resource, .. } => ReplyError {
                kind: ReplyErrorKind::Unimplemented,
                resource,
                source,
                extra,
            },
            SvcError::AffinityGroupNotFound { .. } => ReplyError {
                kind: ReplyErrorKind::NotFound,
                resource: ResourceKind::AffinityGroup,
                source,
                extra,
            },
            SvcError::RestrictedReplicaCount { resource, .. } => ReplyError {
                kind: ReplyErrorKind::FailedPrecondition,
                resource,
                source,
                extra,
            },
            SvcError::ReplicaSnapSkipped { .. } => ReplyError {
                kind: ReplyErrorKind::Aborted,
                resource: ResourceKind::VolumeSnapshot,
                source,
                extra,
            },
            SvcError::ReplicaSnapMiss { .. } => ReplyError {
                kind: ReplyErrorKind::Aborted,
                resource: ResourceKind::VolumeSnapshot,
                source,
                extra,
            },
            SvcError::ReplicaSnapError { .. } => ReplyError {
                kind: ReplyErrorKind::Aborted,
                resource: ResourceKind::VolumeSnapshot,
                source,
                extra,
            },
            SvcError::VolSnapshotNotFound { .. } => ReplyError {
                kind: ReplyErrorKind::NotFound,
                resource: ResourceKind::VolumeSnapshot,
                source,
                extra,
            },
            SvcError::InvalidSnapshotSource { .. } => ReplyError {
                kind: ReplyErrorKind::InvalidArgument,
                resource: ResourceKind::VolumeSnapshot,
                source,
                extra,
            },
            SvcError::SnapshotNotCreatedNoVolume { .. } => ReplyError {
                kind: ReplyErrorKind::FailedPrecondition,
                resource: ResourceKind::VolumeSnapshot,
                source,
                extra,
            },
            SvcError::ServiceBusy {} => ReplyError {
                kind: ReplyErrorKind::Aborted,
                resource: ResourceKind::Unknown,
                source,
                extra,
            },
            SvcError::ServiceShutdown {} => ReplyError {
                kind: ReplyErrorKind::Unavailable,
                resource: ResourceKind::Unknown,
                source,
                extra,
            },
            SvcError::SnapshotMaxTransactions { .. } => ReplyError {
                kind: ReplyErrorKind::DeadlineExceeded,
                resource: ResourceKind::VolumeSnapshot,
                source,
                extra,
            },
            SvcError::ClonedSnapshotVolumeRepl {} => ReplyError {
                kind: ReplyErrorKind::InvalidArgument,
                resource: ResourceKind::VolumeSnapshotClone,
                source,
                extra,
            },
            SvcError::ClonedSnapshotVolumeSize {} => ReplyError {
                kind: ReplyErrorKind::OutOfRange,
                resource: ResourceKind::VolumeSnapshotClone,
                source,
                extra,
            },
            SvcError::ClonedSnapshotVolumeThin {} => ReplyError {
                kind: ReplyErrorKind::InvalidArgument,
                resource: ResourceKind::VolumeSnapshotClone,
                source,
                extra,
            },
            SvcError::SnapshotNotCreated {} => ReplyError {
                kind: ReplyErrorKind::InvalidArgument,
                resource: ResourceKind::VolumeSnapshot,
                source,
                extra,
            },
            SvcError::DrainNotAllowedWhenHAisDisabled {} => ReplyError {
                kind: ReplyErrorKind::FailedPrecondition,
                resource: ResourceKind::Node,
                source,
                extra,
            },
            SvcError::SwitchoverNotAllowedWhenHAisDisabled {} => ReplyError {
                kind: ReplyErrorKind::FailedPrecondition,
                resource: ResourceKind::Nexus,
                source,
                extra,
            },
            SvcError::CapacityLimitExceeded { .. } => ReplyError {
                kind: ReplyErrorKind::CapacityLimitExceeded,
                resource: ResourceKind::Volume,
                source,
                extra,
            },
            SvcError::InsufficientHealthyReplicas { .. } => ReplyError {
                kind: ReplyErrorKind::FailedPrecondition,
                resource: ResourceKind::VolumeSnapshot,
                source,
                extra,
            },
            SvcError::SnapshotMaxLimit { .. } => ReplyError {
                kind: ReplyErrorKind::OutOfRange,
                resource: ResourceKind::Volume,
                source,
                extra,
            },
            SvcError::InvalidSetProperty { .. } => ReplyError {
                kind: ReplyErrorKind::InvalidArgument,
                resource: ResourceKind::Volume,
                source,
                extra,
            },
            SvcError::InsufficientSnapshotsForClone { .. } => ReplyError {
                kind: ReplyErrorKind::FailedPrecondition,
                resource: ResourceKind::VolumeSnapshot,
                source,
                extra,
            },
            SvcError::ReplicaSetPropertyFailed { .. } => ReplyError {
                kind: ReplyErrorKind::FailedPersist,
                resource: ResourceKind::Replica,
                source,
                extra,
            },
        }
    }
}

fn grpc_to_reply_error(error: SvcError) -> ReplyError {
    match error {
        SvcError::GrpcRequestError {
            source,
            request,
            resource,
        } => {
            let kind = match source.code() {
                Code::Ok => ReplyErrorKind::Internal,
                Code::Cancelled => ReplyErrorKind::Cancelled,
                Code::Unknown => ReplyErrorKind::Internal,
                Code::InvalidArgument => ReplyErrorKind::InvalidArgument,
                Code::DeadlineExceeded => ReplyErrorKind::DeadlineExceeded,
                Code::NotFound => ReplyErrorKind::NotFound,
                Code::AlreadyExists => ReplyErrorKind::AlreadyExists,
                Code::PermissionDenied => ReplyErrorKind::PermissionDenied,
                Code::ResourceExhausted => ReplyErrorKind::ResourceExhausted,
                Code::FailedPrecondition => ReplyErrorKind::FailedPrecondition,
                Code::Aborted => ReplyErrorKind::Aborted,
                Code::OutOfRange => ReplyErrorKind::OutOfRange,
                Code::Unimplemented => ReplyErrorKind::Unimplemented,
                Code::Internal => ReplyErrorKind::Internal,
                Code::Unavailable => ReplyErrorKind::Unavailable,
                Code::DataLoss => ReplyErrorKind::Internal,
                Code::Unauthenticated => ReplyErrorKind::Unauthenticated,
            };
            let extra = format!("{request}::{source}");
            ReplyError {
                kind,
                resource,
                source: "SvcError::GrpcRequestError".to_string(),
                extra,
            }
        }
        _ => unreachable!("Expected a GrpcRequestError!"),
    }
}

/// Not enough resources available
#[derive(Debug, Snafu)]
#[allow(missing_docs)]
pub enum NotEnough {
    #[snafu(display("Not enough suitable pools available, {}/{}", have, need))]
    OfPools { have: u64, need: u64 },
    #[snafu(display("Not enough replicas available, {}/{}", have, need))]
    OfReplicas { have: u64, need: u64 },
    #[snafu(display("Not enough nexuses available, {}/{}", have, need))]
    OfNexuses { have: u64, need: u64 },
    #[snafu(display("Not enough nodes available, {}/{}", have, need))]
    OfNodes { have: u64, need: u64 },
    #[snafu(display("Not enough free space in the pool"))]
    PoolFree {},
}
