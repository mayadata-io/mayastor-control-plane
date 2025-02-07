use crate::{
    controller::{
        reconciler::PollTriggerEvent,
        registry::Registry,
        resources::{
            operations::{
                ResourceLifecycle, ResourceLifecycleExt, ResourceLifecycleWithLifetime,
                ResourceOwnerUpdate, ResourceProperty, ResourcePublishing, ResourceReplicas,
                ResourceResize, ResourceSharing, ResourceShutdownOperations,
            },
            operations_helper::{
                GuardedOperationsHelper, OnCreateFail, OperationSequenceGuard, ResourceSpecsLocked,
                SpecOperationsHelper,
            },
            OperationGuardArc, ResourceUid, TraceSpan, TraceStrLog,
        },
        scheduling::pool::ENoSpcReplica,
    },
    volume::{
        clone_operations::SnapshotCloneOp,
        snapshot_operations::DestroyVolumeSnapshotRequest,
        specs::{
            create_volume_replicas, healthy_volume_replicas, resizeable_replicas,
            volume_move_replica_candidates, CreateReplicaCandidate,
        },
    },
};
use agents::errors::SvcError;
use stor_port::{
    transport_api::ErrorChain,
    types::v0::{
        store::{
            nexus_persistence::NexusInfoKey,
            replica::ReplicaSpec,
            volume::{PublishOperation, RepublishOperation, VolumeOperation, VolumeSpec},
        },
        transport::{
            CreateReplica, CreateVolume, DestroyNexus, DestroyReplica, DestroyShutdownTargets,
            DestroyVolume, NodeTopology, Protocol, PublishVolume, Replica, ReplicaId,
            ReplicaOwners, RepublishVolume, ResizeVolume, SetVolumeProperty, SetVolumeReplica,
            ShareNexus, ShareVolume, ShutdownNexus, UnpublishVolume, UnshareNexus, UnshareVolume,
            Volume,
        },
    },
};

use std::{fmt::Debug, ops::Deref};

#[async_trait::async_trait]
impl ResourceLifecycle for OperationGuardArc<VolumeSpec> {
    type Create = CreateVolume;
    type CreateOutput = Self;
    type Destroy = DestroyVolume;

    async fn create(
        registry: &Registry,
        request: &Self::Create,
    ) -> Result<Self::CreateOutput, SvcError> {
        let request = CreateVolumeSource::None(request);
        OperationGuardArc::<VolumeSpec>::create_ext(registry, &request).await
    }

    /// Destroy a volume based on the given `DestroyVolume` request.
    /// Volume destruction will succeed even if the nexus or replicas cannot be destroyed (i.e. due
    /// to an inaccessible node). In this case the resources will be destroyed by the garbage
    /// collector at a later time.
    async fn destroy(
        &mut self,
        registry: &Registry,
        request: &Self::Destroy,
    ) -> Result<(), SvcError> {
        let specs = registry.specs();
        self.start_destroy(registry).await?;

        let nexuses = specs.volume_nexuses(&request.uuid);
        for nexus_arc in nexuses {
            let nexus = nexus_arc.lock().deref().clone();
            match nexus_arc.operation_guard_wait().await {
                Ok(mut guard) => {
                    let destroy = DestroyNexus::from(&nexus)
                        .with_disown(&request.uuid)
                        .with_lazy(true);
                    if let Err(error) = guard.destroy(registry, &destroy).await {
                        nexus.warn_span(|| {
                            tracing::warn!(
                                error=%error,
                                "Nexus destruction failed. It will be garbage collected later."
                            )
                        });
                    }

                    // Delete the NexusInfo entry persisted by the IoEngine.
                    ResourceSpecsLocked::delete_nexus_info(
                        &NexusInfoKey::new(&Some(request.uuid.clone()), &nexus.uuid),
                        registry,
                    )
                    .await;
                }
                Err(error) => {
                    nexus.warn_span(|| {
                        tracing::warn!(error=%error,
                            "Nexus was busy. It will be garbage collected later."
                        )
                    });
                }
            }
        }

        // When nexus is destroyed ahead of the volume destroy, then
        // delete_nexus_info in previous will not be called since nexus won't be present.
        // So invoke delete_nexus_info explicitly using the nexus id in target_config if present.
        if let Some(config) = self.as_ref().config() {
            let nexus_id = config.target().nexus();
            // Delete the NexusInfo entry persisted by the IoEngine.
            ResourceSpecsLocked::delete_nexus_info(
                &NexusInfoKey::new(&Some(self.uuid().clone()), nexus_id),
                registry,
            )
            .await;
        }

        let replicas = specs.volume_replicas(&request.uuid);
        for replica in replicas {
            let mut replica = match replica.operation_guard_wait().await {
                Ok(replica) => replica,
                Err(_) => continue,
            };
            if let Some(node) = ResourceSpecsLocked::replica_node(registry, replica.as_ref()).await
            {
                let result = replica
                    .destroy(
                        registry,
                        &replica.destroy_request(ReplicaOwners::new_disown_all(), &node),
                    )
                    .await;
                if let Err(error) = result {
                    tracing::warn!(replica.uuid=%replica.uuid(), error=%error,
                        "Replica destruction failed. This will be garbage collected later"
                    );
                }
            } else {
                // The above is able to handle when a pool is moved to a different node but if a
                // pool is unplugged we should disown the replica and allow the garbage
                // collector to destroy it later.
                tracing::warn!(replica.uuid=%replica.uuid(),"Replica node not found");
                let disowner = ReplicaOwners::from_volume(self.uuid());
                if let Err(error) = replica.remove_owners(registry, &disowner, true).await {
                    tracing::error!(replica.uuid=%replica.uuid(), error=%error, "Failed to disown volume replica");
                }
            }
        }

        // Destroy all the snapshots that are in creating state as the source is getting destroyed.
        let pending_creation_snapshots = specs.creating_snapshots_by_volume(self.uuid());
        for snapshot in pending_creation_snapshots {
            let mut snapshot_guard = match snapshot.operation_guard_wait().await {
                Ok(snapshot_guard) => snapshot_guard,
                Err(_) => continue,
            };
            let snapshot_user_spec = snapshot.lock().spec().clone();
            let result = snapshot_guard
                .destroy(
                    registry,
                    &DestroyVolumeSnapshotRequest::new(
                        snapshot,
                        Some(snapshot_user_spec.source_id().clone()),
                        snapshot_user_spec.uuid().clone(),
                    ),
                )
                .await;
            if let Err(error) = result {
                tracing::warn!(snapshot.uuid=%snapshot_guard.uuid(), error=%error,
                    "Snapshot destruction failed. It will be garbage collected later"
                );
            }
        }

        self.complete_destroy(Ok(()), registry).await
    }
}

#[async_trait::async_trait]
impl ResourceResize for OperationGuardArc<VolumeSpec> {
    type Resize = ResizeVolume;
    type ResizeOutput = Volume;

    async fn resize(
        &mut self,
        registry: &Registry,
        request: &Self::Resize,
    ) -> Result<Self::ResizeOutput, SvcError> {
        let spec = self.as_ref().clone();
        let state = registry.volume_state(&request.uuid).await?;

        // If the volume is published, then we need to resize nexus also along with replicas.
        let target_cfg = spec.target();
        let nexus = if let Some(tcfg) = target_cfg {
            Some(registry.specs().nexus(tcfg.nexus()).await?)
        } else {
            // Unpublished volume
            None
        };

        // Pre-check - Ensure pools that host replicas have enough space to resize the replicas,
        // and also ensure that the replicas are Online.
        let resizeable_replicas =
            resizeable_replicas(&spec, registry, request.requested_size).await?;

        let spec_clone = self
            .start_update(
                registry,
                &state,
                VolumeOperation::Resize(request.requested_size),
            )
            .await?;
        // Resize each replica of the volume. If any replica fails to be resized then the
        // volume resize operation is deemed as a failure.
        let result_repl = self
            .resize_volume_replicas(registry, &resizeable_replicas, request.requested_size)
            .await;

        // If we had found a nexus, i.e. the volume is published, we need to go ahead with
        // nexus resize now, but only if replicas have also resized successfully.
        let result_nx = match (result_repl, nexus) {
            (Ok(_), Some(mut nexus_grd)) => {
                self.resize_target(registry, &mut nexus_grd, request.requested_size)
                    .await
            }
            (Err(e), Some(_)) | (Err(e), None) => Err(e),
            (Ok(_), None) => Ok(()),
        };

        // An error code NexusResizeStatusUnknown is an indication at this point that we
        // are uncertain whether nexus bdev underneath got resized or not. However, we'll
        // assume that it has, and hence report volume resize operation as success even
        // though the nexus spec isn't updated with new size yet. The reconciler will
        // find this mismatch between nexus and volume specs, and re-attempt to send
        // nexus resize gRPC to dataplane.
        let final_result = match result_nx {
            Ok(_) => Ok(()),
            Err(SvcError::NexusResizeStatusUnknown { .. }) => Ok(()),
            Err(error) => Err(error),
        };

        self.complete_update(registry, final_result, spec_clone)
            .await?;

        registry.volume(&request.uuid).await
    }
}

#[async_trait::async_trait]
impl ResourceSharing for OperationGuardArc<VolumeSpec> {
    type Share = ShareVolume;
    type Unshare = UnshareVolume;
    type ShareOutput = String;
    type UnshareOutput = ();

    async fn share(
        &mut self,
        registry: &Registry,
        request: &Self::Share,
    ) -> Result<String, SvcError> {
        let specs = registry.specs();
        let state = registry.volume_state(&request.uuid).await?;

        let spec_clone = self
            .start_update(registry, &state, VolumeOperation::Share(request.protocol))
            .await?;

        let target = state.target.expect("already validated");
        let result = match specs.nexus(&target.uuid).await {
            Ok(mut nexus) => {
                nexus
                    .share(
                        registry,
                        &ShareNexus::new(
                            &target,
                            request.protocol,
                            request
                                .frontend_hosts
                                .clone()
                                .into_iter()
                                .map(TryInto::try_into)
                                .collect::<Result<_, _>>()?,
                        ),
                    )
                    .await
            }
            Err(error) => Err(error),
        };

        self.complete_update(registry, result, spec_clone).await
    }

    async fn unshare(
        &mut self,
        registry: &Registry,
        request: &Self::Unshare,
    ) -> Result<Self::UnshareOutput, SvcError> {
        let specs = registry.specs();
        let state = registry.volume_state(&request.uuid).await?;

        let spec_clone = self
            .start_update(registry, &state, VolumeOperation::Unshare)
            .await?;

        let target = state.target.expect("Already validated");
        let result = match specs.nexus(&target.uuid).await {
            Ok(mut nexus) => nexus.unshare(registry, &UnshareNexus::from(&target)).await,
            Err(error) => Err(error),
        };

        self.complete_update(registry, result, spec_clone).await
    }
}

#[async_trait::async_trait]
impl ResourcePublishing for OperationGuardArc<VolumeSpec> {
    type Publish = PublishVolume;
    type PublishOutput = Volume;
    type Unpublish = UnpublishVolume;
    type Republish = RepublishVolume;

    async fn publish(
        &mut self,
        registry: &Registry,
        request: &Self::Publish,
    ) -> Result<Self::PublishOutput, SvcError> {
        let state = registry.volume_state(&request.uuid).await?;
        let nexus_node = self
            .next_target_node(registry, request, &state, false)
            .await?;

        let last_target = self.as_ref().health_info_id().cloned();
        let frontend_nodes = &request.frontend_nodes;
        let target_cfg = self
            .next_target_config(
                registry,
                nexus_node.candidate(),
                &request.share,
                frontend_nodes,
            )
            .await;

        let operation = VolumeOperation::Publish(PublishOperation::new(
            target_cfg.clone(),
            request.publish_context.clone(),
        ));
        let spec_clone = self.start_update(registry, &state, operation).await?;

        // Create a Nexus on the requested or auto-selected node.
        let result = self.create_nexus(registry, &target_cfg).await;

        let (mut nexus, nexus_state) = self
            .validate_update_step(registry, result, &spec_clone)
            .await?;

        // Share the Nexus if it was requested.
        let mut result = Ok(());
        if let Some(share) = request.share {
            let allowed_hosts = target_cfg.frontend().node_nqns();
            result = match nexus
                .share(
                    registry,
                    &ShareNexus::new(&nexus_state, share, allowed_hosts),
                )
                .await
            {
                Ok(_) => Ok(()),
                Err(error) => {
                    // Since we failed to share, we'll revert back to the previous state.
                    // If we fail to do this inline, the reconcilers will pick up the slack.
                    nexus
                        .destroy(registry, &DestroyNexus::from(nexus_state).with_disown_all())
                        .await
                        .ok();
                    Err(error)
                }
            }
        }

        self.complete_update(registry, result, spec_clone).await?;

        // If there was a previous nexus we should delete the persisted NexusInfo structure.
        if let Some(nexus_id) = last_target {
            ResourceSpecsLocked::delete_nexus_info(
                &NexusInfoKey::new(&Some(self.uuid().clone()), &nexus_id),
                registry,
            )
            .await;
        }

        let volume = registry.volume(&request.uuid).await?;
        registry
            .notify_if_degraded(&volume, PollTriggerEvent::VolumeDegraded)
            .await;
        Ok(volume)
    }

    async fn unpublish(
        &mut self,
        registry: &Registry,
        request: &Self::Unpublish,
    ) -> Result<(), SvcError> {
        let specs = registry.specs();

        let state = registry.volume_state(&request.uuid).await?;

        let spec_clone = self
            .start_update(registry, &state, VolumeOperation::Unpublish)
            .await?;

        let volume_target = spec_clone.target().expect("already validated");
        let frontend_nodes = &request.frontend_nodes;
        tracing::debug!("unpublish_volume: frontend nodes {frontend_nodes:?}");
        if !frontend_nodes.is_empty() {
            if let Some(tgt_cfg) = spec_clone.active_config() {
                for unode in frontend_nodes {
                    if !tgt_cfg.frontend().nodename_allowed(unode.as_str()) {
                        self.validate_update_step(
                            registry,
                            Err(SvcError::FrontendNodeNotAllowed {
                                node: unode.to_string(),
                                vol_id: request.uuid.to_string(),
                            }),
                            &spec_clone,
                        )
                        .await?;
                    }
                }
            }
        }

        let result = match specs.nexus_opt(volume_target.nexus()).await? {
            None => Ok(()),
            Some(mut nexus) => {
                let nexus_clone = nexus.lock().clone();
                let destroy = DestroyNexus::from(&nexus_clone).with_disown(&request.uuid);
                // Destroy the Nexus
                match nexus.destroy(registry, &destroy).await {
                    Ok(_) => Ok(()),
                    Err(error) if !request.force() => Err(error),
                    Err(error) => {
                        let node_online = match registry.node_wrapper(&nexus_clone.node).await {
                            Ok(node) => {
                                let mut node = node.write().await;
                                node.is_online() && node.liveness_probe().await.is_ok()
                            }
                            _ => false,
                        };
                        if !node_online {
                            nexus_clone.warn_span(|| {
                                tracing::warn!("Force unpublish. Forgetting about the target nexus because the node is not online and it was requested");
                            });
                            Ok(())
                        } else {
                            Err(error)
                        }
                    }
                }
            }
        };

        self.complete_update(registry, result, spec_clone).await
    }

    async fn republish(
        &mut self,
        registry: &Registry,
        request: &Self::Republish,
    ) -> Result<Self::PublishOutput, SvcError> {
        // If HA is disabled there is no point in switchover.
        if registry.ha_disabled() {
            return Err(SvcError::SwitchoverNotAllowedWhenHAisDisabled {});
        }
        let specs = registry.specs();
        let spec = self.as_ref().clone();
        let state = registry.volume_state(&request.uuid).await?;
        // If the volume is not published then it should issue publish call rather than republish.
        let target_cfg = match spec.active_config() {
            Some(cfg)
                if !cfg
                    .frontend()
                    .nodename_allowed(request.frontend_node.as_str()) =>
            {
                Err(SvcError::FrontendNodeNotAllowed {
                    node: request.frontend_node.to_string(),
                    vol_id: request.uuid.to_string(),
                })
            }
            Some(config) => Ok(config),
            None => Err(SvcError::VolumeNotPublished {
                vol_id: request.uuid.to_string(),
            }),
        }?;

        let mut older_nexus = specs.nexus(target_cfg.target().nexus()).await?;
        let mut move_nexus = true;
        let mut nexus_node = None;
        let healthy_replicas_result =
            healthy_volume_replicas(&spec, &older_nexus.as_ref().node, registry).await;
        let healthy_replicas = healthy_replicas_result.is_ok();
        match healthy_replicas_result {
            Ok(_) => {
                let reuse_existing = match request.reuse_existing_fallback
                    && !request.reuse_existing
                {
                    true => match self.next_target_node(registry, request, &state, true).await {
                        Ok(node) => {
                            nexus_node = Some(Ok(node));
                            false
                        }
                        // use older target as a fallback...
                        Err(error @ SvcError::NotEnoughResources { .. }) => {
                            nexus_node = Some(Err(error));
                            true
                        }
                        Err(error) => return Err(error),
                    },
                    false => request.reuse_existing,
                };
                if reuse_existing
                    && !older_nexus.as_ref().is_shutdown()
                    && older_nexus.missing_nexus_recreate(registry).await.is_ok()
                {
                    move_nexus = false;
                }
            }
            Err(error) => {
                if !older_nexus.as_ref().is_shutdown() {
                    return Err(error);
                }
            }
        }

        if !move_nexus {
            // The older nexus is back again, so completing republish without modifications.
            tracing::info!(nexus.uuid=%older_nexus.as_ref().uuid, "Current target is back online, not moving nexus");
            let volume = registry.volume(&request.uuid).await?;
            return Ok(volume);
        }

        // Get the newer target node for the new nexus creation.
        let nexus_node = match nexus_node {
            Some(result) => result,
            None => self.next_target_node(registry, request, &state, true).await,
        }?;
        let nodes = target_cfg.frontend().node_names();
        let target_cfg = self
            .next_target_config(
                registry,
                nexus_node.candidate(),
                &Some(request.share),
                &nodes,
            )
            .await;
        let operation = VolumeOperation::Republish(RepublishOperation::new(target_cfg.clone()));

        let spec_clone = self.start_update(registry, &state, operation).await?;

        let older_nexus_id = older_nexus.uuid().clone();

        // Shutdown the older nexus before newer nexus creation.
        let result = older_nexus
            .shutdown(
                registry,
                &ShutdownNexus::new(older_nexus_id, true, !healthy_replicas),
            )
            .await;
        self.validate_update_step(registry, result, &spec_clone)
            .await?;

        // Create a Nexus on the requested or auto-selected node.
        let result = self.create_nexus(registry, &target_cfg).await;
        let (mut nexus, nexus_state) = self
            .validate_update_step(registry, result, &spec_clone)
            .await?;
        let allowed_host = target_cfg.frontend().node_nqns();
        // Share the Nexus.
        let result = match nexus
            .share(
                registry,
                &ShareNexus::new(&nexus_state, request.share, allowed_host),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(error) => {
                // Since we failed to share, we'll revert back to the previous state.
                // If we fail to do this inline, the reconcilers will pick up the slack.
                nexus
                    .destroy(registry, &DestroyNexus::from(nexus_state).with_disown_all())
                    .await
                    .ok();
                Err(error)
            }
        };

        self.complete_update(registry, result, spec_clone).await?;

        let volume = registry.volume(&request.uuid).await?;
        registry
            .notify_if_degraded(&volume, PollTriggerEvent::VolumeDegraded)
            .await;
        Ok(volume)
    }
}

/// Request to move the given replica to another pool.
/// May be useful to reclaim space in the current pool to handle thin provisioning.
#[derive(Debug, Clone)]
pub(crate) struct MoveReplicaRequest {
    replica: ReplicaId,
    /// Delete the moved replica after we've created the replacement replica?
    /// todo: we might only want to delete after rebuild completes only..
    delete: bool,
}
impl MoveReplicaRequest {
    /// Get a reference to the replica.
    pub(crate) fn replica(&self) -> &ReplicaId {
        &self.replica
    }
    /// Builder-like specification of delete behaviour.
    pub(crate) fn with_delete(mut self, delete: bool) -> Self {
        self.delete = delete;
        self
    }
}
impl From<&ENoSpcReplica> for MoveReplicaRequest {
    fn from(value: &ENoSpcReplica) -> Self {
        Self {
            replica: value.replica().uuid.clone(),
            delete: false,
        }
    }
}

#[async_trait::async_trait]
impl ResourceReplicas for OperationGuardArc<VolumeSpec> {
    type Request = SetVolumeReplica;
    type MoveRequest = MoveReplicaRequest;
    type MoveResp = Replica;

    async fn set_replica(
        &mut self,
        registry: &Registry,
        request: &Self::Request,
    ) -> Result<(), SvcError> {
        let state = registry.volume_state(&request.uuid).await?;

        let operation = VolumeOperation::SetReplica(request.replicas);
        let spec_clone = self.start_update(registry, &state, operation).await?;

        assert_ne!(request.replicas, spec_clone.num_replicas);
        if request.replicas > spec_clone.num_replicas {
            self.increase_volume_replica(registry, state, spec_clone)
                .await?;
        } else {
            self.decrease_volume_replica(registry, state, spec_clone)
                .await?;
        }
        Ok(())
    }

    async fn move_replica(
        &mut self,
        registry: &Registry,
        request: &Self::MoveRequest,
    ) -> Result<Self::MoveResp, SvcError> {
        let candidates =
            volume_move_replica_candidates(registry, self.as_ref(), request.replica()).await?;

        let new_replica = self
            .create_volume_replica_with(registry, candidates)
            .await?;

        if let Some(nexus_spec) = &self
            .as_ref()
            .target()
            .and_then(|t| registry.specs().nexus_rsc(t.nexus()))
        {
            let mut guard = nexus_spec.operation_guard()?;
            guard
                .attach_replica(registry, &new_replica, self.has_snapshots())
                .await?;

            if request.delete {
                self.remove_child_replica(request.replica(), &mut guard, registry)
                    .await?;
            }
        } else if request.delete {
            // todo: if there's no nexus, should we delete it?
            // For now let the reconciler delete it?
        }

        Ok(new_replica)
    }
}

#[async_trait::async_trait]
impl ResourceProperty for OperationGuardArc<VolumeSpec> {
    type Request = SetVolumeProperty;

    async fn set_property(
        &mut self,
        registry: &Registry,
        request: &Self::Request,
    ) -> Result<(), SvcError> {
        let state = registry.volume_state(&request.uuid).await?;
        let operation = VolumeOperation::SetVolumeProperty(request.property.clone());
        let spec_clone = self.start_update(registry, &state, operation).await?;

        self.complete_update(registry, Ok(()), spec_clone).await?;
        Ok(())
    }
}
#[async_trait::async_trait]
impl ResourceShutdownOperations for OperationGuardArc<VolumeSpec> {
    type RemoveShutdownTargets = DestroyShutdownTargets;
    type Shutdown = ();

    async fn shutdown(
        &mut self,
        _registry: &Registry,
        _request: &Self::Shutdown,
    ) -> Result<(), SvcError> {
        // not applicable for volume
        unimplemented!()
    }

    async fn remove_shutdown_targets(
        &mut self,
        registry: &Registry,
        request: &Self::RemoveShutdownTargets,
    ) -> Result<(), SvcError> {
        let shutdown_nexuses = registry
            .specs()
            .volume_shutdown_nexuses(request.uuid())
            .await;
        let mut result = Ok(());
        for nexus_res in shutdown_nexuses {
            match nexus_res.operation_guard_wait().await {
                Ok(mut guard) => {
                    if let Ok(nexus) = registry.nexus(nexus_res.uuid()).await {
                        if Self::target_registered(request.registered_targets(), nexus)? {
                            continue;
                        }
                    }
                    let nexus_spec = guard.as_ref().clone();
                    let destroy_req = DestroyNexus::from(nexus_spec)
                        .with_disown(request.uuid())
                        .with_lazy(true);
                    match guard.destroy(registry, &destroy_req).await {
                        Ok(_) => {
                            if self.as_ref().health_info_id() != Some(guard.uuid()) {
                                ResourceSpecsLocked::delete_nexus_info(
                                    &NexusInfoKey::new(&Some(request.uuid().clone()), guard.uuid()),
                                    registry,
                                )
                                .await;
                            }
                        }
                        Err(error) => match error {
                            // If the store is not available, no point in trying the others.
                            SvcError::Store { .. } => return Err(error),
                            _ => {
                                tracing::debug!(
                                    %error,
                                    nexus.uuid = %destroy_req.uuid,
                                    "Encountered error while destroying shutdown nexus"
                                );
                                // if we're not at least marked for deletion then we'll have to
                                // get the cluster agent to retry..
                                if !guard.lock().status().deleting_or_deleted() {
                                    result = Err(error);
                                }
                            }
                        },
                    }
                }
                Err(error) => {
                    result = Err(error);
                }
            }
        }
        result
    }
}

#[async_trait::async_trait]
impl ResourceLifecycleExt<CreateVolume> for OperationGuardArc<VolumeSpec> {
    type CreateOutput = Self;

    async fn create_ext(
        registry: &Registry,
        request: &CreateVolume,
    ) -> Result<Self::CreateOutput, SvcError> {
        let specs = registry.specs();
        let mut volume = specs
            .get_or_create_volume(&CreateVolumeSource::None(request))?
            .operation_guard_wait()
            .await?;
        let volume_clone = volume.start_create(registry, request).await?;

        // If the volume is a part of the ag, create or update accordingly.
        registry.specs().get_or_create_affinity_group(&volume_clone);

        // todo: pick nodes and pools using the Node&Pool Topology
        // todo: virtually increase the pool usage to avoid a race for space with concurrent calls
        let result = create_volume_replicas(registry, request, &volume_clone).await;
        let create_replica_candidate = volume
            .validate_create_step_ext(registry, result, OnCreateFail::Delete)
            .await?;

        let mut replicas = Vec::<Replica>::new();
        for replica in create_replica_candidate.candidates() {
            if replicas.len() >= request.replicas as usize {
                break;
            } else if replicas.iter().any(|r| r.node == replica.node) {
                // don't reuse the same node
                continue;
            }
            let replica = if replicas.is_empty() {
                let mut replica = replica.clone();
                // the local replica needs to be connected via "bdev:///"
                replica.share = Protocol::None;
                replica
            } else {
                replica.clone()
            };
            match OperationGuardArc::<ReplicaSpec>::create(registry, &replica).await {
                Ok(replica) => {
                    replicas.push(replica);
                }
                Err(error) => {
                    volume_clone.error(&format!(
                        "Failed to create replica {:?} for volume, error: {}",
                        replica,
                        error.full_string()
                    ));
                    // continue trying...
                }
            };
        }

        // we can't fulfil the required replication factor, so let the caller
        // decide what to do next
        let result = if replicas.len() < request.replicas as usize {
            for replica_state in replicas {
                let result = match specs.replica(&replica_state.uuid).await {
                    Ok(mut replica) => {
                        let request = DestroyReplica::from(replica_state.clone());
                        replica.destroy(registry, &request.with_disown_all()).await
                    }
                    Err(error) => Err(error),
                };
                if let Err(error) = result {
                    volume_clone.error(&format!(
                        "Failed to delete replica {:?} from volume, error: {}",
                        replica_state,
                        error.full_string()
                    ));
                }
            }
            Err(SvcError::ReplicaCreateNumber {
                id: request.uuid.to_string(),
            })
        } else {
            Ok(())
        };

        // we can destroy volume on error because there's no volume resource created on the nodes,
        // only sub-resources (such as nexuses/replicas which will be garbage-collected later).
        volume
            .complete_create(result, registry, OnCreateFail::Delete)
            .await?;
        Ok(volume)
    }
}

#[async_trait::async_trait]
impl ResourceLifecycleExt<CreateVolumeSource<'_>> for OperationGuardArc<VolumeSpec> {
    type CreateOutput = Self;

    async fn create_ext(
        registry: &Registry,
        request_src: &CreateVolumeSource,
    ) -> Result<Self::CreateOutput, SvcError> {
        request_src.pre_flight_check()?;
        let specs = registry.specs();
        let mut volume = specs
            .get_or_create_volume(request_src)?
            .operation_guard_wait()
            .await?;
        let volume_clone = volume
            .start_create_update(registry, request_src.source())
            .await?;

        // If the volume is a part of the ag, create or update accordingly.
        registry.specs().get_or_create_affinity_group(&volume_clone);

        let context = Context {
            registry,
            volume: &mut volume,
        };
        let result = match request_src {
            CreateVolumeSource::None(params) => params.run(context).await,
            CreateVolumeSource::Snapshot(params) => params.run(context).await,
        };

        // we can destroy volume on error because there's no volume resource created on the nodes,
        // only sub-resources (such as nexuses/replicas which will be garbage-collected later).
        volume
            .complete_create(result, registry, OnCreateFail::Delete)
            .await?;
        Ok(volume)
    }
}

/// A volume can be created with different sources for its replicas.
pub(crate) enum CreateVolumeSource<'a> {
    /// Carve out new replicas from a pool matching the requested topology.
    None(&'a CreateVolume),
    /// Clone replica from an existing volume snapshot.
    Snapshot(SnapshotCloneOp<'a>),
}

impl CreateVolumeSource<'_> {
    /// Get the source create volume request.
    pub(crate) fn source(&self) -> &CreateVolume {
        match self {
            Self::None(param) => param,
            Self::Snapshot(param) => param.0.params(),
        }
    }
}

/// Context for SetupVolumeReplicas trait.
pub(super) struct Context<'a> {
    pub(super) registry: &'a Registry,
    pub(super) volume: &'a mut OperationGuardArc<VolumeSpec>,
}

/// Trait that abstracts away the pre-flight validation checks when creating a volume.
pub(super) trait CreateVolumeExeVal: Sync + Send {
    fn pre_flight_check(&self) -> Result<(), SvcError>;
}

/// Trait that abstracts away the process of creating volume replicas.
#[async_trait::async_trait]
pub(super) trait CreateVolumeExe: CreateVolumeExeVal {
    type Candidates: Send + Sync;

    async fn run<'a>(&'a self, mut context: Context<'a>) -> Result<Vec<Replica>, SvcError> {
        let result = self.setup(&mut context).await;
        let candidates = context
            .volume
            .validate_create_step_ext(context.registry, result, OnCreateFail::Delete)
            .await?;
        let replicas = self.create(&mut context, candidates).await;

        // we can't fulfil the required replication factor, so let the caller
        // decide what to do next
        if replicas.len() < context.volume.as_ref().num_replicas as usize {
            self.undo(&mut context, replicas).await;
            Err(SvcError::ReplicaCreateNumber {
                id: context.volume.uid_str(),
            })
        } else {
            Ok(replicas)
        }
    }
    async fn setup<'a>(&'a self, context: &mut Context<'a>) -> Result<Self::Candidates, SvcError>;
    async fn create<'a>(
        &'a self,
        context: &mut Context<'a>,
        candidates: Self::Candidates,
    ) -> Vec<Replica>;
    async fn undo<'a>(&'a self, context: &mut Context<'a>, replicas: Vec<Replica>);
}

impl CreateVolumeExeVal for CreateVolume {
    fn pre_flight_check(&self) -> Result<(), SvcError> {
        snafu::ensure!(
            self.allowed_nodes().is_empty() || self.allowed_nodes().len() >= self.replicas as usize,
            agents::errors::InvalidArguments {}
        );
        Ok(())
    }
}

#[async_trait::async_trait]
impl CreateVolumeExe for CreateVolume {
    type Candidates = CreateReplicaCandidate;

    async fn setup<'a>(
        &'a self,
        context: &mut Context<'a>,
    ) -> Result<CreateReplicaCandidate, SvcError> {
        // todo: pick nodes and pools using the Node&Pool Topology
        // todo: virtually increase the pool usage to avoid a race for space with concurrent calls
        create_volume_replicas(context.registry, self, context.volume.as_ref()).await
    }

    async fn create<'a>(
        &'a self,
        context: &mut Context<'a>,
        candidates: CreateReplicaCandidate,
    ) -> Vec<Replica> {
        let mut replicas = Vec::<Replica>::with_capacity(candidates.candidates().len());
        for replica in candidates.candidates() {
            if replicas.len() >= self.replicas as usize {
                break;
            } else if replicas.iter().any(|r| {
                r.node == replica.node
                    || spread_label_is_same(r, replica, context).unwrap_or_else(|error| {
                        context.volume.error(&format!(
                            "Failed to create replica {:?} for volume, error: {}",
                            replica,
                            error.full_string()
                        ));
                        false
                    })
            }) {
                // don't re-use the same node or same exclusion labels
                continue;
            }
            let replica = if replicas.is_empty() {
                let mut replica = replica.clone();
                // the local replica needs to be connected via "bdev:///"
                replica.share = Protocol::None;
                replica
            } else {
                replica.clone()
            };
            match OperationGuardArc::<ReplicaSpec>::create(context.registry, &replica).await {
                Ok(replica) => {
                    replicas.push(replica);
                }
                Err(error) => {
                    context.volume.error(&format!(
                        "Failed to create replica {:?} for volume, error: {}",
                        replica,
                        error.full_string()
                    ));
                    // continue trying...
                }
            };
        }
        replicas
    }

    async fn undo<'a>(&'a self, context: &mut Context<'a>, replicas: Vec<Replica>) {
        for replica_state in replicas {
            let result = match context.registry.specs().replica(&replica_state.uuid).await {
                Ok(mut replica) => {
                    let request = DestroyReplica::from(replica_state.clone());
                    replica
                        .destroy(context.registry, &request.with_disown_all())
                        .await
                }
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                context.volume.error(&format!(
                    "Failed to delete replica {:?} from volume, error: {}",
                    replica_state,
                    error.full_string()
                ));
            }
        }
    }
}

impl CreateVolumeExeVal for CreateVolumeSource<'_> {
    fn pre_flight_check(&self) -> Result<(), SvcError> {
        match self {
            CreateVolumeSource::None(params) => params.pre_flight_check(),
            CreateVolumeSource::Snapshot(params) => params.pre_flight_check(),
        }
    }
}

// spread_label_is_same checks if the node labels is same as that of all existing replicas.
fn spread_label_is_same(
    replica: &Replica,
    replica_candidate: &CreateReplica,
    context: &Context<'_>,
) -> Result<bool, SvcError> {
    let topology = context.volume.as_ref().topology.as_ref();
    let node_topology = topology.and_then(|topology| topology.node.as_ref());

    match node_topology {
        Some(NodeTopology::Explicit(_)) => Ok(false),
        Some(NodeTopology::Labelled(labelled_topology)) => {
            if !labelled_topology.exclusion.is_empty() {
                let candidate_node = context.registry.specs().node(&replica_candidate.node)?;
                let replica_node = context.registry.specs().node(&replica.node)?;

                for key in labelled_topology.exclusion.keys() {
                    if let (Some(candidate_value), Some(replica_value)) = (
                        candidate_node.labels().get(key),
                        replica_node.labels().get(key),
                    ) {
                        if candidate_value == replica_value {
                            return Ok(true);
                        }
                    }
                }
            }
            Ok(false)
        }
        _ => Ok(false),
    }
}
