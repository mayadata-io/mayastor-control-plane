use crate::controller::{
    registry::Registry,
    resources::{
        operations_helper::{
            GuardedOperationsHelper, OperationSequenceGuard, ResourceSpecs, ResourceSpecsLocked,
            SpecOperationsHelper,
        },
        OperationGuardArc, ResourceMutex,
    },
};
use agents::errors::{SvcError, SvcError::PoolNotFound};
use stor_port::{
    transport_api::ResourceKind,
    types::v0::{
        store::{
            pool::{PoolLabelOp, PoolOperation, PoolSpec, PoolUnLabelOp},
            replica::{ReplicaOperation, ReplicaSpec},
            SpecStatus, SpecTransaction,
        },
        transport::{
            CreatePool, CreateReplica, NodeId, PoolId, PoolStatus, Replica, ReplicaId,
            ReplicaOwners, ReplicaStatus,
        },
    },
};

#[async_trait::async_trait]
impl GuardedOperationsHelper for OperationGuardArc<PoolSpec> {
    type Create = CreatePool;
    type Owners = ();
    type Status = PoolStatus;
    type State = PoolSpec;
    type UpdateOp = PoolOperation;
    type Inner = PoolSpec;

    fn validate_destroy(&self, registry: &Registry) -> Result<(), SvcError> {
        let id = self.lock().id.clone();
        let pool_in_use =
            registry.specs().pool_has_replicas(&id) || registry.specs().pool_has_snapshots(&id);

        if pool_in_use {
            Err(SvcError::InUse {
                kind: ResourceKind::Pool,
                id: id.to_string(),
            })
        } else {
            Ok(())
        }
    }

    fn remove_spec(&self, registry: &Registry) {
        let id = self.lock().id.clone();
        registry.specs().remove_pool(&id);
    }
}

#[async_trait::async_trait]
impl SpecOperationsHelper for PoolSpec {
    type Create = CreatePool;
    type Owners = ();
    type Status = PoolStatus;
    type State = PoolSpec;
    type UpdateOp = PoolOperation;

    fn start_create_op(&mut self, _request: &Self::Create) {
        self.start_op(PoolOperation::Create);
    }
    fn start_destroy_op(&mut self) {
        self.start_op(PoolOperation::Destroy);
    }
    async fn start_update_op(
        &mut self,
        _: &Registry,
        _state: &Self::State,
        op: Self::UpdateOp,
    ) -> Result<(), SvcError> {
        match &op {
            PoolOperation::Label(PoolLabelOp { labels, overwrite }) => {
                let (existing, conflict) = self.label_collisions(labels);
                if !*overwrite && !existing.is_empty() {
                    Err(SvcError::LabelsExists {
                        resource: ResourceKind::Pool,
                        id: self.id().to_string(),
                        labels: format!("{existing:?}"),
                        conflict,
                    })
                } else {
                    self.start_op(op);
                    Ok(())
                }
            }
            PoolOperation::Unlabel(PoolUnLabelOp { label_key }) => {
                // Check that the label is present.
                if !self.has_labels_key(label_key) {
                    Err(SvcError::LabelNotFound {
                        resource: ResourceKind::Pool,
                        id: self.id().to_string(),
                        label_key: label_key.to_string(),
                    })
                } else {
                    self.start_op(op);
                    Ok(())
                }
            }
            _ => {
                self.start_op(op);
                Ok(())
            }
        }
    }

    fn dirty(&self) -> bool {
        // The pool spec can be dirty if a pool create operation fails to complete because it cannot
        // write to etcd.
        self.has_pending_op()
    }
    fn kind(&self) -> ResourceKind {
        ResourceKind::Pool
    }
    fn uuid_str(&self) -> String {
        self.id.to_string()
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

#[async_trait::async_trait]
impl GuardedOperationsHelper for OperationGuardArc<ReplicaSpec> {
    type Create = CreateReplica;
    type Owners = ReplicaOwners;
    type Status = ReplicaStatus;
    type State = Replica;
    type UpdateOp = ReplicaOperation;
    type Inner = ReplicaSpec;

    fn remove_spec(&self, registry: &Registry) {
        let uuid = self.lock().uuid.clone();
        registry.specs().remove_replica(&uuid);
    }
}

#[async_trait::async_trait]
impl SpecOperationsHelper for ReplicaSpec {
    type Create = CreateReplica;
    type Owners = ReplicaOwners;
    type Status = ReplicaStatus;
    type State = Replica;
    type UpdateOp = ReplicaOperation;

    async fn start_update_op(
        &mut self,
        _: &Registry,
        state: &Self::State,
        op: Self::UpdateOp,
    ) -> Result<(), SvcError> {
        match op {
            ReplicaOperation::Share(proto, _) if proto != self.share && self.share.shared() => {
                Err(SvcError::AlreadyShared {
                    kind: self.kind(),
                    id: self.uuid_str(),
                    share: state.share.to_string(),
                })
            }
            ReplicaOperation::Share(_, _) if !self.share.shared() => Ok(()),
            ReplicaOperation::Share(_, ref allowed_hosts) => {
                if allowed_hosts == &self.allowed_hosts && self.allowed_hosts == state.allowed_hosts
                {
                    Err(SvcError::AlreadyShared {
                        kind: self.kind(),
                        id: self.uuid_str(),
                        share: state.share.to_string(),
                    })
                } else {
                    Ok(())
                }
            }
            ReplicaOperation::Unshare if !self.share.shared() && !state.share.shared() => {
                Err(SvcError::NotShared {
                    kind: self.kind(),
                    id: self.uuid_str(),
                })
            }
            ReplicaOperation::Unshare => Ok(()),
            ReplicaOperation::OwnerUpdate(_) => Ok(()),
            // TODO: Shall we do something here with input size?
            ReplicaOperation::Resize(_) => Ok(()),
            _ => unreachable!(),
        }?;
        self.start_op(op);
        Ok(())
    }
    fn start_create_op(&mut self, _request: &Self::Create) {
        self.start_op(ReplicaOperation::Create);
    }
    fn start_destroy_op(&mut self) {
        self.start_op(ReplicaOperation::Destroy);
    }
    fn dirty(&self) -> bool {
        self.has_pending_op()
    }
    fn kind(&self) -> ResourceKind {
        ResourceKind::Replica
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
    fn owned(&self) -> bool {
        self.owners.is_owned()
    }
    fn owners(&self) -> Option<String> {
        Some(format!("{:?}", self.owners))
    }
    fn disown(&mut self, owner: &Self::Owners) {
        self.owners.disown(owner)
    }
    fn disown_all(&mut self) {
        self.owners.disown_all();
    }
    fn operation_result(&self) -> Option<Option<bool>> {
        self.operation.as_ref().map(|r| r.result)
    }
}

/// Implementation of the ResourceSpecs which is retrieved from the ResourceSpecsLocked.
/// During these calls, no other thread can add/remove elements from the list.
impl ResourceSpecs {
    /// Check if the given pool `id` has any replicas.
    fn pool_has_replicas(&self, id: &PoolId) -> bool {
        for replica in self.replicas.values() {
            let replica = replica.lock();
            if id == replica.pool_name() {
                return true;
            }
        }
        false
    }
    /// Check if the given pool `id` has any snapshots.
    fn pool_has_snapshots(&self, id: &PoolId) -> bool {
        for snapshot in self.volume_snapshots.values() {
            let snapshot = snapshot.lock();
            let transactions = snapshot.metadata().transactions();
            let this_pool = transactions
                .values()
                .flatten()
                .any(|r| r.spec().source_id().pool_id() == id);
            if this_pool {
                return true;
            }
        }
        false
    }
    /// Gets all ReplicaSpec's.
    pub(crate) fn replicas(&self) -> Vec<ReplicaSpec> {
        self.replicas
            .values()
            .map(|obj| obj.lock().clone())
            .collect::<Vec<_>>()
    }

    /// Get all PoolSpecs.
    pub(crate) fn pools(&self) -> Vec<PoolSpec> {
        self.pools
            .values()
            .map(|obj| obj.lock().clone())
            .collect::<Vec<_>>()
    }

    /// Get the nodes where these replicas reside on.
    pub(crate) fn replica_nodes<'a>(
        &self,
        replicas: impl Iterator<Item = &'a ResourceMutex<ReplicaSpec>>,
    ) -> Vec<NodeId> {
        replicas
            .flat_map(|r| self.replica_spec_node(r.immutable_ref()))
            .collect::<Vec<_>>()
    }
    /// Get the node where this replica resides on.
    pub(crate) fn replica_spec_node(&self, replica: &ReplicaSpec) -> Option<NodeId> {
        self.pools
            .get(replica.pool_name())
            .map(|pool| pool.lock().node.clone())
    }
}

impl ResourceSpecsLocked {
    /// Get the guarded ReplicaSpec for the given replica `id`, if any exists.
    pub(crate) async fn replica_opt(
        &self,
        replica: &ReplicaId,
    ) -> Result<Option<OperationGuardArc<ReplicaSpec>>, SvcError> {
        Ok(match self.replica_rsc(replica) {
            None => None,
            Some(replica) => Some(replica.operation_guard_wait().await?),
        })
    }
    /// Get the guarded ReplicaSpec for the given replica `id`, if any exists.
    pub(crate) async fn replica(
        &self,
        replica: &ReplicaId,
    ) -> Result<OperationGuardArc<ReplicaSpec>, SvcError> {
        match self.replica_rsc(replica) {
            None => Err(SvcError::ReplicaNotFound {
                replica_id: replica.clone(),
            }),
            Some(replica) => Ok(replica.operation_guard_wait().await?),
        }
    }

    /// Get or Create the resourced ReplicaSpec for the given request.
    pub(crate) fn get_or_create_replica(
        &self,
        request: &CreateReplica,
    ) -> ResourceMutex<ReplicaSpec> {
        let mut specs = self.write();
        if let Some(replica) = specs.replicas.get(&request.uuid) {
            replica.clone()
        } else {
            specs.replicas.insert(ReplicaSpec::from(request))
        }
    }
    /// Get a resourced ReplicaSpec for the given replica `id`, if it exists.
    pub(crate) fn replica_rsc(&self, id: &ReplicaId) -> Option<ResourceMutex<ReplicaSpec>> {
        let specs = self.read();
        specs.replicas.get(id).cloned()
    }

    /// Get the node where the replica `id` resides on, if it exists.
    pub(crate) fn replica_id_node(&self, id: &ReplicaId) -> Option<NodeId> {
        let specs = self.read();
        let replica = specs.replicas.get(id)?;
        specs.replica_spec_node(replica.immutable_ref())
    }

    /// Get or Create the resourced PoolSpec for the given request.
    pub(crate) fn get_or_create_pool(&self, request: &CreatePool) -> ResourceMutex<PoolSpec> {
        let mut specs = self.write();
        if let Some(pool) = specs.pools.get(&request.id) {
            pool.clone()
        } else {
            specs.pools.insert(PoolSpec::from(request))
        }
    }
    /// Get a resourced PoolSpec for the given pool `id`, if it exists.
    pub(crate) fn pool_rsc(&self, id: &PoolId) -> Option<ResourceMutex<PoolSpec>> {
        let specs = self.read();
        specs.pools.get(id).cloned()
    }
    /// Get a PoolSpec for the given pool `id`, if it exists.
    pub(crate) fn pool(&self, id: &PoolId) -> Result<PoolSpec, SvcError> {
        let specs = self.read();
        specs
            .pools
            .get(id)
            .map(|p| p.lock().clone())
            .ok_or(PoolNotFound {
                pool_id: id.to_owned(),
            })
    }
    /// Get a pools's node from its spec for the given pool `id`, if it exists.
    pub(crate) fn spec_pool_node(&self, id: &PoolId) -> Option<NodeId> {
        let specs = self.read();
        specs.pools.get(id).map(|p| p.immutable_arc().node.clone())
    }
    /// Get a vector of resourced PoolSpec's.
    pub(crate) fn pools_rsc(&self) -> Vec<ResourceMutex<PoolSpec>> {
        let specs = self.read();
        specs.pools.to_vec()
    }
    /// Get a vector of PoolSpec's.
    pub(crate) fn pools(&self) -> Vec<PoolSpec> {
        self.read().pools()
    }
    /// Check if the given pool `id` has any replicas.
    fn pool_has_replicas(&self, id: &PoolId) -> bool {
        let specs = self.read();
        specs.pool_has_replicas(id)
    }
    /// Check if the given pool `id` has any snapshots.
    fn pool_has_snapshots(&self, id: &PoolId) -> bool {
        let specs = self.read();
        specs.pool_has_snapshots(id)
    }
    /// Remove the replica `id` from the spec list.
    fn remove_replica(&self, id: &ReplicaId) {
        let mut specs = self.write();
        specs.replicas.remove(id);
    }
    /// Remove the Pool `id` from the spec list.
    fn remove_pool(&self, id: &PoolId) {
        let mut specs = self.write();
        specs.pools.remove(id);
    }

    /// Get a vector of resourced ReplicaSpec's.
    pub(crate) fn replicas(&self) -> Vec<ResourceMutex<ReplicaSpec>> {
        let specs = self.read();
        specs.replicas.to_vec()
    }

    /// Get a vector of ReplicaSpec's.
    pub(crate) fn replicas_cloned(&self) -> Vec<ReplicaSpec> {
        let specs = self.read();
        specs.replicas()
    }

    /// Worker that reconciles dirty PoolSpec's with the persistent store.
    /// This is useful when pool operations are performed but we fail to
    /// update the spec with the persistent store.
    pub(crate) async fn reconcile_dirty_pools(&self, registry: &Registry) -> bool {
        let mut pending_ops = false;

        let pools = self.pools_rsc();
        for pool in pools {
            if let Ok(mut guard) = pool.operation_guard() {
                let on_fail = guard.on_create_fail(registry);

                if !guard.handle_incomplete_ops_ext(registry, on_fail).await {
                    // Not all pending operations could be handled.
                    pending_ops = true;
                }
            }
        }
        pending_ops
    }

    /// Worker that reconciles dirty ReplicaSpec's with the persistent store.
    /// This is useful when replica operations are performed but we fail to
    /// update the spec with the persistent store.
    pub(crate) async fn reconcile_dirty_replicas(&self, registry: &Registry) -> bool {
        let mut pending_ops = false;

        let replicas = self.replicas();
        for replica in replicas {
            if let Ok(mut guard) = replica.operation_guard() {
                if !guard.handle_incomplete_ops(registry).await {
                    // Not all pending operations could be handled.
                    pending_ops = true;
                }
            }
        }
        pending_ops
    }

    /// Get guarded pool spec by its `PoolId`.
    pub async fn guarded_pool(
        &self,
        pool_id: &PoolId,
    ) -> Result<OperationGuardArc<PoolSpec>, SvcError> {
        match self.pool_rsc(pool_id) {
            Some(pool) => pool.operation_guard_wait().await,
            None => Err(PoolNotFound {
                pool_id: pool_id.to_owned(),
            }),
        }
    }
}
