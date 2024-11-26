use crate::{
    controller::{
        io_engine::PoolApi,
        registry::Registry,
        resources::{
            operations::ResourceLifecycle,
            operations_helper::{GuardedOperationsHelper, OnCreateFail, SpecOperationsHelper},
            OperationGuardArc,
        },
    },
    node::wrapper::GetterOps,
};
use agents::{errors, errors::SvcError};
use snafu::OptionExt;
use std::ops::Deref;
use stor_port::types::v0::{
    store::{
        pool::PoolSpec,
        replica::{PoolRef, ReplicaSpec},
    },
    transport::{CreatePool, DestroyReplica, NodeId, ReplicaOwners},
};

impl OperationGuardArc<PoolSpec> {
    /// Retries the creation of the pool which is being done in the background by the io-engine.
    /// This may happen if the pool create gRPC times out, for very large pools.
    /// We could increase the timeout but as things stand today that would block all gRPC
    /// access to the node.
    /// TODO: Since the data-plane now allows concurrent gRPC we should also modify the
    ///  control-plane to allow this, which would allows to set large timeouts for some gRPCs.
    pub(crate) async fn retry_creating(&mut self, registry: &Registry) -> Result<(), SvcError> {
        let request = {
            let spec = self.lock();
            if on_create_fail(&spec, registry).is_some() {
                return Ok(());
            }
            CreatePool::from(spec.deref())
        };

        let node = registry.node_wrapper(&request.node).await?;
        if node.pool(&request.id).await.is_none() {
            return Ok(());
        }

        let _ = self.start_create(registry, &request).await?;
        let result = node.create_pool(&request).await;
        let _state = self
            .complete_create(result, registry, OnCreateFail::LeaveAsIs)
            .await?;

        Ok(())
    }

    /// Ge the `OnCreateFail` policy.
    /// For more information see [`Self::retry_creating`].
    pub(crate) fn on_create_fail(&self, registry: &Registry) -> OnCreateFail {
        let spec = self.lock();
        on_create_fail(&spec, registry).unwrap_or(OnCreateFail::LeaveAsIs)
    }
}

fn on_create_fail(pool: &PoolSpec, registry: &Registry) -> Option<OnCreateFail> {
    if !pool.status().creating() {
        return Some(OnCreateFail::LeaveAsIs);
    }
    let Some(last_mod_elapsed) = pool.creat_tsc.and_then(|t| t.elapsed().ok()) else {
        return Some(OnCreateFail::SetDeleting);
    };
    if last_mod_elapsed > registry.pool_async_creat_tmo() {
        return Some(OnCreateFail::SetDeleting);
    }
    None
}

impl OperationGuardArc<ReplicaSpec> {
    /// Destroy the replica from its volume
    pub(crate) async fn destroy_volume_replica(
        &mut self,
        registry: &Registry,
        node_id: Option<&NodeId>,
    ) -> Result<(), SvcError> {
        let node_id = match node_id {
            Some(node_id) => node_id.clone(),
            None => {
                let replica_uuid = self.lock().uuid.clone();
                match registry.replica(&replica_uuid).await {
                    Ok(state) => state.node.clone(),
                    Err(_) => {
                        let pool_ref = self.lock().pool.clone();
                        let pool_id = match pool_ref {
                            PoolRef::Named(name) => name,
                            PoolRef::Uuid(name, _) => name,
                        };
                        let pool_spec = registry
                            .specs()
                            .pool_rsc(&pool_id)
                            .context(errors::PoolNotFound { pool_id })?;
                        let node_id = pool_spec.lock().node.clone();
                        node_id
                    }
                }
            }
        };

        self.destroy(
            registry,
            &self.destroy_request(ReplicaOwners::new_disown_all(), &node_id),
        )
        .await
    }

    /// Return a `DestroyReplica` request based on the provided arguments
    pub(crate) fn destroy_request(&self, by: ReplicaOwners, node: &NodeId) -> DestroyReplica {
        let spec = self.as_ref().clone();
        let pool_id = match spec.pool.clone() {
            PoolRef::Named(id) => id,
            PoolRef::Uuid(id, _) => id,
        };
        let pool_uuid = match spec.pool {
            PoolRef::Named(_) => None,
            PoolRef::Uuid(_, uuid) => Some(uuid),
        };
        DestroyReplica {
            node: node.clone(),
            pool_id,
            pool_uuid,
            uuid: spec.uuid,
            name: spec.name.into(),
            disowners: by,
        }
    }
}
