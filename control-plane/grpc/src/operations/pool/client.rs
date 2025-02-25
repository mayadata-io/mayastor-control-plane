use crate::{
    common::{CommonFilter, NodeFilter, NodePoolFilter, PoolFilter},
    context::{Client, Context, TracedChannel},
    operations::pool::traits::{CreatePoolInfo, DestroyPoolInfo, LabelPoolInfo, PoolOperations},
    pool::{
        create_pool_reply, get_pools_reply, get_pools_request, label_pool_reply,
        pool_grpc_client::PoolGrpcClient, unlabel_pool_reply, GetPoolsRequest,
    },
};
use std::{convert::TryFrom, ops::Deref};
use stor_port::{
    transport_api::{v0::Pools, ReplyError, ResourceKind, TimeoutOptions},
    types::v0::transport::{Filter, MessageIdVs, Pool},
};
use tonic::transport::Uri;

use super::traits::UnlabelPoolInfo;

/// RPC Pool Client
#[derive(Clone)]
pub struct PoolClient {
    inner: Client<PoolGrpcClient<TracedChannel>>,
}
impl Deref for PoolClient {
    type Target = Client<PoolGrpcClient<TracedChannel>>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl PoolClient {
    /// creates a new base tonic endpoint with the timeout options and the address
    pub async fn new<O: Into<Option<TimeoutOptions>>>(addr: Uri, opts: O) -> Self {
        let client = Client::new(addr, opts, PoolGrpcClient::new).await;
        Self { inner: client }
    }
}

/// Implement pool operations supported by the Pool RPC client.
/// This converts the client side data into a RPC request.
#[tonic::async_trait]
impl PoolOperations for PoolClient {
    #[tracing::instrument(name = "PoolClient::create", level = "info", skip(self), err)]
    async fn create(
        &self,
        request: &dyn CreatePoolInfo,
        ctx: Option<Context>,
    ) -> Result<Pool, ReplyError> {
        let req = self.request(request, ctx, MessageIdVs::CreatePool);
        let response = self.client().create_pool(req).await?.into_inner();
        match response.reply {
            Some(create_pool_reply) => match create_pool_reply {
                create_pool_reply::Reply::Pool(pool) => Ok(Pool::try_from(pool)?),
                create_pool_reply::Reply::Error(err) => Err(err.into()),
            },
            None => Err(ReplyError::invalid_response(ResourceKind::Pool)),
        }
    }

    #[tracing::instrument(name = "PoolClient::destroy", level = "info", skip(self), err)]
    async fn destroy(
        &self,
        request: &dyn DestroyPoolInfo,
        ctx: Option<Context>,
    ) -> Result<(), ReplyError> {
        let req = self.request(request, ctx, MessageIdVs::DestroyPool);
        let response = self.client().destroy_pool(req).await?.into_inner();
        match response.error {
            None => Ok(()),
            Some(err) => Err(err.into()),
        }
    }

    #[tracing::instrument(name = "PoolClient::get", level = "debug", skip(self), err)]
    async fn get(&self, filter: Filter, ctx: Option<Context>) -> Result<Pools, ReplyError> {
        let req: GetPoolsRequest = match filter {
            Filter::Node(id) => GetPoolsRequest {
                filter: Some(get_pools_request::Filter::Node(NodeFilter {
                    node_id: id.into(),
                })),
            },
            Filter::Pool(id) => GetPoolsRequest {
                filter: Some(get_pools_request::Filter::Pool(PoolFilter {
                    pool_id: id.into(),
                })),
            },
            Filter::NodePool(node_id, pool_id) => GetPoolsRequest {
                filter: Some(get_pools_request::Filter::NodePool(NodePoolFilter {
                    node_id: node_id.into(),
                    pool_id: pool_id.into(),
                })),
            },
            Filter::Volume(volume_id) => GetPoolsRequest {
                filter: Some(get_pools_request::Filter::Common(CommonFilter {
                    volume_id: volume_id.into(),
                })),
            },
            _ => GetPoolsRequest { filter: None },
        };
        let req = self.request(req, ctx, MessageIdVs::GetPools);
        let response = self.client().get_pools(req).await?.into_inner();
        match response.reply {
            Some(get_pools_reply) => match get_pools_reply {
                get_pools_reply::Reply::Pools(pools) => Ok(Pools::try_from(pools)?),
                get_pools_reply::Reply::Error(err) => Err(err.into()),
            },
            None => Err(ReplyError::invalid_response(ResourceKind::Pool)),
        }
    }

    #[tracing::instrument(name = "PoolClient::label", level = "debug", skip(self), err)]
    async fn label(
        &self,
        request: &dyn LabelPoolInfo,
        ctx: Option<Context>,
    ) -> Result<Pool, ReplyError> {
        let req = self.request(request, ctx, MessageIdVs::LabelPool);
        let response = self.client().label_pool(req).await?.into_inner();
        match response.reply {
            Some(label_pool_reply) => match label_pool_reply {
                label_pool_reply::Reply::Pool(pool) => Ok(Pool::try_from(pool)?),
                label_pool_reply::Reply::Error(err) => Err(err.into()),
            },
            None => Err(ReplyError::invalid_response(ResourceKind::Pool)),
        }
    }

    #[tracing::instrument(name = "PoolClient::unlabel", level = "debug", skip(self), err)]
    async fn unlabel(
        &self,
        request: &dyn UnlabelPoolInfo,
        ctx: Option<Context>,
    ) -> Result<Pool, ReplyError> {
        let req = self.request(request, ctx, MessageIdVs::UnlabelPool);
        let response = self.client().unlabel_pool(req).await?.into_inner();
        match response.reply {
            Some(unlabel_pool_reply) => match unlabel_pool_reply {
                unlabel_pool_reply::Reply::Pool(pool) => Ok(Pool::try_from(pool)?),
                unlabel_pool_reply::Reply::Error(err) => Err(err.into()),
            },
            None => Err(ReplyError::invalid_response(ResourceKind::Pool)),
        }
    }
}
