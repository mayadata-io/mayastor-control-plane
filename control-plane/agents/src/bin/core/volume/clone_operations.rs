use crate::{
    controller::{
        registry::Registry,
        resources::{
            operations::{ResourceCloning, ResourceLifecycleExt},
            operations_helper::SpecOperationsHelper,
            OperationGuardArc, TraceStrLog,
        },
        scheduling::{volume::CloneVolumeSnapshot, ResourceFilter},
    },
    volume::operations::{Context, CreateVolumeExe, CreateVolumeExeVal, CreateVolumeSource},
};
use agents::errors::{self, SvcError};
use stor_port::{
    transport_api::ErrorChain,
    types::v0::{
        store::{
            replica::ReplicaSpec,
            snapshots::volume::VolumeSnapshot,
            volume::{VolumeContentSource, VolumeSpec},
        },
        transport::{
            CreateVSnapshotClone, Replica, SnapshotCloneId, SnapshotCloneParameters,
            SnapshotCloneSpecParams,
        },
    },
};

pub(crate) struct SnapshotCloneOp<'a>(
    pub(crate) &'a CreateVSnapshotClone,
    pub(crate) &'a mut OperationGuardArc<VolumeSnapshot>,
);

impl SnapshotCloneOp<'_> {
    /// Get a Snapshot Source for VolumeContentSource from the same.
    pub(crate) fn to_snapshot_source(&self) -> VolumeContentSource {
        VolumeContentSource::new_snapshot_source(
            self.1.uuid().clone(),
            self.1.as_ref().spec().source_id().clone(),
        )
    }
}

#[async_trait::async_trait]
impl ResourceCloning for OperationGuardArc<VolumeSnapshot> {
    type Create = CreateVSnapshotClone;
    type CreateOutput = OperationGuardArc<VolumeSpec>;

    async fn create_clone(
        &mut self,
        registry: &Registry,
        request: &Self::Create,
    ) -> Result<Self::CreateOutput, SvcError> {
        let request = CreateVolumeSource::Snapshot(SnapshotCloneOp(request, self));
        OperationGuardArc::<VolumeSpec>::create_ext(registry, &request).await
    }
}

#[async_trait::async_trait]
impl CreateVolumeExeVal for SnapshotCloneOp<'_> {
    fn pre_flight_check(&self) -> Result<(), SvcError> {
        let new_volume = self.0.params();
        let snapshot = self.1.as_ref();

        snafu::ensure!(new_volume.replicas == 1, errors::NReplSnapshotNotAllowed {});
        snafu::ensure!(
            new_volume.allowed_nodes().is_empty()
                || new_volume.allowed_nodes().len() >= new_volume.replicas as usize,
            errors::InvalidArguments {}
        );
        snafu::ensure!(new_volume.thin, errors::ClonedSnapshotVolumeThin {});
        snafu::ensure!(snapshot.status().created(), errors::SnapshotNotCreated {});
        snafu::ensure!(
            snapshot.metadata().replica_snapshots().map(|s| s.len()) == Some(1),
            errors::ClonedSnapshotVolumeRepl {}
        );
        snafu::ensure!(
            new_volume.size == snapshot.metadata().spec_size(),
            errors::ClonedSnapshotVolumeSize {}
        );

        Ok(())
    }
}

#[async_trait::async_trait]
impl CreateVolumeExe for SnapshotCloneOp<'_> {
    type Candidates = SnapshotCloneSpecParams;

    async fn setup<'a>(&'a self, context: &mut Context<'a>) -> Result<Self::Candidates, SvcError> {
        self.cloneable_snapshot(context).await
    }

    async fn create<'a>(
        &'a self,
        context: &mut Context<'a>,
        clone_replica: Self::Candidates,
    ) -> Vec<Replica> {
        match OperationGuardArc::<ReplicaSpec>::create_ext(context.registry, &clone_replica).await {
            Ok(replica) => vec![replica],
            Err(error) => {
                context.volume.error(&format!(
                    "Failed to create replica {:?} for volume, error: {}",
                    clone_replica,
                    error.full_string()
                ));
                vec![]
            }
        }
    }

    async fn undo<'a>(&'a self, _context: &mut Context<'a>, _replicas: Vec<Replica>) {
        // nothing to undo since we only support 1-replica snapshot
    }
}

impl SnapshotCloneOp<'_> {
    /// Return healthy replica snapshots for volume snapshot cloning.
    async fn cloneable_snapshot(
        &self,
        context: &Context<'_>,
    ) -> Result<SnapshotCloneSpecParams, SvcError> {
        let new_volume = context.volume.as_ref();
        let registry = context.registry;
        let snapshot = self.1.as_ref();

        // Already validated by CreateVolumeExeVal

        let snapshots = snapshot.metadata().replica_snapshots();
        let snapshot = match snapshots.map(|s| s.as_slice()) {
            Some([replica]) => Ok(replica),
            None | Some([]) => Err(SvcError::NoHealthyReplicas {
                id: new_volume.uuid_str(),
            }),
            _ => Err(SvcError::NReplSnapshotNotAllowed {}),
        }?;

        let mut pools = CloneVolumeSnapshot::builder_with_defaults(registry, new_volume, snapshot)
            .await
            .collect();

        let pool = match pools.pop() {
            Some(pool) if pools.is_empty() => Ok(pool),
            // todo: support more than 1 replica snapshots
            Some(_) => Err(SvcError::NReplSnapshotNotAllowed {}),
            None => Err(SvcError::NoHealthyReplicas {
                id: new_volume.uuid_str(),
            }),
        }?;

        let clone_id = SnapshotCloneId::new();
        let clone_name = clone_id.to_string();
        let repl_params =
            SnapshotCloneParameters::new(snapshot.spec().uuid().clone(), clone_name, clone_id);
        Ok(SnapshotCloneSpecParams::new(
            repl_params,
            snapshot.meta().source_spec_size(),
            pool.pool().pool_ref(),
            pool.pool.node.clone(),
            new_volume.uuid.clone(),
        ))
    }
}
