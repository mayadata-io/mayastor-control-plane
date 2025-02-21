use crate::{
    controller::{
        reconciler::PollTriggerEvent,
        registry::Registry,
        resources::{ResourceMutex, ResourceUid},
    },
    node::wrapper::GetterOps,
};
use agents::errors::SvcError;
use grpc::operations::{
    volume::traits::{self as grpc_mod, VolumeReplicaSnapshotState, VolumeSnapshotState},
    PaginatedResult, Pagination,
};
use stor_port::{
    transport_api::ResourceKind,
    types::v0::{
        store::{
            nexus::NexusSpec,
            replica::ReplicaSpec,
            snapshots::{replica::ReplicaSnapshotSpec, volume::VolumeSnapshot},
            volume::VolumeSpec,
        },
        transport::{
            uri_with_hostnqn, Nexus, NexusStatus, NodeBugFix, ReplicaSnapshot, ReplicaStatus,
            ReplicaTopology, SnapshotId, Volume, VolumeId, VolumeState, VolumeStatus, VolumeUsage,
        },
    },
    IntoOption,
};

use std::collections::HashMap;

impl Registry {
    /// Get the volume state for the specified volume.
    pub(crate) async fn volume_state(
        &self,
        volume_uuid: &VolumeId,
    ) -> Result<VolumeState, SvcError> {
        let volume_spec = self.specs().volume_clone(volume_uuid)?;
        let replica_specs = self.specs().volume_replicas_cln(volume_uuid);

        self.volume_state_with_replicas(&volume_spec, &replica_specs)
            .await
    }

    /// Get the volume state for the specified volume.
    /// replicas is a pre-fetched list of replicas from any and all volumes.
    pub(crate) async fn volume_state_with_replicas(
        &self,
        volume_spec: &VolumeSpec,
        replicas: &[ReplicaSpec],
    ) -> Result<VolumeState, SvcError> {
        let replica_specs = replicas
            .iter()
            .filter(|r| r.owners.owned_by(volume_spec.uid()))
            .collect::<Vec<_>>();

        let nexus_spec = self.specs().volume_target_nexus_rsc(volume_spec);
        let nexus = match nexus_spec {
            None => None,
            Some(nexus) => {
                let spec = nexus.immutable_ref();
                self.node_nexus(&spec.node, &spec.uuid)
                    .await
                    .ok()
                    .map(|s| (nexus, s))
            }
        };

        let mut total_replica = 0;
        let mut total_snapshots = 0;
        let mut largest_replica = 0;
        let mut largest_snapshot = 0;
        let mut largest_all_snapshot = 0;

        // Construct the topological information for the volume replicas.
        let mut replica_topology = HashMap::new();
        for replica_spec in &replica_specs {
            let replica = self.replica_topology(replica_spec, &nexus).await;
            if let Some(usage) = replica.usage() {
                let allocated = usage.allocated();
                let allocated_snaps = usage.allocated_snapshots();
                total_replica += allocated;
                total_snapshots += allocated_snaps;
                if allocated > largest_replica {
                    largest_replica = allocated;
                }
                if allocated_snaps > largest_snapshot {
                    largest_snapshot = allocated_snaps;
                }
                if usage.allocated_all_snapshots() > largest_all_snapshot {
                    largest_all_snapshot = usage.allocated_all_snapshots();
                }
            }
            replica_topology.insert(replica_spec.uuid.clone(), replica);
        }

        let usage = Some(VolumeUsage::new(
            volume_spec.size,
            largest_replica + largest_snapshot,
            largest_replica,
            largest_snapshot,
            largest_all_snapshot,
            total_replica + total_snapshots,
            total_replica,
            total_snapshots,
        ));

        // todo: fill in info from the pstor(etcd)
        let health = None;

        Ok(if let Some((nexus, mut nexus_state)) = nexus {
            let ah = nexus.lock().allowed_hosts.clone();
            nexus_state.device_uri = uri_with_hostnqn(&nexus_state.device_uri, &ah);
            VolumeState {
                uuid: volume_spec.uuid.to_owned(),
                size: nexus_state.size,
                status: match nexus_state.status {
                    NexusStatus::Online
                        if nexus_state.children.len() != volume_spec.num_replicas as usize =>
                    {
                        VolumeStatus::Degraded
                    }
                    _ => nexus_state.status.clone(),
                },
                target: Some(nexus_state),
                replica_topology,
                usage,
                health,
            }
        } else {
            VolumeState {
                uuid: volume_spec.uuid.to_owned(),
                size: volume_spec.size,
                status: if volume_spec.target().is_none() {
                    if replica_specs.len() >= volume_spec.num_replicas as usize {
                        VolumeStatus::Online
                    } else if replica_specs.is_empty() {
                        VolumeStatus::Faulted
                    } else {
                        VolumeStatus::Degraded
                    }
                } else {
                    VolumeStatus::Unknown
                },
                target: None,
                replica_topology,
                usage,
                health,
            }
        })
    }

    /// Construct a replica topology from a replica spec.
    /// If the replica cannot be found, return the default replica topology.
    async fn replica_topology(
        &self,
        spec: &ReplicaSpec,
        nexus: &Option<(ResourceMutex<NexusSpec>, Nexus)>,
    ) -> ReplicaTopology {
        // todo: fill in info from the pstor(etcd)
        let healthy = None;
        match self.replica(&spec.uuid).await {
            Ok(state) => {
                let child = nexus.as_ref().and_then(|(_, n)| n.child(&state.uri));
                ReplicaTopology::new(
                    Some(state.node),
                    Some(state.pool_id),
                    state.status,
                    state.space.into_opt(),
                    child.map(|c| c.state.clone()),
                    child.map(|c| c.state_reason.clone()),
                    child.and_then(|c| c.rebuild_progress),
                    healthy,
                )
            }
            Err(_) => {
                if let Some((nexus_spec, state)) = nexus {
                    if let Some(child) = nexus_spec
                        .lock()
                        .replica_uuid_uri(&spec.uuid)
                        .and_then(|repl| state.child(repl.uri().as_str()))
                    {
                        return ReplicaTopology::new(
                            self.specs().spec_pool_node(spec.pool.pool_name()),
                            Some(spec.pool_name().clone()),
                            ReplicaStatus::Unknown,
                            None,
                            Some(child.state.clone()),
                            Some(child.state_reason.clone()),
                            child.rebuild_progress,
                            healthy,
                        );
                    }
                }
                ReplicaTopology::new(
                    self.specs().spec_pool_node(spec.pool.pool_name()),
                    Some(spec.pool_name().clone()),
                    ReplicaStatus::Unknown,
                    None,
                    None,
                    None,
                    None,
                    healthy,
                )
            }
        }
    }

    /// Get all volumes.
    pub(crate) async fn volumes(&self) -> Vec<Volume> {
        let volume_specs = self.specs().volumes();
        let replicas = self.specs().replicas_cloned();
        let mut volumes = Vec::with_capacity(volume_specs.len());
        for spec in volume_specs {
            if let Ok(state) = self.volume_state_with_replicas(&spec, &replicas).await {
                volumes.push(Volume::new(spec, state));
            }
        }
        volumes
    }

    /// Get a paginated subset of volumes.
    pub(super) async fn paginated_volumes(
        &self,
        pagination: &Pagination,
    ) -> PaginatedResult<Volume> {
        let volume_specs = self.specs().paginated_volumes(pagination);
        let mut volumes = Vec::with_capacity(volume_specs.len());
        let last = volume_specs.last();
        for spec in volume_specs.result() {
            if let Ok(state) = self.volume_state(&spec.uuid).await {
                volumes.push(Volume::new(spec, state));
            }
        }
        PaginatedResult::new(volumes, last)
    }

    /// Return a volume object corresponding to the ID.
    pub(crate) async fn volume(&self, id: &VolumeId) -> Result<Volume, SvcError> {
        Ok(Volume::new(
            self.specs().volume_clone(id)?,
            self.volume_state(id).await?,
        ))
    }

    /// Notify the reconcilers if the volume is degraded.
    pub(crate) async fn notify_if_degraded(&self, volume: &Volume, event: PollTriggerEvent) {
        if volume.status() == Some(VolumeStatus::Degraded) {
            self.notify(event).await;
        }
    }

    /// Get the replica snapshot state for the specified volume.
    pub(crate) async fn snapshot_replica(
        &self,
        snapshot: &ReplicaSnapshotSpec,
    ) -> Result<ReplicaSnapshot, SvcError> {
        let err = || SvcError::NotFound {
            kind: ResourceKind::ReplicaSnapshot,
            id: snapshot.uuid().to_string(),
        };

        let pool_id = snapshot.source_id().pool_id();
        let pool_node = self.pool_node(pool_id).await.ok_or(err())?;
        let node = self.node_wrapper(&pool_node).await?;
        let snapshot = node.snapshot(snapshot.uuid()).await;
        snapshot.ok_or(err())
    }
    /// Get the snapshot state for the specified volume.
    pub(crate) async fn snapshot_state(&self, snapshot: &VolumeSnapshot) -> VolumeSnapshotState {
        let mut replica_snaps = vec![];
        let mut allocated_size = None;
        if let Some(meta_snapshots) = snapshot.metadata().replica_snapshots() {
            replica_snaps = Vec::with_capacity(meta_snapshots.len());
            for replica_snap in meta_snapshots {
                replica_snaps.push(
                    if let Ok(state) = self.snapshot_replica(replica_snap.spec()).await {
                        allocated_size = Some(state.allocated_size());
                        VolumeReplicaSnapshotState::new_online(replica_snap.spec(), state)
                    } else {
                        VolumeReplicaSnapshotState::new_offline(replica_snap.spec())
                    },
                );
            }
        }

        VolumeSnapshotState::new(snapshot, allocated_size, replica_snaps)
    }

    /// Return a single snapshot object corresponding, either matching (vol_id + snap_id)
    /// OR matching snap_id when the input vol_id is None.
    pub(crate) async fn snapshot(
        &self,
        vol_id: Option<&VolumeId>,
        snap_id: &SnapshotId,
    ) -> Result<grpc_mod::VolumeSnapshot, SvcError> {
        // Since snapshot id is universally unique, we can use only snap id to filter the spec.
        let snapshot =
            self.specs()
                .volume_snapshot_rsc(snap_id)
                .ok_or(SvcError::VolSnapshotNotFound {
                    snap_id: snap_id.to_string(),
                    source_id: vol_id.map(|v| v.to_string()),
                })?;

        let snapshot = snapshot.lock().clone();
        if let Some(vol_id) = vol_id {
            let snap_source_id = snapshot.spec().source_id();

            // Verify that the fetched snapshot spec has the same volume id as input.
            if vol_id != snap_source_id {
                return Err(SvcError::InvalidSnapshotSource {
                    snap_id: snap_id.to_string(),
                    invalid_source_id: vol_id.to_string(),
                    correct_source_id: snap_source_id.to_string(),
                });
            }
        }

        Ok(grpc_mod::VolumeSnapshot::new(
            &snapshot,
            self.snapshot_state(&snapshot).await,
        ))
    }

    /// For a particular volume, get all the volume snapshots with states.
    pub(crate) async fn volume_snapshots(
        &self,
        vol_id: &VolumeId,
    ) -> Vec<grpc_mod::VolumeSnapshot> {
        let snap_specs = self.specs().snapshots_by_vol(vol_id);

        let mut snapshots = Vec::with_capacity(snap_specs.len());

        for spec in snap_specs {
            let state = self.snapshot_state(&spec).await;
            snapshots.push(grpc_mod::VolumeSnapshot::new(&spec, state));
        }
        snapshots
    }

    /// Get all the volume snapshots and corresponding states, for all existing volumes.
    // TODO: Merge volume_snapshots() and volume_snapshots_all, taking Optional<&VolumeId> ?
    pub(crate) async fn volume_snapshots_all(&self) -> Vec<grpc_mod::VolumeSnapshot> {
        let snap_specs = self.specs().snapshots();
        let mut snapshots = Vec::with_capacity(snap_specs.len());

        for spec in snap_specs {
            let state = self.snapshot_state(&spec).await;
            snapshots.push(grpc_mod::VolumeSnapshot::new(&spec, state));
        }
        snapshots
    }

    /// Get a paginated subset of volume snapshots.
    pub(super) async fn paginated_snapshots(
        &self,
        pagination: &Pagination,
        vol_id: Option<&VolumeId>,
    ) -> PaginatedResult<grpc_mod::VolumeSnapshot> {
        let snap_specs = self.specs().paginated_snapshots(pagination, vol_id);
        let mut snapshots = Vec::with_capacity(snap_specs.len());
        let last = snap_specs.last();
        for spec in snap_specs.result() {
            let state = self.snapshot_state(&spec).await;
            snapshots.push(grpc_mod::VolumeSnapshot::new(&spec, state));
        }
        PaginatedResult::new(snapshots, last)
    }

    /// Verify if all replica nodes have the rebuild ancestry fix and error out otherwise.
    pub fn volume_replica_nodes_fix(
        &self,
        volume: &VolumeSpec,
        fix: &NodeBugFix,
    ) -> Result<(), SvcError> {
        let specs = self.specs().read();
        let replicas = specs.volume_replicas_it(&volume.uuid);
        let nodes = specs.replica_nodes(replicas);

        self.verify_nodes_fix(nodes.iter(), fix)
    }
}
