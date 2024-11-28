use crate::{
    controller::{
        registry::Registry,
        resources::{
            operations::{ResourceLifecycle, ResourceOwnerUpdate, ResourceResize},
            operations_helper::{
                GuardedOperationsHelper, OperationSequenceGuard, ResourceSpecsLocked,
            },
            OperationGuardArc, ResourceUid, TraceSpan, TraceStrLog,
        },
        scheduling::resources::{HealthyChildItems, ReplicaItem},
    },
    volume::{
        scheduling::target_node_candidate,
        specs::{
            healthy_volume_replicas, nexus_attach_candidates, nexus_child_remove_candidates,
            volume_replica_candidates, volume_replica_remove_candidate,
            volume_unused_replica_remove_candidates, NexusNodeCandidate,
        },
    },
};
use agents::errors::{NotEnough, SvcError, SvcError::ReplicaRemovalNoCandidates};

use grpc::operations::volume::traits::PublishVolumeInfo;
use stor_port::{
    transport_api::ErrorChain,
    types::v0::{
        store::{
            nexus::{NexusSpec, ReplicaUri},
            nexus_child::NexusChild,
            replica::ReplicaSpec,
            volume::{FrontendConfig, TargetConfig, VolumeOperation, VolumeSpec, VolumeTarget},
        },
        transport::{
            CreateNexus, CreateReplica, Nexus, NexusId, NexusNvmePreemption, NexusNvmfConfig,
            NodeBugFix, NodeId, NvmeReservation, NvmfControllerIdRange, Protocol, Replica,
            ReplicaId, ReplicaOwners, ResizeNexus, ResizeReplica, Volume, VolumeShareProtocol,
            VolumeState,
        },
    },
    HostAccessControl,
};

use http::Uri;

impl OperationGuardArc<VolumeSpec> {
    /// Check if the volume is published.
    pub(super) fn published(&self) -> bool {
        self.as_ref().target().is_some()
    }
    /// Make the next target config.
    /// This essentially bumps up the controller id by 1 as otherwise the initiator cannot tell
    /// this target apart from others.
    /// Also sets the reservation key based off the nexus uuid.
    pub(super) async fn next_target_config(
        &self,
        registry: &Registry,
        node: &NodeId,
        share: &Option<VolumeShareProtocol>,
        frontend_nodes: &[String],
    ) -> TargetConfig {
        let nexus = NexusId::new();
        let resv_key = nexus.as_u128() as u64;
        let range = match self.as_ref().config() {
            None => NvmfControllerIdRange::new_min(2),
            Some(cfg) => {
                // todo: should the cluster agent tell us which controller Id to use?
                #[allow(clippy::if_same_then_else)]
                if self.published() {
                    cfg.config().controller_id_range().next(2)
                } else {
                    // if we're not published should we start over?
                    // for now let's carry on to next as there might some cases where we unpublish
                    // but the initiator doesn't disconnect properly.
                    cfg.config().controller_id_range().next(2)
                }
            }
        };
        let disable_acc = registry.target_acc_disabled();
        let host_acl = registry.host_acl_nodename(HostAccessControl::Nexuses, frontend_nodes);
        let frontend = FrontendConfig::from_acls(host_acl);
        let mut config = NexusNvmfConfig::new(
            range,
            resv_key,
            NvmeReservation::ExclusiveAccess,
            NexusNvmePreemption::Holder,
        );
        if disable_acc {
            config = config.with_no_resv();
        }

        TargetConfig::new(
            VolumeTarget::new(node.clone(), nexus, *share),
            config,
            frontend,
        )
    }

    /// Remove the given NexusChild Replica but make sure we're not removing the last healthy
    /// replica of the volume which would cause data loss!
    /// TODO: Should there be a minimum remaining number of healthy replicas left?
    #[tracing::instrument(level = "info", skip(self, nexus, registry), fields(volume.uuid = %self.uuid()))]
    pub(super) async fn remove_child_replica(
        &mut self,
        replica_id: &ReplicaId,
        nexus: &mut OperationGuardArc<NexusSpec>,
        registry: &Registry,
    ) -> Result<(), SvcError> {
        let health_info = self.lock().health_info_id().cloned();
        let nexus_info = registry
            .nexus_info(Some(self.uuid()), health_info.as_ref(), true)
            .await?
            .ok_or(SvcError::Internal {
                details: "No NexusInfo for a volume with allocated storage?".into(),
            })?;

        let nexus_state = registry.nexus(nexus.uuid()).await?;

        let replica_uri = match nexus.as_ref().replica_uuid_uri(replica_id) {
            Some(replica_uri) => Ok(replica_uri.clone()),
            // If the child is no longer present, not much point carrying on..
            None => Err(SvcError::ChildNotFound {
                nexus: nexus.uuid().to_string(),
                child: replica_id.to_string(),
            }),
        }?;
        let spec_children = &nexus.as_ref().children;
        let healthy_children_len = nexus_state
            .children
            .into_iter()
            .filter(|c| c.state.online())
            .filter(|c| {
                match spec_children
                    .iter()
                    .find(|sc| sc.uri() == c.uri)
                    .and_then(|sc| sc.as_replica())
                {
                    Some(sc) => nexus_info.is_replica_healthy(sc.uuid()),
                    None => false,
                }
            })
            .filter(|c| &c.uri != replica_uri.uri())
            .count();

        // todo: what should the minimum number of healthy children be?
        if healthy_children_len == 0 {
            self.warn_span(|| {
                tracing::error!(
                    replica.uuid = replica_id.as_str(),
                    "Cannot remove replica as volume has no other healthy children remaining"
                )
            });
            return Err(SvcError::NoHealthyReplicas {
                id: self.uuid().to_string(),
            });
        }

        let mut replica = registry.specs().replica(replica_id).await?;
        replica.fault(nexus, registry).await?;
        self.warn_span(|| {
            tracing::warn!(
                child.uri = replica_uri.uri().as_str(),
                child.uuid = replica_uri.uuid().as_str(),
                replica.pool = replica.as_ref().pool.pool_name().as_str(),
                "Successfully faulted child"
            )
        });

        Ok(())
    }

    /// Increase the replica count of the given volume by 1
    /// Creates a new data replica from a list of candidates
    /// Adds the replica to the volume nexuses (if any)
    pub(super) async fn increase_volume_replica(
        &mut self,
        registry: &Registry,
        state: VolumeState,
        spec_clone: VolumeSpec,
    ) -> Result<Volume, SvcError> {
        // Create an ag guard to prevent candidate collision.
        let _ag_guard = match registry.specs().get_or_create_affinity_group(&spec_clone) {
            Some(ag) => Some(ag.operation_guard_wait().await?),
            _ => None,
        };

        // Prepare a list of candidates (based on some criteria)
        let result = volume_replica_candidates(registry, &spec_clone).await;
        let candidates = self
            .validate_update_step(registry, result, &spec_clone)
            .await?;

        // Create the data replica from the pool candidates
        let result = self.create_volume_replica_with(registry, candidates).await;
        let replica = self
            .validate_update_step(registry, result, &spec_clone)
            .await?;

        // Add the newly created replica to the nexus, if it's up
        let result = self.attach_to_target(registry, replica).await;
        self.complete_update(registry, result, spec_clone).await?;

        registry.volume(&state.uuid).await
    }

    /// Decrement the replica count of the given volume by 1
    /// Removes the replica from all volume nexuses
    pub(super) async fn decrease_volume_replica(
        &mut self,
        registry: &Registry,
        state: VolumeState,
        spec_clone: VolumeSpec,
    ) -> Result<Volume, SvcError> {
        // Create a ag guard to prevent candidate collision.
        let _ag_guard = match registry.specs().get_or_create_affinity_group(&spec_clone) {
            Some(ag) => Some(ag.operation_guard_wait().await?),
            _ => None,
        };

        // Determine which replica is most suitable to be removed
        let result = volume_replica_remove_candidate(&spec_clone, &state, registry).await;

        if let Err(ReplicaRemovalNoCandidates { .. }) = result {
            // The desired number of replicas is already met. This can occur if a replica has been
            // removed from the volume due to an error.
            self.complete_update(registry, Ok(()), spec_clone).await?;
        } else {
            // Can fail if meanwhile the state of a replica/nexus/child changes, so fail gracefully
            let remove = self
                .validate_update_step(registry, result, &spec_clone)
                .await?;

            // Remove the replica from its nexus (where it exists as a child)
            let result = self.remove_volume_child_candidate(registry, &remove).await;
            self.validate_update_step(registry, result, &spec_clone)
                .await?;

            // todo: we could ignore it here, since we've already removed it from the nexus
            //  now remove the replica from the pool
            let result = self.destroy_replica(registry, remove.spec()).await;

            self.complete_update(registry, result, spec_clone).await?;
        }

        registry.volume(&state.uuid).await
    }

    /// Remove unused replicas from the volume
    /// (that is, replicas which are not used by a nexus and are in excess to the
    /// volume's replica count).
    pub(crate) async fn remove_unused_volume_replicas(
        &mut self,
        registry: &Registry,
        mut count: usize,
    ) -> Result<(), SvcError> {
        let state = registry.volume_state(self.uuid()).await?;

        let mut candidates =
            volume_unused_replica_remove_candidates(self.as_ref(), &state, registry).await?;

        let mut result = Ok(());
        while let Some(replica) = candidates.next() {
            if count == 0 {
                break;
            }
            match self
                .remove_unused_volume_replica(registry, &replica.spec().uuid)
                .await
            {
                Ok(_) => {
                    self.info(&format!(
                        "Successfully removed unused replica '{}'",
                        replica.spec().uuid
                    ));
                    count -= 1;
                }
                Err(error) => {
                    self.warn_span(|| {
                        tracing::warn!(
                            "Failed to remove unused replicas, error: {}",
                            error.full_string()
                        )
                    });
                    result = Err(error);
                }
            }
        }
        result
    }

    /// Remove unused replica from the volume
    /// (that is, replicas which are not used by a nexus).
    /// It must not be the last replica of the volume
    /// (an error will be returned in such case).
    pub(crate) async fn remove_unused_volume_replica(
        &mut self,
        registry: &Registry,
        replica_id: &ReplicaId,
    ) -> Result<(), SvcError> {
        let state = registry.volume_state(self.uuid()).await?;
        let spec_clone = self
            .start_update(
                registry,
                &state,
                VolumeOperation::RemoveUnusedReplica(replica_id.clone()),
            )
            .await?;

        // The replica is unused, so we can disown it...
        let replica = registry.specs().replica(replica_id).await;
        let mut replica = self
            .validate_update_step(registry, replica, &spec_clone)
            .await?;

        // disown it from the volume first, so at the very least it can be garbage collected
        // at a later point if the node is not accessible
        let disowner = ReplicaOwners::from_volume(self.uuid());
        let result = replica.remove_owners(registry, &disowner, true).await;
        self.validate_update_step(registry, result, &spec_clone)
            .await?;

        // the garbage collector will destroy it at a later time
        if let Err(error) = replica.destroy_volume_replica(registry, None).await {
            spec_clone.error_span(|| {
                tracing::error!(
                    "Failed to destroy replica '{}'. Error: '{}'. It will be garbage collected.",
                    replica_id,
                    error.full_string()
                )
            });
        }
        self.complete_update(registry, Ok(()), spec_clone).await?;
        Ok(())
    }

    /// Attach existing replicas to the given volume nexus until it reaches the required number of
    /// data replicas.
    /// Returns the first encountered error, but tries to add as many as it can until it does so.
    pub(crate) async fn attach_replicas_to_nexus(
        &mut self,
        registry: &Registry,
        nexus: &mut OperationGuardArc<NexusSpec>,
    ) -> Result<(), SvcError> {
        let (replicas, nodes) = {
            let specs = registry.specs().read();
            let replicas = specs.volume_replicas_it(self.uuid());
            let nodes = specs.replica_nodes(replicas.clone());
            (replicas.count(), nodes)
        };
        let volume_children = replicas.min(self.as_ref().num_replicas as usize);
        let mut nexus_children = nexus.as_ref().children.len();

        if self.has_snapshots() {
            registry.verify_nodes_fix(nodes.iter(), &NodeBugFix::NexusRebuildReplicaAncestry)?;
        }
        let replicas = nexus_attach_candidates(self.as_ref(), nexus.as_ref(), registry).await?;

        let mut result = Ok(());
        for replica in replicas {
            if nexus_children >= volume_children {
                break;
            }

            if replica.rebuildable() == &Some(false) {
                if self
                    .remove_unused_volume_replica(registry, &replica.state().uuid)
                    .await
                    .is_ok()
                {
                    nexus.info_span(|| {
                        let state = replica.state();
                        tracing::warn!(
                            replica.uuid = %state.uuid,
                            replica.pool = %state.pool_id,
                            replica.node = %state.node,
                            "Removed unrebuildable replica from the volume",
                        )
                    });
                }
                continue;
            }

            match nexus
                .attach_replica(registry, replica.state(), self.has_snapshots())
                .await
            {
                Ok(_) => {
                    nexus.info_span(|| {
                        let state = replica.state();
                        tracing::info!(
                            replica.uuid = %state.uuid,
                            replica.pool = %state.pool_id,
                            replica.node = %state.node,
                            "Successfully attached replica to nexus",
                        )
                    });
                    nexus_children += 1;
                }
                Err(error) => {
                    nexus.warn_span(|| {
                        let state = replica.state();
                        tracing::error!(
                            replica.uuid = %state.uuid,
                            replica.pool = %state.pool_id,
                            replica.node = %state.node,
                            error = error.full_string(),
                            "Failed to attach replica to nexus",
                        )
                    });
                    result = Err(error);
                }
            };
        }
        result
    }

    /// Remove excessive replicas from the given volume nexus.
    /// It should not have more replicas then the volume's required replica count.
    /// Returns the first encountered error, but tries to remove as many as it can until it does so.
    pub(crate) async fn remove_excess_replicas_from_nexus(
        &mut self,
        registry: &Registry,
        nexus: &mut OperationGuardArc<NexusSpec>,
        nexus_state: &Nexus,
    ) -> Result<(), SvcError> {
        let vol_spec_clone = self.lock().clone();
        let nexus_spec_clone = nexus.lock().clone();
        let volume_children = vol_spec_clone.num_replicas as usize;
        let mut nexus_replica_children =
            nexus_spec_clone
                .children
                .iter()
                .fold(0usize, |mut c, child| {
                    if let Some(replica) = child.as_replica() {
                        if registry.specs().replica_rsc(replica.uuid()).is_some() {
                            c += 1;
                        }
                    }
                    c
                });

        let mut candidates =
            nexus_child_remove_candidates(&vol_spec_clone, &nexus_spec_clone, registry).await?;

        while let Some(candidate) = candidates.next() {
            if nexus_replica_children <= volume_children {
                break;
            }
            let child_uri = match candidate.child_state() {
                None => break,
                Some(child) => child.uri.clone(),
            };
            match nexus
                .remove_child_by_uri(registry, nexus_state, &child_uri)
                .await
            {
                Ok(_) => {
                    nexus_spec_clone.info(&format!(
                        "Successfully removed child '{child_uri}' from nexus",
                    ));
                    nexus_replica_children -= 1;
                }
                Err(error) => {
                    nexus_spec_clone.error(&format!(
                        "Failed to remove child '{}' from nexus, error: '{}'",
                        child_uri,
                        error.full_string()
                    ));
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    /// Create a replica for the given volume using the provided list of candidates in order.
    pub(super) async fn create_volume_replica_with(
        &self,
        registry: &Registry,
        candidates: Vec<CreateReplica>,
    ) -> Result<Replica, SvcError> {
        let mut result = Err(SvcError::NotEnoughResources {
            source: NotEnough::OfReplicas { have: 0, need: 1 },
        });
        for mut attempt in candidates {
            if let Some(target) = &self.as_ref().target() {
                if target.node() == &attempt.node {
                    attempt.share = Protocol::None;
                }
            }

            result = OperationGuardArc::<ReplicaSpec>::create(registry, &attempt).await;
            if result.is_ok() {
                break;
            }
        }
        result
    }

    /// Create `count` replicas for the given volume.
    pub(crate) async fn create_volume_replicas(
        &self,
        registry: &Registry,
        count: usize,
    ) -> Result<Vec<ReplicaId>, SvcError> {
        let mut created_replicas = Vec::with_capacity(count);
        let mut candidate_error = None;

        for iter in 0..count {
            let candidates = match volume_replica_candidates(registry, self.as_ref()).await {
                Ok(candidates) => candidates,
                Err(error) => {
                    candidate_error = Some(error);
                    break;
                }
            };

            match self.create_volume_replica_with(registry, candidates).await {
                Ok(replica) => {
                    self.debug(&format!("Successfully created replica '{}'", replica.uuid));
                    created_replicas.push(replica.uuid);
                    break;
                }
                Err(error) => {
                    self.error(&format!(
                        "Failed to create replica, error: '{}'",
                        error.full_string(),
                    ));
                }
            }

            if created_replicas.len() <= iter {
                break;
            }
        }

        if created_replicas.is_empty() {
            if let Some(error) = candidate_error {
                return Err(error);
            }
        }
        Ok(created_replicas)
    }

    /// Resize the replicas of the volume.
    pub(super) async fn resize_volume_replicas(
        &self,
        registry: &Registry,
        replicas: &Vec<Replica>,
        requested_size: u64,
    ) -> Result<(), SvcError> {
        for replica in replicas {
            let mut replica_grd = registry.specs().replica(&replica.uuid).await?;

            replica_grd
                .resize(
                    registry,
                    &ResizeReplica::new(
                        &replica.node,
                        replica_grd.as_ref().pool_name(),
                        None,
                        replica_grd.uuid(),
                        requested_size,
                    ),
                )
                .await?;
        }

        Ok(())
    }

    /// Resize the volume target(nexus).
    pub(super) async fn resize_target(
        &self,
        registry: &Registry,
        nexus_grd: &mut OperationGuardArc<NexusSpec>,
        requested_size: u64,
    ) -> Result<(), SvcError> {
        nexus_grd
            .resize(
                registry,
                &ResizeNexus::new(
                    &nexus_grd.as_ref().node.clone(),
                    &nexus_grd.uuid().clone(),
                    requested_size,
                ),
            )
            .await?;
        Ok(())
    }

    /// Add the given replica to the target nexus of the volume.
    async fn attach_to_target(
        &self,
        registry: &Registry,
        replica: Replica,
    ) -> Result<(), SvcError> {
        if let Some(target) = &self.as_ref().target() {
            let mut nexus_guard = registry.specs().nexus(target.nexus()).await?;
            nexus_guard
                .attach_replica(registry, &replica, self.has_snapshots())
                .await
        } else {
            Ok(())
        }
    }

    /// Remove a replica from all nexuses for the given volume.
    /// Only volumes with 1 nexus are currently supported.
    pub(super) async fn remove_volume_child_candidate(
        &self,
        registry: &Registry,
        remove: &ReplicaItem,
    ) -> Result<(), SvcError> {
        // if the nexus is up, first remove the child from the nexus before deleting the replica
        let mut nexus = registry.specs().volume_target_nexus(self.as_ref()).await?;
        let nexus = nexus.iter_mut().find_map(|n| {
            let found = n
                .lock()
                .children
                .iter()
                .flat_map(|c| c.as_replica())
                .find(|r| r.uuid() == &remove.spec().uuid);
            found.map(|r| (n, r))
        });
        match nexus {
            None => Ok(()),
            Some((nexus, replica)) => nexus.remove_replica(registry, &replica).await,
        }
    }

    /// Create a nexus for the given volume on the specified target_node.
    /// Existing replicas may be shared/unshared so we can connect to them.
    pub(super) async fn create_nexus(
        &self,
        registry: &Registry,
        target_config: &TargetConfig,
    ) -> Result<(OperationGuardArc<NexusSpec>, Nexus), SvcError> {
        let vol_spec = self.as_ref();
        let target_node = target_config.target().node();
        let nexus_id = target_config.target().nexus();
        let children = healthy_volume_replicas(vol_spec, target_node, registry).await?;
        let (count, items) = match children {
            HealthyChildItems::One(_, candidates) => (1, candidates),
            HealthyChildItems::All(_, candidates) => (candidates.len(), candidates),
        };

        let mut nexus_replicas = vec![];
        let mut nodes = vec![];
        for item in items {
            if nexus_replicas.len() >= count {
                break;
            } else if nodes.contains(&item.state().node) {
                // spread the children across the nodes
                continue;
            }

            if let Ok(uri) = OperationGuardArc::<NexusSpec>::make_replica_accessible(
                registry,
                item.state(),
                target_node,
            )
            .await
            {
                nexus_replicas.push(NexusChild::Replica(ReplicaUri::new(
                    &item.spec().uuid,
                    &uri,
                )));
                nodes.push(item.state().node.clone());
            }
        }
        nexus_replicas.truncate(vol_spec.num_replicas as usize);

        if nexus_replicas.is_empty() {
            return Err(SvcError::NoOnlineReplicas {
                id: vol_spec.uid_str(),
            });
        }

        // Create the nexus on the requested node
        let (guard, nexus) = OperationGuardArc::<NexusSpec>::create(
            registry,
            &CreateNexus::new(
                target_node,
                nexus_id,
                vol_spec.size,
                &nexus_replicas,
                true,
                Some(&vol_spec.uuid),
                Some(target_config.config().clone()),
            ),
        )
        .await?;

        if nexus.children.len() < vol_spec.num_replicas as usize {
            vol_spec.warn_span(|| {
                tracing::warn!(
                    "Recreated volume target with only {} out of {} desired replicas",
                    nexus.children.len(),
                    vol_spec.num_replicas
                )
            });
        }

        Ok((guard, nexus))
    }

    /// Get the volume's next target node.
    pub(super) async fn next_target_node(
        &self,
        registry: &Registry,
        request: &impl PublishVolumeInfo,
        state: &VolumeState,
        republish: bool,
    ) -> Result<NexusNodeCandidate, SvcError> {
        // Create a ag guard to prevent candidate collision.
        let affinity_group = self.as_ref().affinity_group.as_ref();
        let ag_guard = match affinity_group {
            None => None,
            Some(ag) => {
                let ag_spec = registry.specs().get_affinity_group(ag.id()).ok_or(
                    SvcError::AffinityGroupNotFound {
                        vol_grp_id: ag.id().to_string(),
                    },
                )?;
                Some(ag_spec.operation_guard_wait().await?)
            }
        };

        if !republish {
            // We can't configure a new target_node if the volume is currently published
            if let Some(target) = self.as_ref().target() {
                return Err(SvcError::VolumeAlreadyPublished {
                    vol_id: self.uuid().to_string(),
                    node: target.node().to_string(),
                    protocol: format!("{:?}", target.protocol()),
                });
            }
        }

        match request
            .target_node()
            .as_ref()
            .map(|n| (n, affinity_group.is_some()))
        {
            None | Some((_, true)) => {
                // In case there is no target node specified, let the control-plane scheduling logic
                // determine a suitable node for the same and in case of affinity group, let
                // control-plane decide with specified node as the preferred node.
                let candidate =
                    target_node_candidate(self.as_ref(), registry, state, &request.target_node())
                        .await?;
                tracing::debug!(node.id=%candidate.id(), "Node selected for volume publish by the core-agent");
                Ok(NexusNodeCandidate::new(candidate.id().clone(), ag_guard))
            }
            Some((node, false)) => {
                // Make sure the requested node is available.
                // todo: check the max number of nexuses per node is respected
                let node = registry.node_wrapper(node).await?;
                let node = node.read().await;
                if node.is_online() {
                    Ok(NexusNodeCandidate::new(node.id().clone(), ag_guard))
                } else {
                    Err(SvcError::NodeNotOnline {
                        node: node.id().clone(),
                    })
                }
            }
        }
    }

    /// Disown and if possible destroy the given volume replica.
    pub(super) async fn destroy_replica(
        &self,
        registry: &Registry,
        replica_spec: &ReplicaSpec,
    ) -> Result<(), SvcError> {
        match ResourceSpecsLocked::replica_node(registry, replica_spec).await {
            // Should never happen, but just in case...
            None => Err(SvcError::Internal {
                details: "Failed to find the node where a replica lives".to_string(),
            }),
            Some(node) => {
                let destroy_by = ReplicaOwners::from_volume(self.uuid());
                let mut replica = registry.specs().replica(&replica_spec.uuid).await?;
                replica
                    .destroy(registry, &replica.destroy_request(destroy_by, &node))
                    .await
            }
        }
    }

    /// Checks if Nexus is present in registered target list. Returns true if yes.
    pub(super) fn target_registered(
        registered_target: &Option<Vec<String>>,
        nexus: Nexus,
    ) -> Result<bool, SvcError> {
        if let Some(targets) = registered_target {
            let parsed_uri = nexus
                .device_uri
                .parse::<Uri>()
                .map_err(|_| SvcError::InvalidArguments {})?;
            let host = parsed_uri
                .host()
                .ok_or(SvcError::InvalidArguments {})?
                .to_string();
            Ok(targets.contains(&host))
        } else {
            Ok(false)
        }
    }

    pub fn has_snapshots(&self) -> bool {
        self.as_ref().metadata.has_snapshots()
    }
}
