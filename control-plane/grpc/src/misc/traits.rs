use crate::common;
use stor_port::transport_api::{ReplyError, ReplyErrorKind, ResourceKind};

/// Trait to validate the Grpc type by an intermediate conversion
pub trait ValidateRequestTypes {
    /// The validated type
    type Validated;
    /// The method that is needed to be implemented to ensure validation
    fn validated(self) -> Result<Self::Validated, ReplyError>;
}

impl From<ResourceKind> for common::ResourceKind {
    fn from(kind: ResourceKind) -> Self {
        match kind {
            ResourceKind::Unknown => Self::Unknown,
            ResourceKind::Node => Self::Node,
            ResourceKind::Pool => Self::Pool,
            ResourceKind::Replica => Self::Replica,
            ResourceKind::ReplicaState => Self::ReplicaState,
            ResourceKind::ReplicaSpec => Self::ReplicaSpec,
            ResourceKind::Nexus => Self::Nexus,
            ResourceKind::Child => Self::Child,
            ResourceKind::Volume => Self::Volume,
            ResourceKind::JsonGrpc => Self::JsonGrpc,
            ResourceKind::Block => Self::Block,
            ResourceKind::Watch => Self::Watch,
            ResourceKind::Spec => Self::Spec,
            ResourceKind::State => Self::State,
            ResourceKind::NvmeSubsystem => Self::NvmeSubsystem,
            ResourceKind::NvmePath => Self::NvmePath,
            ResourceKind::AffinityGroup => Self::AffinityGroup,
            ResourceKind::VolumeSnapshot => Self::VolumeSnapshot,
            ResourceKind::ReplicaSnapshot => Self::ReplicaSnapshot,
            ResourceKind::VolumeSnapshotClone => Self::VolumeSnapshotClone,
            ResourceKind::ReplicaSnapshotClone => Self::ReplicaSnapshotClone,
            ResourceKind::AppNode => Self::AppNode,
        }
    }
}

impl From<common::ResourceKind> for ResourceKind {
    fn from(kind: common::ResourceKind) -> Self {
        match kind {
            common::ResourceKind::Unknown => Self::Unknown,
            common::ResourceKind::Node => Self::Node,
            common::ResourceKind::Pool => Self::Pool,
            common::ResourceKind::Replica => Self::Replica,
            common::ResourceKind::ReplicaState => Self::ReplicaState,
            common::ResourceKind::ReplicaSpec => Self::ReplicaSpec,
            common::ResourceKind::Nexus => Self::Nexus,
            common::ResourceKind::Child => Self::Child,
            common::ResourceKind::Volume => Self::Volume,
            common::ResourceKind::JsonGrpc => Self::JsonGrpc,
            common::ResourceKind::Block => Self::Block,
            common::ResourceKind::Watch => Self::Watch,
            common::ResourceKind::Spec => Self::Spec,
            common::ResourceKind::State => Self::State,
            common::ResourceKind::NvmeSubsystem => Self::NvmeSubsystem,
            common::ResourceKind::NvmePath => Self::NvmePath,
            common::ResourceKind::AffinityGroup => Self::AffinityGroup,
            common::ResourceKind::VolumeSnapshot => Self::VolumeSnapshot,
            common::ResourceKind::ReplicaSnapshot => Self::ReplicaSnapshot,
            common::ResourceKind::VolumeSnapshotClone => Self::VolumeSnapshotClone,
            common::ResourceKind::ReplicaSnapshotClone => Self::ReplicaSnapshotClone,
            common::ResourceKind::AppNode => Self::AppNode,
        }
    }
}

impl From<ReplyErrorKind> for common::ReplyErrorKind {
    fn from(kind: ReplyErrorKind) -> Self {
        match kind {
            ReplyErrorKind::WithMessage => Self::WithMessage,
            ReplyErrorKind::DeserializeReq => Self::DeserializeReq,
            ReplyErrorKind::Internal => Self::Internal,
            ReplyErrorKind::Timeout => Self::Timeout,
            ReplyErrorKind::InvalidArgument => Self::InvalidArgument,
            ReplyErrorKind::DeadlineExceeded => Self::DeadlineExceeded,
            ReplyErrorKind::NotFound => Self::NotFound,
            ReplyErrorKind::AlreadyExists => Self::AlreadyExists,
            ReplyErrorKind::PermissionDenied => Self::PermissionDenied,
            ReplyErrorKind::ResourceExhausted => Self::ResourceExhausted,
            ReplyErrorKind::FailedPrecondition => Self::FailedPrecondition,
            ReplyErrorKind::Aborted => Self::Aborted,
            ReplyErrorKind::OutOfRange => Self::OutOfRange,
            ReplyErrorKind::Unimplemented => Self::Unimplemented,
            ReplyErrorKind::Unavailable => Self::Unavailable,
            ReplyErrorKind::Unauthenticated => Self::Unauthenticated,
            ReplyErrorKind::Unauthorized => Self::Unauthorized,
            ReplyErrorKind::Conflict => Self::Conflict,
            ReplyErrorKind::FailedPersist => Self::FailedPersist,
            ReplyErrorKind::NotShared => Self::NotShared,
            ReplyErrorKind::AlreadyShared => Self::AlreadyShared,
            ReplyErrorKind::NotPublished => Self::NotPublished,
            ReplyErrorKind::AlreadyPublished => Self::AlreadyPublished,
            ReplyErrorKind::Deleting => Self::IsDeleting,
            ReplyErrorKind::ReplicaCountAchieved => Self::ReplicaCountAchieved,
            ReplyErrorKind::ReplicaChangeCount => Self::ReplicaChangeCount,
            ReplyErrorKind::ReplicaIncrease => Self::ReplicaIncrease,
            ReplyErrorKind::ReplicaCreateNumber => Self::ReplicaCreateNumber,
            ReplyErrorKind::VolumeNoReplicas => Self::VolumeNoReplicas,
            ReplyErrorKind::InUse => Self::InUse,
            ReplyErrorKind::CapacityLimitExceeded => Self::CapacityLimitExceeded,
            ReplyErrorKind::NotAcceptable => Self::NotAcceptable,
            ReplyErrorKind::Cancelled => Self::Cancelled,
        }
    }
}

impl From<common::ReplyErrorKind> for ReplyErrorKind {
    fn from(kind: common::ReplyErrorKind) -> Self {
        match kind {
            common::ReplyErrorKind::WithMessage => Self::WithMessage,
            common::ReplyErrorKind::DeserializeReq => Self::DeserializeReq,
            common::ReplyErrorKind::Internal => Self::Internal,
            common::ReplyErrorKind::Timeout => Self::Timeout,
            common::ReplyErrorKind::InvalidArgument => Self::InvalidArgument,
            common::ReplyErrorKind::DeadlineExceeded => Self::DeadlineExceeded,
            common::ReplyErrorKind::NotFound => Self::NotFound,
            common::ReplyErrorKind::AlreadyExists => Self::AlreadyExists,
            common::ReplyErrorKind::PermissionDenied => Self::PermissionDenied,
            common::ReplyErrorKind::ResourceExhausted => Self::ResourceExhausted,
            common::ReplyErrorKind::FailedPrecondition => Self::FailedPrecondition,
            common::ReplyErrorKind::Aborted => Self::Aborted,
            common::ReplyErrorKind::OutOfRange => Self::OutOfRange,
            common::ReplyErrorKind::Unimplemented => Self::Unimplemented,
            common::ReplyErrorKind::Unavailable => Self::Unavailable,
            common::ReplyErrorKind::Unauthenticated => Self::Unauthenticated,
            common::ReplyErrorKind::Unauthorized => Self::Unauthorized,
            common::ReplyErrorKind::Conflict => Self::Conflict,
            common::ReplyErrorKind::FailedPersist => Self::FailedPersist,
            common::ReplyErrorKind::NotShared => Self::NotShared,
            common::ReplyErrorKind::AlreadyShared => Self::AlreadyShared,
            common::ReplyErrorKind::NotPublished => Self::NotPublished,
            common::ReplyErrorKind::AlreadyPublished => Self::AlreadyPublished,
            common::ReplyErrorKind::IsDeleting => Self::Deleting,
            common::ReplyErrorKind::ReplicaCountAchieved => Self::ReplicaCountAchieved,
            common::ReplyErrorKind::ReplicaChangeCount => Self::ReplicaChangeCount,
            common::ReplyErrorKind::ReplicaIncrease => Self::ReplicaIncrease,
            common::ReplyErrorKind::ReplicaCreateNumber => Self::ReplicaCreateNumber,
            common::ReplyErrorKind::VolumeNoReplicas => Self::VolumeNoReplicas,
            common::ReplyErrorKind::InUse => Self::InUse,
            common::ReplyErrorKind::CapacityLimitExceeded => Self::CapacityLimitExceeded,
            common::ReplyErrorKind::NotAcceptable => Self::NotAcceptable,
            common::ReplyErrorKind::Cancelled => Self::Cancelled,
        }
    }
}

impl From<ReplyError> for crate::common::ReplyError {
    fn from(err: ReplyError) -> Self {
        let kind: common::ReplyErrorKind = err.clone().kind.into();
        let resource: common::ResourceKind = err.clone().resource.into();
        crate::common::ReplyError {
            kind: kind as i32,
            resource: resource as i32,
            source: err.clone().source,
            extra: err.extra,
        }
    }
}

impl From<crate::common::ReplyError> for ReplyError {
    fn from(err: crate::common::ReplyError) -> Self {
        ReplyError {
            kind: common::ReplyErrorKind::try_from(err.clone().kind)
                .unwrap_or(common::ReplyErrorKind::Aborted)
                .into(),
            resource: common::ResourceKind::try_from(err.clone().resource)
                .unwrap_or(common::ResourceKind::Unknown)
                .into(),
            source: err.clone().source,
            extra: err.extra,
        }
    }
}

/// A newtype that is similar to a google StringValue generated code
/// for simpler conversion to uuids
pub struct StringValue(pub Option<String>);
