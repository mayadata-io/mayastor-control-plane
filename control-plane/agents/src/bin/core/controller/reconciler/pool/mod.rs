mod capacity;

use crate::controller::{
    reconciler::{GarbageCollect, ReCreate},
    resources::{
        operations::ResourceLifecycle,
        operations_helper::{OperationSequenceGuard, SpecOperationsHelper},
        OperationGuardArc, TraceSpan,
    },
    task_poller::{PollContext, PollPeriods, PollResult, PollTimer, PollerState, TaskPoller},
};

use crate::controller::io_engine::PoolApi;
use stor_port::types::v0::{
    store::pool::PoolSpec,
    transport::{DestroyPool, ImportPool, NodeStatus},
};
use tracing::Instrument;

/// Pool Reconciler loop which:
/// 1. recreates pools which are not present following an io-engine restart
#[derive(Debug)]
pub(crate) struct PoolReconciler {
    counter: PollTimer,
}
impl PoolReconciler {
    /// Return new `Self` with the provided period
    pub(crate) fn from(period: PollPeriods) -> Self {
        PoolReconciler {
            counter: PollTimer::from(period),
        }
    }
    /// Return new `Self` with the default period
    pub(crate) fn new() -> Self {
        Self::from(1)
    }
}

#[async_trait::async_trait]
impl TaskPoller for PoolReconciler {
    async fn poll(&mut self, context: &PollContext) -> PollResult {
        let pools = context.specs().pools_rsc();
        let mut results = Vec::with_capacity(pools.len());
        for pool in pools {
            if pool.lock().dirty() {
                continue;
            }
            let mut pool = match pool.operation_guard() {
                Ok(guard) => guard,
                Err(_) => continue,
            };

            results.push(Self::squash_results(vec![
                pool.garbage_collect(context).await,
                pool.recreate_state(context).await,
                retry_create_pool_reconciler(&mut pool, context).await,
            ]))
        }
        capacity::remove_larger_replicas(context.registry()).await;
        Self::squash_results(results)
    }

    async fn poll_timer(&mut self, _context: &PollContext) -> bool {
        self.counter.poll()
    }
}

#[async_trait::async_trait]
impl GarbageCollect for OperationGuardArc<PoolSpec> {
    async fn garbage_collect(&mut self, context: &PollContext) -> PollResult {
        self.destroy_deleting(context).await
    }

    async fn destroy_deleting(&mut self, context: &PollContext) -> PollResult {
        deleting_pool_spec_reconciler(self, context).await
    }

    async fn destroy_orphaned(&mut self, _context: &PollContext) -> PollResult {
        unimplemented!()
    }

    async fn disown_unused(&mut self, _context: &PollContext) -> PollResult {
        unimplemented!()
    }

    async fn disown_orphaned(&mut self, _context: &PollContext) -> PollResult {
        unimplemented!()
    }

    async fn disown_invalid(&mut self, _context: &PollContext) -> PollResult {
        unimplemented!()
    }
}

#[async_trait::async_trait]
impl ReCreate for OperationGuardArc<PoolSpec> {
    async fn recreate_state(&mut self, context: &PollContext) -> PollResult {
        missing_pool_state_reconciler(self, context).await
    }
}

/// If a pool has a spec but not state, it means that the io-engine instance where the pool should
/// exist does not have the pool open.
/// This can happen if the pool is destroyed under the control plane, or if the io-engine
/// crashed/restarted.
/// In such a case, we issue a new create pool request against the io-engine instance where the pool
/// should exist.
#[tracing::instrument(skip(pool, context), level = "trace", fields(pool.id = %pool.id(), request.reconcile = true))]
async fn missing_pool_state_reconciler(
    pool: &mut OperationGuardArc<PoolSpec>,
    context: &PollContext,
) -> PollResult {
    if !pool.as_ref().status().created() {
        // nothing to do here
        return PollResult::Ok(PollerState::Idle);
    }
    let pool_id = pool.id();

    if !context.registry().has_pool_state(pool_id).await {
        let pool_spec = pool.lock().clone();

        let warn_missing = |pool_spec: &PoolSpec, node_status: NodeStatus| {
            let node_id = &pool_spec.node;
            pool_spec.trace_span(|| {
                tracing::trace!(
                    node.id = %node_id,
                    node.status = %node_status.to_string(),
                    "Attempted to recreate missing pool state, but the node is not online"
                )
            });
        };
        let node = match context.registry().node_wrapper(&pool_spec.node).await {
            Ok(node) if !node.read().await.is_online() => {
                let node_status = node.read().await.status();
                warn_missing(&pool_spec, node_status);
                return PollResult::Ok(PollerState::Idle);
            }
            Err(_) => {
                warn_missing(&pool_spec, NodeStatus::Unknown);
                return PollResult::Ok(PollerState::Idle);
            }
            Ok(node) => node,
        };

        async {
            pool_spec.warn_span(|| tracing::warn!("Attempting to recreate missing pool"));

            let request = ImportPool::new(&pool_spec.node, &pool_spec.id, &pool_spec.disks);
            match node.import_pool(&request).await {
                Ok(_) => {
                    pool_spec.info_span(|| tracing::info!("Pool successfully recreated"));
                    PollResult::Ok(PollerState::Idle)
                }
                Err(error) => {
                    pool_spec.error_span(
                        || tracing::error!(error=%error, "Failed to recreate the pool"),
                    );
                    Err(error)
                }
            }
        }
        .instrument(tracing::info_span!("missing_pool_state_reconciler", pool.id = %pool_spec.id, request.reconcile = true))
        .await
    } else {
        PollResult::Ok(PollerState::Idle)
    }
}

/// If a pool is tried to be deleted after its corresponding io-engine node is down,
/// the pool deletion gets struck in Deleting state, this creates a problem as when
/// the node comes up we cannot create a pool with same specs, the deleting_pool_spec_reconciler
/// cleans up any such pool when node comes up.
#[tracing::instrument(skip(pool, context), level = "trace", fields(pool.id = %pool.id(), request.reconcile = true))]
async fn deleting_pool_spec_reconciler(
    pool: &mut OperationGuardArc<PoolSpec>,
    context: &PollContext,
) -> PollResult {
    if !pool.as_ref().status().deleting() {
        // nothing to do here
        return PollResult::Ok(PollerState::Idle);
    }

    match context.registry().node_wrapper(&pool.as_ref().node).await {
        Ok(node) => {
            if !node.read().await.is_online() {
                return PollResult::Ok(PollerState::Idle);
            }
        }
        Err(_) => return PollResult::Ok(PollerState::Idle),
    };

    let pool_id = &pool.immutable_arc().id;
    async {
        let request = DestroyPool {
            node: pool.as_ref().node.clone(),
            id: pool.as_ref().id.clone(),
        };
        match pool.destroy(context.registry(), &request).await {
            Ok(_) => {
                pool.as_ref()
                    .info_span(|| tracing::info!("Pool deleted successfully"));
                PollResult::Ok(PollerState::Idle)
            }
            Err(error) => {
                pool.as_ref()
                    .error_span(|| tracing::error!(error=%error, "Failed to delete the pool"));
                Err(error)
            }
        }
    }
    .instrument(tracing::info_span!(
        "deleting_pool_spec_reconciler",
        pool.id = %pool_id,
        request.reconcile = true
    ))
    .await
}

#[tracing::instrument(skip(pool, context), level = "trace", fields(pool.id = %pool.id(), request.reconcile = true))]
async fn retry_create_pool_reconciler(
    pool: &mut OperationGuardArc<PoolSpec>,
    context: &PollContext,
) -> PollResult {
    pool.retry_creating(context.registry()).await?;
    Ok(PollerState::Idle)
}
