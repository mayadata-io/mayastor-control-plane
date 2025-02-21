use super::translation::{rpc_pool_to_agent, AgentToIoEngine};
use crate::controller::io_engine::v1::translation::TryAgentToIoEngine;
use agents::errors::{GrpcRequest as GrpcRequestError, SvcError};
use rpc::v1::pool::ListPoolOptions;
use stor_port::{
    transport_api::ResourceKind,
    types::v0::transport::{CreatePool, DestroyPool, ImportPool, PoolState},
};

use snafu::ResultExt;

#[async_trait::async_trait]
impl crate::controller::io_engine::PoolListApi for super::RpcClient {
    async fn list_pools(&self) -> Result<Vec<PoolState>, SvcError> {
        let rpc_pools = self
            .pool()
            .list_pools(ListPoolOptions::default())
            .await
            .context(GrpcRequestError {
                resource: ResourceKind::Pool,
                request: "list_pools",
            })?;
        let rpc_pools = &rpc_pools.get_ref().pools;
        let pools = rpc_pools
            .iter()
            .map(|p| rpc_pool_to_agent(p, self.context.node()))
            .collect();
        Ok(pools)
    }
}

#[async_trait::async_trait]
impl crate::controller::io_engine::PoolApi for super::RpcClient {
    #[tracing::instrument(name = "rpc::v1::pool::create", level = "debug", skip(self), err)]
    async fn create_pool(&self, request: &CreatePool) -> Result<PoolState, SvcError> {
        match self.pool().create_pool(request.to_rpc()?).await {
            Ok(rpc_pool) => {
                let pool = rpc_pool_to_agent(&rpc_pool.into_inner(), &request.node);
                Ok(pool)
            }
            Err(error)
                if error.code() == tonic::Code::Internal
                    && error.message()
                        == format!(
                            "Failed to create a BDEV '{}'",
                            request.disks.first().cloned().unwrap_or_default()
                        ) =>
            {
                Err(SvcError::GrpcRequestError {
                    resource: ResourceKind::Pool,
                    request: "create_pool".to_string(),
                    source: tonic::Status::not_found(error.message()),
                })
            }
            Err(error) => Err(error).context(GrpcRequestError {
                resource: ResourceKind::Pool,
                request: "create_pool",
            }),
        }
    }

    #[tracing::instrument(name = "rpc::v1::pool::destroy", level = "debug", skip(self), err)]
    async fn destroy_pool(&self, request: &DestroyPool) -> Result<(), SvcError> {
        let _ = self
            .pool()
            .destroy_pool(request.to_rpc())
            .await
            .context(GrpcRequestError {
                resource: ResourceKind::Pool,
                request: "destroy_pool",
            })?;
        Ok(())
    }

    #[tracing::instrument(name = "rpc::v1::pool::import", level = "debug", skip(self), err)]
    async fn import_pool(&self, request: &ImportPool) -> Result<PoolState, SvcError> {
        let rpc_pool =
            self.pool()
                .import_pool(request.to_rpc()?)
                .await
                .context(GrpcRequestError {
                    resource: ResourceKind::Pool,
                    request: "import_pool",
                })?;
        let pool = rpc_pool_to_agent(&rpc_pool.into_inner(), &request.node);
        Ok(pool)
    }
}
