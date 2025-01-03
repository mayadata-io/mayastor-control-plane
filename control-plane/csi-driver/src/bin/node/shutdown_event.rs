/// Shutdown Event
pub(crate) struct Shutdown {}
impl Shutdown {
    /// Returns a future that completes when the shutdown signal has been received.
    pub(crate) async fn wait() {
        // We don't care which shutdown event it was..
        let _ = shutdown::Shutdown::wait_sig().await;
    }
}

#[cfg(test)]
mod tests {
    use csi_driver::node::internal::{
        node_plugin_client::NodePluginClient,
        node_plugin_server::{NodePlugin, NodePluginServer},
        FindVolumeReply, FindVolumeRequest, ForceUnstageVolumeReply, ForceUnstageVolumeRequest,
        FreezeFsReply, FreezeFsRequest, UnfreezeFsReply, UnfreezeFsRequest, VolumeType,
    };
    use std::{
        str::FromStr,
        sync::{Arc, Mutex},
        time::Duration,
    };
    use tonic::{
        transport::{Server, Uri},
        Code, Request, Response, Status,
    };

    #[derive(Debug, Default)]
    struct NodePluginSvc {
        first_call: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    }
    impl NodePluginSvc {
        fn new(chan: tokio::sync::oneshot::Sender<()>) -> Self {
            Self {
                first_call: Arc::new(Mutex::new(Some(chan))),
            }
        }
    }

    #[tonic::async_trait]
    impl NodePlugin for NodePluginSvc {
        async fn freeze_fs(
            &self,
            _request: Request<FreezeFsRequest>,
        ) -> Result<Response<FreezeFsReply>, Status> {
            unimplemented!()
        }

        async fn unfreeze_fs(
            &self,
            _request: Request<UnfreezeFsRequest>,
        ) -> Result<Response<UnfreezeFsReply>, Status> {
            unimplemented!()
        }

        async fn find_volume(
            &self,
            _request: Request<FindVolumeRequest>,
        ) -> Result<Response<FindVolumeReply>, Status> {
            {
                let mut inner = self.first_call.lock().unwrap();
                let sender = inner
                    .take()
                    .expect("Only the first call should get through!");
                sender.send(()).unwrap();
            }
            println!("Sleeping...");
            tokio::time::sleep(Duration::from_secs(2)).await;
            println!("Done...");
            Ok(Response::new(FindVolumeReply {
                volume_type: Some(VolumeType::Rawblock as i32),
                device_path: "".to_string(),
            }))
        }

        async fn force_unstage_volume(
            &self,
            _request: Request<ForceUnstageVolumeRequest>,
        ) -> Result<Response<ForceUnstageVolumeReply>, Status> {
            unimplemented!()
        }
    }

    /// Tests the shutdown of a tonic service.
    /// A shutdown event is issued after a "long" requests starts being processed. This request
    /// should be able to complete, even if it takes longer.
    /// However, meanwhile, any new requests should be rejected.
    #[tokio::test]
    async fn shutdown() {
        async fn wait(wait: tokio::sync::oneshot::Receiver<()>) {
            wait.await.unwrap();
        }
        let (first_sender, first_receiver) = tokio::sync::oneshot::channel();
        let (shutdown_sender, shutdown_receiver) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            if let Err(e) = Server::builder()
                .add_service(NodePluginServer::new(NodePluginSvc::new(first_sender)))
                .serve_with_shutdown("[::]:50011".parse().unwrap(), wait(shutdown_receiver))
                .await
            {
                panic!("gRPC server failed with error: {e}");
            }
        });
        tokio::time::sleep(Duration::from_millis(250)).await;
        let channel =
            tonic::transport::Endpoint::from(Uri::from_str("https://[::]:50011").unwrap())
                .connect()
                .await
                .unwrap();
        let mut cli = NodePluginClient::new(channel);

        // 1. schedule the first request
        let mut cli_first = cli.clone();
        let first_request = tokio::spawn(async move {
            cli_first
                .find_volume(FindVolumeRequest {
                    volume_id: "".to_string(),
                })
                .await
        });
        // 2. wait until the first request is being processed
        first_receiver.await.unwrap();

        // 3. send the shutdown event
        shutdown_sender.send(()).unwrap();
        // add a little slack so the shutdown can be processed
        tokio::time::sleep(Duration::from_millis(250)).await;

        // 4. try to send a new request which...
        let second_response = cli
            .find_volume(FindVolumeRequest {
                volume_id: "".to_string(),
            })
            .await;
        println!("Second Request Status: {second_response:?}");
        // should fail because we've started to shutdown!
        assert_eq!(second_response.unwrap_err().code(), Code::Cancelled);

        // 4. the initial request should complete gracefully
        let first_request_resp = first_request.await.unwrap();
        println!("First Request Response: {first_request_resp:?}");
        assert_eq!(
            first_request_resp.unwrap().into_inner().volume_type,
            Some(VolumeType::Rawblock as i32)
        );
    }
}
