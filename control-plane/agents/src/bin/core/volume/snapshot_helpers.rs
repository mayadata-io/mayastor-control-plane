use crate::controller::{
    registry::Registry,
    resources::{
        operations_helper::{GuardedOperationsHelper, SpecOperationsHelper},
        OperationGuardArc, ResourceUid, TraceStrLog,
    },
    scheduling::{resources::ChildItem, volume::SnapshotVolumeReplica, ResourceFilter},
};
use agents::errors::{NotEnough, SvcError};
use stor_port::{
    transport_api::ResourceKind,
    types::v0::{
        store::{
            snapshots::{
                replica::ReplicaSnapshot,
                volume::{
                    VolumeSnapshot, VolumeSnapshotCompleter, VolumeSnapshotCreateInfo,
                    VolumeSnapshotOperation,
                },
            },
            volume::VolumeSpec,
            SpecStatus, SpecTransaction,
        },
        transport::{Replica, SnapshotParameters, VolumeId},
    },
};

use std::collections::HashSet;

/// A request type for creating snapshot of a volume, which essentially
/// means a snapshot of all(or selected) healthy replicas associated with that volume.
pub(super) struct PrepareVolumeSnapshot {
    pub(super) parameters: SnapshotParameters<VolumeId>,
    pub(super) replica_snapshot: Vec<(Replica, ReplicaSnapshot)>,
    pub(super) completer: VolumeSnapshotCompleter,
}

#[async_trait::async_trait]
impl GuardedOperationsHelper for OperationGuardArc<VolumeSnapshot> {
    type Create = VolumeSnapshotCreateInfo;
    type Owners = ();
    type Status = ();
    type State = VolumeSnapshot;
    type UpdateOp = VolumeSnapshotOperation;
    type Inner = VolumeSnapshot;

    fn remove_spec(&self, registry: &Registry) {
        let uuid = self.uuid().clone();
        registry.specs().remove_volume_snapshot(&uuid);
    }
}

#[async_trait::async_trait]
impl SpecOperationsHelper for VolumeSnapshot {
    type Create = VolumeSnapshotCreateInfo;
    type Owners = ();
    type Status = ();
    type State = VolumeSnapshot;
    type UpdateOp = VolumeSnapshotOperation;

    async fn start_update_op(
        &mut self,
        _registry: &Registry,
        _state: &Self::State,
        operation: Self::UpdateOp,
    ) -> Result<(), SvcError> {
        self.start_op(operation);
        Ok(())
    }
    fn start_create_op(&mut self, request: &Self::Create) {
        self.start_op(VolumeSnapshotOperation::Create(request.clone()));
    }
    fn start_destroy_op(&mut self) {
        self.start_op(VolumeSnapshotOperation::Destroy);
    }
    fn dirty(&self) -> bool {
        self.has_pending_op()
    }
    fn kind(&self) -> ResourceKind {
        ResourceKind::VolumeSnapshot
    }
    fn uuid_str(&self) -> String {
        self.uid().to_string()
    }
    fn status(&self) -> SpecStatus<Self::Status> {
        self.status().clone()
    }
    fn set_status(&mut self, status: SpecStatus<Self::Status>) {
        self.set_status(status);
    }
    fn operation_result(&self) -> Option<Option<bool>> {
        self.metadata().operation().as_ref().map(|r| r.result)
    }
}

/// Return healthy replicas for volume snapshotting.
pub(crate) async fn snapshoteable_replica(
    volume: &VolumeSpec,
    registry: &Registry,
) -> Result<Vec<ChildItem>, SvcError> {
    let children = super::scheduling::snapshoteable_replica(volume, registry).await?;

    if children.candidates().len() != volume.num_replicas as usize {
        return Err(SvcError::InsufficientHealthyReplicas {
            id: volume.uuid_str(),
        });
    }

    volume.trace(&format!("Snapshoteable replicas for volume: {children:?}"));

    if children.candidates().is_empty() {
        return Err(SvcError::NoHealthyReplicas {
            id: volume.uuid_str(),
        });
    }

    //todo: check for snapshot chain for all the replicas.

    let pools =
        SnapshotVolumeReplica::builder_with_defaults(registry, volume, children.candidates())
            .await
            .collect();
    let pools: HashSet<_> = pools.iter().map(|item| item.pool.id.clone()).collect();

    for item in children.candidates() {
        if !pools.contains(&item.pool().id) {
            return Err(SvcError::NotEnoughResources {
                source: NotEnough::PoolFree {},
            });
        }
    }
    Ok(children.candidates().clone())
}
