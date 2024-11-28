use crate::{
    context::{Client, Context, TracedChannel},
    jsongrpc::{json_grpc_client, json_grpc_reply, ProbeRequest},
    operations::jsongrpc::traits::{JsonGrpcOperations, JsonGrpcRequestInfo},
};
use serde_json::Value;
use std::{ops::Deref, time::Duration};
use stor_port::{
    transport_api::{ReplyError, ReplyErrorKind, ResourceKind, TimeoutOptions},
    types::v0::transport::MessageIdVs,
};
use tonic::transport::Uri;

/// RPC JsonGrpc Client
#[derive(Clone)]
pub struct JsonGrpcClient {
    inner: Client<json_grpc_client::JsonGrpcClient<TracedChannel>>,
}

impl JsonGrpcClient {
    /// creates a new base tonic endpoint with the timeout options and the address
    pub async fn new<O: Into<Option<TimeoutOptions>>>(addr: Uri, opts: O) -> Self {
        let client = Client::new(addr, opts, json_grpc_client::JsonGrpcClient::new).await;
        Self { inner: client }
    }
    /// Try to wait until the JsonGrpc Service is ready, up to a timeout, by using the Probe method.
    pub async fn wait_ready(&self, timeout_opts: Option<TimeoutOptions>) -> Result<(), ()> {
        let timeout_opts = match timeout_opts {
            Some(opts) => opts,
            None => TimeoutOptions::new()
                .with_req_timeout(Duration::from_millis(250))
                .with_max_retries(10),
        };
        for attempt in 1..=timeout_opts.max_retries().unwrap_or_default() {
            match self
                .probe(Some(Context::new(Some(timeout_opts.clone()))))
                .await
            {
                Ok(true) => return Ok(()),
                _ => {
                    let delay = std::time::Duration::from_millis(100);
                    if attempt < timeout_opts.max_retries().unwrap_or_default() {
                        tracing::trace!(%attempt, delay=?delay, "Not available, retrying after...");
                    }
                    tokio::time::sleep(delay).await;
                }
            }
        }
        tracing::error!("Timed out");
        Err(())
    }
}

impl Deref for JsonGrpcClient {
    type Target = Client<json_grpc_client::JsonGrpcClient<TracedChannel>>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[tonic::async_trait]
impl JsonGrpcOperations for JsonGrpcClient {
    #[tracing::instrument(name = "JsonGrpcClient::call", level = "debug", skip(self), err)]
    async fn call(
        &self,
        request: &dyn JsonGrpcRequestInfo,
        ctx: Option<Context>,
    ) -> Result<Value, ReplyError> {
        let req = self.request(request, ctx, MessageIdVs::JsonGrpc);
        let response = self.client().json_grpc_call(req).await?.into_inner();
        match response.reply {
            Some(json_grpc_reply) => match json_grpc_reply {
                json_grpc_reply::Reply::Response(value) => match serde_json::to_value(value) {
                    Ok(value) => Ok(value),
                    Err(err) => Err(ReplyError::serde_error(
                        ResourceKind::JsonGrpc,
                        ReplyErrorKind::DeserializeReq,
                        err,
                    )),
                },
                json_grpc_reply::Reply::Error(err) => Err(err.into()),
            },
            None => Err(ReplyError::invalid_response(ResourceKind::JsonGrpc)),
        }
    }
    #[tracing::instrument(name = "JsonGrpcClient::probe", level = "debug", skip(self))]
    async fn probe(&self, _ctx: Option<Context>) -> Result<bool, ReplyError> {
        match self.client().probe(ProbeRequest {}).await {
            Ok(resp) => Ok(resp.into_inner().ready),
            Err(e) => Err(e.into()),
        }
    }
}
