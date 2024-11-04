use crate::{
    controller::{
        registry::Registry,
        resources::{
            operations_helper::{
                GuardedOperationsHelper, OnCreateFail, OperationSequenceGuard, ResourceSpecs,
                ResourceSpecsLocked, SpecOperationsHelper,
            },
            OperationGuardArc, ResourceMutex, ResourceUid, TraceSpan, TraceStrLog,
        },
        scheduling::{
            nexus::GetPersistedNexusChildren,
            resources::{ChildItem, HealthyChildItems, ReplicaItem},
            volume::{
                AddVolumeNexusReplicas, GetChildForRemoval, GetSuitablePools, MoveReplica,
                ReplicaRemovalCandidates,
            },
            ResourceFilter,
        },
    },
    volume::{operations::CreateVolumeSource, scheduling},
};
use agents::{
    errors,
    errors::{
        NotEnough, SvcError,
        SvcError::{VolSnapshotNotFound, VolumeNotFound},
    },
};
use grpc::operations::{PaginatedResult, Pagination};
use stor_port::{
    transport_api::ResourceKind,
    types::v0::{
        store::{
            definitions::ObjectKey,
            nexus::NexusSpec,
            nexus_persistence::NexusInfoKey,
            replica::ReplicaSpec,
            snapshots::volume::{VolumeSnapshot, VolumeSnapshotUserSpec},
            volume::{AffinityGroupId, AffinityGroupSpec, VolumeOperation, VolumeSpec},
            SpecStatus, SpecTransaction,
        },
        transport::{
            CreateReplica, CreateVolume, NodeBugFix, NodeId, PoolId, Protocol, Replica, ReplicaId,
            ReplicaName, ReplicaOwners, SnapshotId, VolumeId, VolumeShareProtocol, VolumeState,
            VolumeStatus,
        },
    },
};

use snafu::OptionExt;
use std::convert::From;

/// CreateReplicaCandidate for volume and Affinity Group.
pub(crate) struct CreateReplicaCandidate {
    candidates: Vec<CreateReplica>,
    _affinity_group_guard: Option<OperationGuardArc<AffinityGroupSpec>>,
}

impl CreateReplicaCandidate {
    /// Create a new `CreateReplicaCandidate` with candidates and optional ag guard.
    pub(crate) fn new(
        candidates: Vec<CreateReplica>,
        affinity_group_guard: Option<OperationGuardArc<AffinityGroupSpec>>,
    ) -> CreateReplicaCandidate {
        Self {
            candidates,
            _affinity_group_guard: affinity_group_guard,
        }
    }
    /// Get the candidates.
    pub(crate) fn candidates(&self) -> &Vec<CreateReplica> {
        &self.candidates
    }
}

/// NexusNodeCandidate for nexus node selection.
pub(crate) struct NexusNodeCandidate {
    candidate: NodeId,
    _affinity_group_guard: Option<OperationGuardArc<AffinityGroupSpec>>,
}

impl NexusNodeCandidate {
    /// Create a new `NexusNodeCandidate` with candidate and optional ag guard.
    pub(crate) fn new(
        candidate: NodeId,
        affinity_group_guard: Option<OperationGuardArc<AffinityGroupSpec>>,
    ) -> NexusNodeCandidate {
        Self {
            candidate,
            _affinity_group_guard: affinity_group_guard,
        }
    }
    /// Get the candidate.
    pub(crate) fn candidate(&self) -> &NodeId {
        &self.candidate
    }
}

/// Select a replica to be removed from the volume.
pub(crate) async fn volume_replica_remove_candidate(
    spec: &VolumeSpec,
    state: &VolumeState,
    registry: &Registry,
) -> Result<ReplicaItem, SvcError> {
    let mut candidates = scheduling::volume_replica_remove_candidates(
        &GetChildForRemoval::new(spec, state, false),
        registry,
    )
    .await?
    .candidates();

    spec.trace_span(|| tracing::trace!("Volume Replica removal candidates: {:?}", candidates));

    candidates
        .next()
        .context(errors::ReplicaRemovalNoCandidates {
            id: spec.uuid_str(),
        })
}

/// Get replica candidates to be removed from the volume.
/// This list includes healthy and non_healthy candidates, so care must be taken to
/// make sure we don't remove "too many healthy" candidates.
pub(crate) async fn volume_unused_replica_remove_candidates(
    spec: &VolumeSpec,
    state: &VolumeState,
    registry: &Registry,
) -> Result<ReplicaRemovalCandidates, SvcError> {
    let candidates = scheduling::volume_replica_remove_candidates(
        &GetChildForRemoval::new(spec, state, true),
        registry,
    )
    .await?
    .candidates();

    spec.trace(&format!(
        "Unused Replica removal candidates for volume: {candidates:?}"
    ));

    Ok(candidates)
}

/// Get a list of nexus children to be removed from a nexus.
pub(crate) async fn nexus_child_remove_candidates(
    vol_spec: &VolumeSpec,
    nexus_spec: &NexusSpec,
    registry: &Registry,
) -> Result<ReplicaRemovalCandidates, SvcError> {
    let candidates = scheduling::nexus_child_remove_candidates(vol_spec, nexus_spec, registry)
        .await?
        .candidates();

    nexus_spec.debug(&format!("Nexus Child removal candidates: {candidates:?}"));

    Ok(candidates)
}

/// Get a list of existing candidate volume replicas to attach to a given nexus.
/// Useful to attach replicas to a nexus when the number of nexus children does not match
/// the volume's replica count.
pub(crate) async fn nexus_attach_candidates(
    vol_spec: &VolumeSpec,
    nexus_spec: &NexusSpec,
    registry: &Registry,
) -> Result<Vec<ChildItem>, SvcError> {
    let candidates = AddVolumeNexusReplicas::builder_with_defaults(vol_spec, nexus_spec, registry)
        .await?
        .collect();

    nexus_spec.debug(&format!("Nexus replica attach candidates: {candidates:?}"));

    Ok(candidates)
}

/// Return a list of appropriate requests which can be used to create a replica on a pool.
/// This can be used when the volume's current replica count is smaller than the desired volume's
/// replica count.
pub(crate) async fn volume_replica_candidates(
    registry: &Registry,
    volume_spec: &VolumeSpec,
) -> Result<Vec<CreateReplica>, SvcError> {
    let request = GetSuitablePools::new(volume_spec, None);
    let pools = scheduling::volume_pool_candidates(request.clone(), registry).await;

    if pools.is_empty() {
        return Err(SvcError::NotEnoughResources {
            source: NotEnough::OfPools { have: 0, need: 1 },
        });
    }

    volume_spec.trace(&format!(
        "Creation pool candidates for volume: {:?}",
        pools.iter().map(|p| p.state()).collect::<Vec<_>>()
    ));

    Ok(pools
        .iter()
        .map(|p| {
            let replica_uuid = ReplicaId::new();
            CreateReplica {
                node: p.node.clone(),
                name: Some(ReplicaName::new(&replica_uuid, Some(&request.uuid))),
                uuid: replica_uuid,
                entity_id: Some(volume_spec.uuid.clone()),
                pool_id: p.id.clone(),
                pool_uuid: None,
                size: request.size,
                thin: request.as_thin(),
                share: Protocol::None,
                managed: true,
                owners: ReplicaOwners::from_volume(&request.uuid),
                allowed_hosts: vec![],
                kind: None,
            }
        })
        .collect::<Vec<_>>())
}

/// Return a list of appropriate requests which can be used to create a replica on a pool to replace
/// a given replica.
/// This can be used when attempting to move a replica due to ENOSPC.
pub(crate) async fn volume_move_replica_candidates(
    registry: &Registry,
    volume_spec: &VolumeSpec,
    move_replica: &ReplicaId,
) -> Result<Vec<CreateReplica>, SvcError> {
    let replica_state = registry.replica(move_replica).await?;

    let move_repl = MoveReplica::new(&replica_state.node, &replica_state.pool_id);
    let request = GetSuitablePools::new(volume_spec, Some(move_repl));
    let pools = scheduling::volume_pool_candidates(request.clone(), registry).await;

    if pools.is_empty() {
        return Err(SvcError::NotEnoughResources {
            source: NotEnough::OfPools { have: 0, need: 1 },
        });
    }

    volume_spec.trace(&format!(
        "Creation pool candidates for volume: {:?}",
        pools.iter().map(|p| p.state()).collect::<Vec<_>>()
    ));

    Ok(pools
        .iter()
        .map(|p| {
            let replica_uuid = ReplicaId::new();
            CreateReplica {
                node: p.node.clone(),
                name: Some(ReplicaName::new(&replica_uuid, Some(&request.uuid))),
                uuid: replica_uuid,
                entity_id: Some(volume_spec.uuid.clone()),
                pool_id: p.id.clone(),
                pool_uuid: None,
                size: request.size,
                thin: request.as_thin(),
                share: Protocol::None,
                managed: true,
                owners: ReplicaOwners::from_volume(&request.uuid),
                allowed_hosts: vec![],
                kind: None,
            }
        })
        .collect::<Vec<_>>())
}

/// Return a list of appropriate requests which can be used to create a a replica on a pool.
/// This can be used when creating a volume.
pub(crate) async fn create_volume_replicas(
    registry: &Registry,
    request: &CreateVolume,
    volume: &VolumeSpec,
) -> Result<CreateReplicaCandidate, SvcError> {
    // Create a ag guard to prevent candidate collision.
    let ag_guard = match registry.specs().get_or_create_affinity_group(volume) {
        Some(ag) => Some(ag.operation_guard_wait().await?),
        _ => None,
    };

    if !request.allowed_nodes().is_empty()
        && request.replicas > request.allowed_nodes().len() as u64
    {
        // oops, how would this even work mr requester?
        return Err(SvcError::InvalidArguments {});
    }

    let node_replicas = volume_replica_candidates(registry, volume).await?;

    if request.replicas > node_replicas.len() as u64 {
        Err(SvcError::from(NotEnough::OfPools {
            have: node_replicas.len() as u64,
            need: request.replicas,
        }))
    } else {
        Ok(CreateReplicaCandidate::new(node_replicas, ag_guard))
    }
}

/// Get all usable healthy replicas for volume nexus creation.
/// If no usable replica is available, return an error.
pub(crate) async fn healthy_volume_replicas(
    spec: &VolumeSpec,
    target_node: &NodeId,
    registry: &Registry,
) -> Result<HealthyChildItems, SvcError> {
    let children = scheduling::healthy_volume_replicas(
        &GetPersistedNexusChildren::new_create(spec, target_node),
        registry,
    )
    .await?;

    spec.trace(&format!(
        "Healthy volume nexus replicas for volume: {children:?}"
    ));

    if children.is_empty() {
        Err(SvcError::NoOnlineReplicas {
            id: spec.uuid_str(),
        })
    } else {
        Ok(children)
    }
}

/// Check if any replica is on a pool that doesn't have sufficient space for
/// resize operation. If no such replica present, it means the volume is good
/// to be resized and the returned vector will be of zero length.
pub(crate) async fn resizeable_replicas(
    spec: &VolumeSpec,
    registry: &Registry,
    requested_size: u64,
) -> Result<Vec<Replica>, SvcError> {
    if spec.size >= requested_size {
        return Err(SvcError::VolumeResizeSize {
            vol_id: spec.uuid_str(),
            requested_size,
            current_size: spec.size,
        });
    }
    let spec_replicas = registry.specs().volume_replicas(spec.uid());
    let resizable_replicas =
        scheduling::resizeable_replicas(spec, registry, requested_size - spec.size).await;

    // All the replicas of the volume should be resizable, else we don't proceed with
    // the volume resize.
    if resizable_replicas.len() != spec.num_replicas as usize {
        return Err(SvcError::ResizeReplError {
            replica_ids: spec_replicas
                .into_iter()
                .filter(|sr| resizable_replicas.iter().all(|r| &r.uuid != sr.uuid()))
                .map(|excl_repl| excl_repl.uuid().to_string())
                .collect(),
            required: requested_size - spec.size,
        });
    }

    Ok(resizable_replicas)
}

/// Implementation of the ResourceSpecs which is retrieved from the ResourceSpecsLocked.
/// During these calls, no other thread can add/remove elements from the list.
impl ResourceSpecs {
    /// Gets all VolumeSpec's
    pub(crate) fn volumes(&self) -> Vec<VolumeSpec> {
        self.volumes.values().map(|v| v.lock().clone()).collect()
    }

    /// Gets all VolumeSnapshot Specs.
    pub(crate) fn snapshots(&self) -> Vec<VolumeSnapshot> {
        self.volume_snapshots
            .values()
            .map(|v| v.lock().clone())
            .collect()
    }

    /// Gets all VolumeSnapshot Specs, filtered by volume id.
    pub(crate) fn snapshots_by_vol(&self, filter_by: &VolumeId) -> Vec<VolumeSnapshot> {
        self.volume_snapshots
            .values()
            .filter_map(|v| {
                if v.immutable_ref().spec().source_id() == filter_by {
                    Some(v.lock().clone())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Gets all AffinityGroupSpecs.
    pub(crate) fn affinity_groups(&self) -> Vec<AffinityGroupSpec> {
        self.affinity_groups
            .values()
            .map(|v| v.lock().clone())
            .collect()
    }

    /// Get a subset of the volumes based on the pagination argument.
    pub(crate) fn paginated_volumes(&self, pagination: &Pagination) -> PaginatedResult<VolumeSpec> {
        let num_volumes = self.volumes.len() as u64;
        let max_entries = pagination.max_entries();
        let offset = std::cmp::min(pagination.starting_token(), num_volumes);
        let mut last_result = false;
        let length = match offset + max_entries >= num_volumes {
            true => {
                last_result = true;
                num_volumes - offset
            }
            false => pagination.max_entries(),
        };

        PaginatedResult::new(self.volumes.paginate(offset, length), last_result)
    }

    pub(crate) fn paginated_snapshots(
        &self,
        pagination: &Pagination,
        vol_id: Option<&VolumeId>,
    ) -> PaginatedResult<VolumeSnapshot> {
        let mut last_result = false;
        let num_snaps = self.volume_snapshots.len() as u64;
        let max_entries = pagination.max_entries();
        let offset = std::cmp::min(pagination.starting_token(), num_snaps);

        let length = match offset + max_entries >= num_snaps {
            true => {
                last_result = true;
                num_snaps - offset
            }
            false => pagination.max_entries(),
        };

        if let Some(vol_id) = vol_id {
            // We need to filter the resource map based on volume id and apply the pagination
            // parameters on that.
            PaginatedResult::new(
                self.volume_snapshots
                    .paginate_filter(offset, length, |s| s.lock().spec().source_id() == vol_id),
                last_result,
            )
        } else {
            // Use complete resource map for pagination, without any filtering.
            PaginatedResult::new(self.volume_snapshots.paginate(offset, length), last_result)
        }
    }

    /// Get an iterator for all the replicas owned by the given volume.
    pub(crate) fn volume_replicas_it<'a>(
        &'a self,
        id: &'a VolumeId,
    ) -> impl Iterator<Item = &'a ResourceMutex<ReplicaSpec>> + Clone + std::fmt::Debug {
        self.replicas
            .values()
            .filter(|r| r.lock().owners.owned_by(id))
    }
}

impl ResourceSpecsLocked {
    /// Get the resourced VolumeSpec for the given volume `id`, if any exists.
    pub(crate) fn volume_rsc(&self, id: &VolumeId) -> Option<ResourceMutex<VolumeSpec>> {
        let specs = self.read();
        specs.volumes.get(id).cloned()
    }

    /// Get a copy of the VolumeSpec for the volume with the given ID.
    pub(crate) fn volume_clone(&self, id: &VolumeId) -> Result<VolumeSpec, SvcError> {
        match self.volume_rsc(id) {
            Some(locked_spec) => {
                let spec = locked_spec.lock();
                Ok(spec.clone())
            }
            None => Err(VolumeNotFound {
                vol_id: id.to_string(),
            }),
        }
    }
    /// Get a guarded VolumeSpec for the volume with the given ID.
    pub(crate) async fn volume(
        &self,
        id: &VolumeId,
    ) -> Result<OperationGuardArc<VolumeSpec>, SvcError> {
        match self.volume_rsc(id) {
            Some(spec) => spec.operation_guard_wait().await,
            None => Err(VolumeNotFound {
                vol_id: id.to_string(),
            }),
        }
    }

    /// Get the AffinityGroupSpec for the given Affinity Group id.
    pub(crate) fn affinity_group_spec(
        &self,
        id: &AffinityGroupId,
    ) -> Result<AffinityGroupSpec, SvcError> {
        let specs = self.read();
        match specs.affinity_groups.get(id) {
            None => Err(SvcError::AffinityGroupNotFound {
                vol_grp_id: id.to_string(),
            }),
            Some(vol_grp) => {
                let spec = vol_grp.lock();
                Ok(spec.clone())
            }
        }
    }

    /// Gets a copy of all VolumeSnapshot Specs.
    pub(crate) fn snapshots(&self) -> Vec<VolumeSnapshot> {
        let specs = self.read();
        specs.snapshots()
    }

    /// Gets a copy of all VolumeSnapshot Specs, filtered by volume id.
    pub(crate) fn snapshots_by_vol(&self, filter_by: &VolumeId) -> Vec<VolumeSnapshot> {
        let specs = self.read();
        specs.snapshots_by_vol(filter_by)
    }

    /// Gets a copy of all VolumeSpec's.
    pub(crate) fn volumes(&self) -> Vec<VolumeSpec> {
        let specs = self.read();
        specs.volumes()
    }

    /// Get a subset of volumes based on the pagination argument.
    pub(crate) fn paginated_volumes(&self, pagination: &Pagination) -> PaginatedResult<VolumeSpec> {
        let specs = self.read();
        specs.paginated_volumes(pagination)
    }

    /// Get a subset of volumes based on the pagination argument.
    pub(crate) fn paginated_snapshots(
        &self,
        pagination: &Pagination,
        vol: Option<&VolumeId>,
    ) -> PaginatedResult<VolumeSnapshot> {
        let specs = self.read();
        specs.paginated_snapshots(pagination, vol)
    }

    /// Gets a copy of all locked VolumeSpec's.
    pub(crate) fn volumes_rsc(&self) -> Vec<ResourceMutex<VolumeSpec>> {
        let specs = self.read();
        specs.volumes.to_vec()
    }

    /// Get a list of nodes currently used as replicas.
    pub(crate) fn volume_data_nodes(&self, id: &VolumeId) -> Vec<NodeId> {
        let used_pools = self
            .read()
            .replicas
            .values()
            .filter(|r| r.lock().owners.owned_by(id))
            .map(|r| r.lock().pool.pool_name().clone())
            .collect::<Vec<_>>();
        self.read()
            .pools()
            .iter()
            .filter(|p| used_pools.iter().any(|up| up == &p.id))
            .map(|p| p.node.clone())
            .collect::<Vec<_>>()
    }

    /// Get a list of resourced ReplicaSpec's for the given volume `id`.
    /// todo: we could also get the replicas from the volume nexuses?
    pub(crate) fn volume_replicas(&self, id: &VolumeId) -> Vec<ResourceMutex<ReplicaSpec>> {
        self.read()
            .replicas
            .values()
            .filter(|r| r.lock().owners.owned_by(id))
            .cloned()
            .collect()
    }

    /// Get a list of cloned volume replicas owned by the given volume `id`.
    pub(crate) fn volume_replicas_cln(&self, id: &VolumeId) -> Vec<ReplicaSpec> {
        self.read()
            .replicas
            .values()
            .filter_map(|r| {
                let r = r.lock();
                if r.owners.owned_by(id) {
                    Some(r.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get the `NodeId` where `replica` lives.
    pub(crate) async fn replica_node(registry: &Registry, replica: &ReplicaSpec) -> Option<NodeId> {
        Self::pool_node(registry, replica.pool.pool_name()).await
    }

    /// Get the `NodeId` where `pool` lives.
    pub(crate) async fn pool_node(registry: &Registry, pool: &PoolId) -> Option<NodeId> {
        registry.pool_node(pool).await
    }

    /// Get a list of resourced NexusSpecs's which are owned by the given volume `id`
    /// but may not be active anymore.
    /// This may happen if the connection to the persistent store is lost and we fail to
    /// update/delete the nexus spec and the control plane restarts.
    /// To get the current active volume nexus target use `get_volume_target_nexus`.
    pub(crate) fn volume_nexuses(&self, id: &VolumeId) -> Vec<ResourceMutex<NexusSpec>> {
        self.read()
            .nexuses
            .values()
            .filter(|n| n.lock().owner.as_ref() == Some(id))
            .cloned()
            .collect()
    }

    /// Get a list of resourced NexusSpecs's which are associated with the given volume `id`
    /// and are currently in shutdown state.
    pub(crate) async fn volume_shutdown_nexuses(
        &self,
        id: &VolumeId,
    ) -> Vec<ResourceMutex<NexusSpec>> {
        self.read()
            .nexuses
            .values()
            .filter(|nexus| {
                let nexus_spec = nexus.lock();
                nexus_spec.name == id.as_str() && nexus_spec.is_shutdown()
            })
            .cloned()
            .collect()
    }

    /// Get a list of resourced NexusSpecs's which are associated with the given volume `id`
    /// which have shutdown failed.
    pub(crate) async fn volume_failed_shutdown_nexuses(
        &self,
        id: &VolumeId,
    ) -> Vec<ResourceMutex<NexusSpec>> {
        self.read()
            .nexuses
            .values()
            .filter(|nexus| {
                let nexus_spec = nexus.lock();
                nexus_spec.name == id.as_str() && nexus_spec.status_info().shutdown_failed()
            })
            .cloned()
            .collect()
    }

    /// Get a list of resourced NexusSpecs's which have failed to shut down.
    pub(crate) async fn failed_shutdown_nexuses(&self) -> Vec<ResourceMutex<NexusSpec>> {
        self.read()
            .nexuses
            .values()
            .filter(|nexus| {
                let nexus_spec = nexus.lock();
                nexus_spec.is_shutdown() && nexus_spec.status_info().shutdown_failed()
            })
            .cloned()
            .collect()
    }

    /// Get the resourced volume nexus target for the given volume.
    pub(crate) fn volume_target_nexus_rsc(
        &self,
        volume: &VolumeSpec,
    ) -> Option<ResourceMutex<NexusSpec>> {
        match volume.target() {
            None => None,
            Some(target) => self.nexus_rsc(target.nexus()),
        }
    }
    /// Get the resourced volume nexus target for the given volume.
    pub(crate) async fn volume_target_nexus(
        &self,
        volume: &VolumeSpec,
    ) -> Result<Option<OperationGuardArc<NexusSpec>>, SvcError> {
        Ok(match volume.target() {
            None => None,
            Some(target) => self.nexus_opt(target.nexus()).await?,
        })
    }

    /// Delete the NexusInfo key from the persistent store.
    /// If deletion fails we just log it and continue.
    pub(crate) async fn delete_nexus_info(key: &NexusInfoKey, registry: &Registry) {
        let vol_id = match key.volume_id() {
            Some(v) => v.as_str(),
            None => "",
        };
        match registry.delete_kv(&key.key()).await {
            Ok(_) => {
                tracing::trace!(
                    volume.uuid = %vol_id,
                    nexus.uuid = %key.nexus_id(),
                    "Deleted NexusInfo entry from persistent store",
                );
            }
            Err(error) => {
                tracing::error!(
                    %error,
                    volume.uuid = %vol_id,
                    nexus.uuid = %key.nexus_id(),
                    "Failed to delete NexusInfo entry from persistent store",
                );
            }
        }
    }

    /// Remove volume by its `id`.
    pub(super) fn remove_volume(&self, id: &VolumeId) {
        let mut specs = self.write();
        specs.volumes.remove(id);
    }
    /// Remove volume snapshot by its `id`.
    pub(super) fn remove_volume_snapshot(&self, id: &SnapshotId) {
        let mut specs = self.write();
        specs.volume_snapshots.remove(id);
    }

    /// Remove Affinity Group by its `id` only if the volume list becomes empty.
    pub(super) fn remove_affinity_group(&self, id: &VolumeId, ag_id: &String) {
        let mut specs = self.write();
        if let Some(ag_spec) = specs.affinity_groups.get(ag_id).cloned() {
            let mut ag_spec = ag_spec.lock();
            ag_spec.remove(id);
            if ag_spec.is_empty() {
                specs.affinity_groups.remove(ag_id);
            }
        }
    }

    /// Get or Create the resourced AffinityGroupSpec for the given request.
    pub(crate) fn get_or_create_affinity_group(
        &self,
        volume_spec: &VolumeSpec,
    ) -> Option<ResourceMutex<AffinityGroupSpec>> {
        volume_spec.affinity_group.as_ref().map(|ag_info| {
            let mut specs = self.write();
            if let Some(ag_spec) = specs.affinity_groups.get(ag_info.id()) {
                ag_spec.lock().append(volume_spec.uuid.clone());
                ag_spec.clone()
            } else {
                let ag_spec = specs.affinity_groups.insert(AffinityGroupSpec::new(
                    ag_info.id().clone(),
                    vec![volume_spec.uuid.clone()],
                ));
                ag_spec
            }
        })
    }

    /// Get or Create the resourced AffinityGroupSpec for the given request.
    pub(crate) fn get_affinity_group(
        &self,
        vol_grp_id: &AffinityGroupId,
    ) -> Option<ResourceMutex<AffinityGroupSpec>> {
        let specs = self.read();
        specs.affinity_groups.get(vol_grp_id).cloned()
    }

    /// Get a guarded VolumeSnapshot for the snapshot with the given ID.
    pub(crate) async fn volume_snapshot(
        &self,
        id: &SnapshotId,
    ) -> Result<OperationGuardArc<VolumeSnapshot>, SvcError> {
        match self.volume_snapshot_rsc(id) {
            Some(spec) => spec.operation_guard_wait().await,
            None => Err(VolSnapshotNotFound {
                snap_id: id.to_string(),
                source_id: None,
            }),
        }
    }

    /// Get or Create the resourced VolumeSnapshot for the given request.
    pub(crate) fn volume_snapshot_rsc(
        &self,
        snapshot_id: &SnapshotId,
    ) -> Option<ResourceMutex<VolumeSnapshot>> {
        let specs = self.read();
        specs.volume_snapshots.get(snapshot_id).cloned()
    }

    /// Gets a copy of all resourced VolumeSnapshots.
    pub(crate) fn volume_snapshots_rsc(&self) -> Vec<ResourceMutex<VolumeSnapshot>> {
        let specs = self.read();
        specs.volume_snapshots.to_vec()
    }

    /// Get the list of snapshots that are in creating state by its source.
    pub(crate) fn creating_snapshots_by_volume(
        &self,
        volume_id: &VolumeId,
    ) -> Vec<ResourceMutex<VolumeSnapshot>> {
        let specs = self.read();
        specs
            .volume_snapshots
            .values()
            .filter(|s| {
                let locked_spec = s.lock();
                locked_spec.status().creating() && locked_spec.spec().source_id() == volume_id
            })
            .cloned()
            .collect()
    }

    /// Get or Create the resourced VolumeSpec for the given request.
    pub(crate) fn get_or_create_volume(
        &self,
        request: &CreateVolumeSource,
    ) -> Result<ResourceMutex<VolumeSpec>, SvcError> {
        let mut specs = self.write();
        if let Some(volume) = specs.volumes.get(&request.source().uuid) {
            Ok(volume.clone())
        } else {
            // if request has a capacity limit, add up the volumes and reject
            // if the capacity limit would be exceeded
            match request.source().cluster_capacity_limit {
                None => {} // no limit, no check needed
                Some(limit) => {
                    let mut total: u64 = specs.volumes.values().map(|v| v.lock().size).sum();
                    total += request.source().size;
                    if total > limit {
                        return Err(SvcError::CapacityLimitExceeded {
                            cluster_capacity_limit: limit,
                            excess: total - limit,
                        });
                    }
                }
            }
            Ok(match request {
                CreateVolumeSource::None(_) => {
                    specs.volumes.insert(VolumeSpec::from(request.source()))
                }
                CreateVolumeSource::Snapshot(create_from_snap) => {
                    let mut spec = VolumeSpec::from(request.source());
                    spec.set_content_source(Some(create_from_snap.to_snapshot_source()));
                    specs.volumes.insert(spec)
                }
            })
        }
    }

    pub(crate) fn check_capacity_limit_for_resize(
        &self,
        cluster_capacity_limit: u64,
        mut capacity_limit: parking_lot::MutexGuard<u64>,
        required: u64,
    ) -> Result<(), SvcError> {
        let specs = self.write();
        let total: u64 = specs.volumes.values().map(|v| v.lock().size).sum();
        let forthcoming_total = *capacity_limit + total + required;
        tracing::trace!(current_borrowed_limit=%capacity_limit, total=%total, forthcoming_total=%forthcoming_total, "Cluster capacity limit checks");
        if forthcoming_total > cluster_capacity_limit {
            return Err(SvcError::CapacityLimitExceeded {
                cluster_capacity_limit,
                excess: forthcoming_total - cluster_capacity_limit,
            });
        }
        *capacity_limit += required;
        Ok(())
    }

    /// Worker that reconciles dirty VolumeSpecs's with the persistent store.
    /// This is useful when volume operations are performed but we fail to
    /// update the spec with the persistent store.
    pub(crate) async fn reconcile_dirty_volumes(&self, registry: &Registry) -> bool {
        let mut pending_ops = false;

        let volumes = self.volumes_rsc();
        for volume_spec in volumes {
            if let Ok(mut guard) = volume_spec.operation_guard() {
                if !guard.handle_incomplete_ops(registry).await {
                    // Not all pending operations could be handled.
                    pending_ops = true;
                }
            }
        }
        pending_ops
    }

    /// Worker that reconciles dirty VolumeSnapshot's with the persistent store.
    /// This is useful when snapshot operations are performed but we fail to
    /// update the spec with the persistent store.
    pub(crate) async fn reconcile_dirty_volume_snapshots(&self, registry: &Registry) -> bool {
        let mut pending_ops = false;

        for snapshot in self.volume_snapshots_rsc() {
            if let Ok(mut guard) = snapshot.operation_guard() {
                if !guard
                    .handle_incomplete_ops_ext(registry, OnCreateFail::LeaveAsIs)
                    .await
                {
                    // Not all pending operations could be handled.
                    pending_ops = true;
                }
            }
        }
        pending_ops
    }

    /// Get the list of nodes where the replicas of the volume are currently placed.
    pub(crate) fn volume_replica_nodes(&self, volume_id: &VolumeId) -> Vec<NodeId> {
        let specs = self.read();

        // Map the replica's pool to the node and return the list of nodes.
        let replicas_ref = specs.replicas.values();
        replicas_ref
            .filter_map(|replica| {
                let replica = replica.lock();
                replica
                    .owned_by(volume_id)
                    .then_some(
                        specs
                            .pools
                            .get(replica.pool_name())
                            .map(|p| p.lock().node.clone()),
                    )
                    .flatten()
            })
            .collect()
    }

    /// Get or Create the resourced VolumeSnapshot for the given request.
    pub(crate) fn get_or_create_snapshot(
        &self,
        request: &VolumeSnapshotUserSpec,
    ) -> ResourceMutex<VolumeSnapshot> {
        let mut specs = self.write();
        if let Some(snapshot) = specs.volume_snapshots.get(request.uuid()) {
            snapshot.clone()
        } else {
            specs.volume_snapshots.insert(VolumeSnapshot::from(request))
        }
    }
}

#[async_trait::async_trait]
impl GuardedOperationsHelper for OperationGuardArc<VolumeSpec> {
    type Create = CreateVolume;
    type Owners = ();
    type Status = VolumeStatus;
    type State = VolumeState;
    type UpdateOp = VolumeOperation;
    type Inner = VolumeSpec;

    fn remove_spec(&self, registry: &Registry) {
        let uuid = self.lock().uuid.clone();
        registry.specs().remove_volume(&uuid);
        let ag_info = self.lock().affinity_group.clone();
        if let Some(ag) = ag_info {
            registry.specs().remove_affinity_group(&uuid, ag.id())
        }
    }
}

#[async_trait::async_trait]
impl SpecOperationsHelper for VolumeSpec {
    type Create = CreateVolume;
    type Owners = ();
    type Status = VolumeStatus;
    type State = VolumeState;
    type UpdateOp = VolumeOperation;

    async fn start_update_op(
        &mut self,
        registry: &Registry,
        state: &Self::State,
        operation: Self::UpdateOp,
    ) -> Result<(), SvcError> {
        if !matches!(
            &operation,
            VolumeOperation::Publish(..)
                | VolumeOperation::Unpublish
                | VolumeOperation::Republish(..)
                | VolumeOperation::CreateSnapshot(..)
                | VolumeOperation::DestroySnapshot(..)
        ) {
            // don't attempt to modify the volume parameters if the nexus target is not "stable"
            if self.target().is_some() != state.target.is_some() {
                return Err(SvcError::NotReady {
                    kind: self.kind(),
                    id: self.uuid_str(),
                });
            }
        }

        match &operation {
            VolumeOperation::Share(protocol) => match protocol {
                VolumeShareProtocol::Nvmf => match &self.target() {
                    None => Err(SvcError::VolumeNotPublished {
                        vol_id: self.uuid_str(),
                    }),
                    Some(target) => match target.protocol() {
                        None => Ok(()),
                        Some(protocol) => Err(SvcError::AlreadyShared {
                            kind: self.kind(),
                            id: self.uuid_str(),
                            share: protocol.to_string(),
                        }),
                    },
                },
                VolumeShareProtocol::Iscsi => Err(SvcError::InvalidShareProtocol {
                    kind: ResourceKind::Volume,
                    id: self.uuid_str(),
                    share: format!("{protocol:?}"),
                }),
            },
            VolumeOperation::Unshare => match self.target() {
                None => Err(SvcError::NotShared {
                    kind: self.kind(),
                    id: self.uuid_str(),
                }),
                Some(target) if target.protocol().is_none() => Err(SvcError::NotShared {
                    kind: self.kind(),
                    id: self.uuid_str(),
                }),
                _ => Ok(()),
            },
            VolumeOperation::PublishOld(_) => Err(SvcError::InvalidArguments {}),
            VolumeOperation::Publish(args) => match args.protocol() {
                None => Ok(()),
                Some(protocol) => match protocol {
                    VolumeShareProtocol::Nvmf => {
                        if let Some(target) = self.target() {
                            Err(SvcError::VolumeAlreadyPublished {
                                vol_id: self.uuid_str(),
                                node: target.node().to_string(),
                                protocol: format!("{:?}", target.protocol()),
                            })
                        } else {
                            self.publish_context = Some(args.publish_context());
                            Ok(())
                        }
                    }
                    VolumeShareProtocol::Iscsi => Err(SvcError::InvalidShareProtocol {
                        kind: ResourceKind::Volume,
                        id: self.uuid_str(),
                        share: format!("{:?}", args.protocol()),
                    }),
                },
            },
            VolumeOperation::Republish(args) => match args.protocol() {
                VolumeShareProtocol::Nvmf => Ok(()),
                VolumeShareProtocol::Iscsi => Err(SvcError::InvalidShareProtocol {
                    kind: ResourceKind::Volume,
                    id: self.uuid_str(),
                    share: format!("{:?}", args.protocol()),
                }),
            },
            VolumeOperation::Unpublish if self.target().is_none() => {
                Err(SvcError::VolumeNotPublished {
                    vol_id: self.uuid_str(),
                })
            }
            VolumeOperation::Unpublish => {
                self.publish_context = None;
                Ok(())
            }

            VolumeOperation::SetReplica(replica_count) => {
                if *replica_count == self.num_replicas {
                    Err(SvcError::ReplicaCountAchieved {
                        id: self.uuid_str(),
                        count: self.num_replicas,
                    })
                } else if *replica_count < 1 {
                    Err(SvcError::LastReplica {
                        replica: "".to_string(),
                        volume: self.uuid_str(),
                    })
                } else if (*replica_count as i16 - self.num_replicas as i16).abs() > 1 {
                    Err(SvcError::ReplicaChangeCount {})
                } else if state.status != VolumeStatus::Online
                    && (*replica_count > self.num_replicas)
                {
                    Err(SvcError::ReplicaIncrease {
                        volume_id: self.uuid_str(),
                        volume_state: state.status.to_string(),
                    })
                } else {
                    // Validation for Affinity Group volume's replica count cannot go below 2.
                    if self.affinity_group.is_some() && *replica_count < 2 {
                        Err(SvcError::RestrictedReplicaCount {
                            resource: ResourceKind::AffinityGroup,
                            count: *replica_count,
                        })
                    } else if *replica_count > self.num_replicas && self.has_snapshots() {
                        let fix = NodeBugFix::NexusRebuildReplicaAncestry;
                        registry.volume_replica_nodes_fix(self, &fix)
                    } else {
                        Ok(())
                    }
                }
            }
            VolumeOperation::RemoveUnusedReplica(uuid) => {
                let last_replica = !registry
                    .specs()
                    .volume_replicas(&self.uuid)
                    .iter()
                    .any(|r| &r.lock().uuid != uuid);
                let nexus = registry.specs().volume_target_nexus_rsc(self);
                let used = nexus.map(|n| n.lock().contains_replica(uuid));
                if last_replica {
                    Err(SvcError::LastReplica {
                        replica: uuid.to_string(),
                        volume: self.uuid_str(),
                    })
                } else if used.unwrap_or_default() {
                    Err(SvcError::InUse {
                        kind: ResourceKind::Replica,
                        id: uuid.to_string(),
                    })
                } else {
                    match registry
                        .nexus_info(Some(&self.uuid), self.health_info_id(), true)
                        .await?
                    {
                        Some(info) => match info
                            .children
                            .iter()
                            .find(|i| i.uuid.as_str() == uuid.as_str())
                        {
                            Some(replica_info)
                                if replica_info.healthy
                                    && !info
                                        .children
                                        .iter()
                                        .filter(|i| i.uuid.as_str() != uuid.as_str())
                                        .any(|i| i.healthy) =>
                            {
                                // if there are no other healthy replicas, then we cannot remove
                                // this replica!
                                Err(SvcError::LastHealthyReplica {
                                    replica: uuid.to_string(),
                                    volume: self.uuid_str(),
                                })
                            }
                            _ => Ok(()),
                        },
                        None => Ok(()),
                    }
                }
            }
            VolumeOperation::Create => unreachable!(),
            VolumeOperation::Destroy => unreachable!(),
            VolumeOperation::CreateSnapshot(_) => Ok(()),
            VolumeOperation::DestroySnapshot(_) => Ok(()),
            VolumeOperation::Resize(_) => Ok(()),
            VolumeOperation::SetVolumeProperty(_) => Ok(()),
        }?;
        self.start_op(operation);
        Ok(())
    }
    fn start_create_op(&mut self, _request: &Self::Create) {
        self.start_op(VolumeOperation::Create);
    }
    fn start_destroy_op(&mut self) {
        self.start_op(VolumeOperation::Destroy);
    }
    fn dirty(&self) -> bool {
        self.has_pending_op()
    }
    fn kind(&self) -> ResourceKind {
        ResourceKind::Volume
    }
    fn uuid_str(&self) -> String {
        self.uuid.to_string()
    }
    fn status(&self) -> SpecStatus<Self::Status> {
        self.status.clone()
    }
    fn set_status(&mut self, status: SpecStatus<Self::Status>) {
        self.status = status;
    }
    fn operation_result(&self) -> Option<Option<bool>> {
        self.operation.as_ref().map(|r| r.result)
    }
}
