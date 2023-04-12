use crate::{
    controller::{
        registry::Registry,
        resources::{
            operations_helper::{
                GuardedOperationsHelper, OperationSequenceGuard, ResourceSpecs,
                ResourceSpecsLocked, SpecOperationsHelper,
            },
            OperationGuardArc, ResourceMutex, TraceSpan, TraceStrLog,
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
    volume::scheduling,
};
use agents::{
    errors,
    errors::{NotEnough, SvcError, SvcError::VolumeNotFound},
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
            volume::{VolumeGroupSpec, VolumeOperation, VolumeSpec},
            SpecStatus, SpecTransaction,
        },
        transport::{
            CreateReplica, CreateVolume, NodeId, PoolId, Protocol, ReplicaId, ReplicaName,
            ReplicaOwners, VolumeId, VolumeShareProtocol, VolumeState, VolumeStatus,
        },
    },
};

use snafu::OptionExt;
use std::convert::From;

/// CreateReplicaCandidate for volume and volume group.
pub(crate) struct CreateReplicaCandidate {
    candidates: Vec<CreateReplica>,
    _volume_group_guard: Option<OperationGuardArc<VolumeGroupSpec>>,
}

impl CreateReplicaCandidate {
    /// Create a new `CreateReplicaCandidate` with candidates and optional vg guard.
    pub(crate) fn new(
        candidates: Vec<CreateReplica>,
        volume_group_guard: Option<OperationGuardArc<VolumeGroupSpec>>,
    ) -> CreateReplicaCandidate {
        Self {
            candidates,
            _volume_group_guard: volume_group_guard,
        }
    }
    /// Get the candidates.
    pub(crate) fn candidates(&self) -> &Vec<CreateReplica> {
        &self.candidates
    }
}

/// Select a replica to be removed from the volume
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

/// Get replica candidates to be removed from the volume
/// This list includes healthy and non_healthy candidates, so care must be taken to
/// make sure we don't remove "too many healthy" candidates
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

/// Get a list of nexus children to be removed from a nexus
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

/// Get a list of existing candidate volume replicas to attach to a given nexus
/// Useful to attach replicas to a nexus when the number of nexus children does not match
/// the volume's replica count
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
/// replica count
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
                pool_id: p.id.clone(),
                pool_uuid: None,
                size: request.size,
                thin: request.thin,
                share: Protocol::None,
                managed: true,
                owners: ReplicaOwners::from_volume(&request.uuid),
                allowed_hosts: vec![],
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
                pool_id: p.id.clone(),
                pool_uuid: None,
                size: request.size,
                thin: request.thin,
                share: Protocol::None,
                managed: true,
                owners: ReplicaOwners::from_volume(&request.uuid),
                allowed_hosts: vec![],
            }
        })
        .collect::<Vec<_>>())
}

/// Return a list of appropriate requests which can be used to create a a replica on a pool
/// This can be used when creating a volume
pub(crate) async fn create_volume_replicas(
    registry: &Registry,
    request: &CreateVolume,
    volume: &VolumeSpec,
) -> Result<CreateReplicaCandidate, SvcError> {
    // Create a vg guard to prevent candidate collision.
    let vg_guard = match registry.specs().get_or_create_volume_group(request) {
        Some(vg) => Some(vg.operation_guard_wait().await?),
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
        Ok(CreateReplicaCandidate::new(node_replicas, vg_guard))
    }
}

/// Get all usable healthy replicas for volume nexus creation
/// If no usable replica is available, return an error
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

/// Implementation of the ResourceSpecs which is retrieved from the ResourceSpecsLocked
/// During these calls, no other thread can add/remove elements from the list
impl ResourceSpecs {
    /// Gets all VolumeSpec's
    pub(crate) fn volumes(&self) -> Vec<VolumeSpec> {
        self.volumes.values().map(|v| v.lock().clone()).collect()
    }

    /// Gets all VolumeGroupSpecs.
    pub(crate) fn volume_groups(&self) -> Vec<VolumeGroupSpec> {
        self.volume_groups
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
}
impl ResourceSpecsLocked {
    /// Get the resourced VolumeSpec for the given volume `id`, if any exists
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

    /// Gets a copy of all VolumeSpec's
    pub(crate) fn volumes(&self) -> Vec<VolumeSpec> {
        let specs = self.read();
        specs.volumes()
    }

    /// Get a subset of volumes based on the pagination argument.
    pub(crate) fn paginated_volumes(&self, pagination: &Pagination) -> PaginatedResult<VolumeSpec> {
        let specs = self.read();
        specs.paginated_volumes(pagination)
    }

    /// Gets a copy of all locked VolumeSpec's
    pub(crate) fn volumes_rsc(&self) -> Vec<ResourceMutex<VolumeSpec>> {
        let specs = self.read();
        specs.volumes.to_vec()
    }

    /// Get a list of nodes currently used as replicas
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

    /// Get a list of resourced ReplicaSpec's for the given volume `id`
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

    /// Get the `NodeId` where `replica` lives
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

    /// Get the resourced volume nexus target for the given volume
    pub(crate) fn volume_target_nexus_rsc(
        &self,
        volume: &VolumeSpec,
    ) -> Option<ResourceMutex<NexusSpec>> {
        match volume.target() {
            None => None,
            Some(target) => self.nexus_rsc(target.nexus()),
        }
    }
    /// Get the resourced volume nexus target for the given volume
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

    /// Remove volume by its `id`
    pub(super) fn remove_volume(&self, id: &VolumeId) {
        let mut specs = self.write();
        specs.volumes.remove(id);
    }

    /// Remove volume group by its `id` only if the volume list becomes empty.
    pub(super) fn remove_volume_group(&self, id: &VolumeId, vg_id: &String) {
        let mut specs = self.write();
        if let Some(vg_spec) = specs.volume_groups.get(vg_id).cloned() {
            let mut vg_spec = vg_spec.lock();
            vg_spec.remove(id);
            if vg_spec.is_empty() {
                specs.volume_groups.remove(vg_id);
            }
        }
    }

    /// Get or Create the resourced VolumeGroupSpec for the given request.
    pub(crate) fn get_or_create_volume_group(
        &self,
        request: &CreateVolume,
    ) -> Option<ResourceMutex<VolumeGroupSpec>> {
        request.volume_group.as_ref().map(|vg_info| {
            let mut specs = self.write();
            if let Some(vg_spec) = specs.volume_groups.get(vg_info.id()) {
                vg_spec.lock().append(request.uuid.clone());
                vg_spec.clone()
            } else {
                let vg_spec = specs.volume_groups.insert(VolumeGroupSpec::new(
                    vg_info.id().clone(),
                    vec![request.uuid.clone()],
                ));
                vg_spec
            }
        })
    }

    /// Get or Create the resourced VolumeSpec for the given request
    pub(crate) fn get_or_create_volume(&self, request: &CreateVolume) -> ResourceMutex<VolumeSpec> {
        let mut specs = self.write();
        if let Some(volume) = specs.volumes.get(&request.uuid) {
            volume.clone()
        } else {
            specs.volumes.insert(VolumeSpec::from(request))
        }
    }

    /// Worker that reconciles dirty VolumeSpecs's with the persistent store.
    /// This is useful when nexus operations are performed but we fail to
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
        let vg_info = self.lock().volume_group.clone();
        if let Some(vg) = vg_info {
            registry.specs().remove_volume_group(&uuid, vg.id())
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
                    Ok(())
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
        }?;
        self.start_op(operation);
        Ok(())
    }
    fn start_create_op(&mut self) {
        self.start_op(VolumeOperation::Create);
    }
    fn start_destroy_op(&mut self) {
        self.start_op(VolumeOperation::Destroy);
    }
    fn dirty(&self) -> bool {
        self.pending_op()
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
