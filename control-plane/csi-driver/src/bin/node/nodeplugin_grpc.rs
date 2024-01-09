//! The internal node plugin gRPC service.
//! This provides access to functionality that needs to be executed on the same
//! node as a IoEngine CSI node plugin, but it is not possible to do so within
//! the CSI framework. This service must be deployed on all nodes the
//! IoEngine CSI node plugin is deployed.
use crate::{
    fsfreeze::{fsfreeze, FsFreezeOpt},
    nodeplugin_svc,
    nodeplugin_svc::{find_mount, lookup_device},
    shutdown_event::Shutdown,
};
use csi_driver::node::internal::{
    node_plugin_server::{NodePlugin, NodePluginServer},
    FindVolumeReply, FindVolumeRequest, FreezeFsReply, FreezeFsRequest, UnfreezeFsReply,
    UnfreezeFsRequest, VolumeType,
};
use nodeplugin_svc::TypeOfMount;
use tonic::{transport::Server, Request, Response, Status};
use tracing::{debug, error, info};

#[derive(Debug, Default)]
pub(crate) struct NodePluginSvc {}

#[tonic::async_trait]
impl NodePlugin for NodePluginSvc {
    async fn freeze_fs(
        &self,
        request: Request<FreezeFsRequest>,
    ) -> Result<Response<FreezeFsReply>, Status> {
        let volume_id = request.into_inner().volume_id;
        fsfreeze(&volume_id, FsFreezeOpt::Freeze).await?;
        Ok(Response::new(FreezeFsReply {}))
    }

    async fn unfreeze_fs(
        &self,
        request: Request<UnfreezeFsRequest>,
    ) -> Result<Response<UnfreezeFsReply>, Status> {
        let volume_id = request.into_inner().volume_id;
        fsfreeze(&volume_id, FsFreezeOpt::Unfreeze).await?;
        Ok(Response::new(UnfreezeFsReply {}))
    }

    async fn find_volume(
        &self,
        request: Request<FindVolumeRequest>,
    ) -> Result<Response<FindVolumeReply>, Status> {
        let volume_id = request.into_inner().volume_id;
        debug!("find_volume({})", volume_id);
        let device = lookup_device(&volume_id).await?;
        let mount = find_mount(&volume_id, device.as_ref()).await?;
        Ok(Response::new(FindVolumeReply {
            volume_type: mount.map(Into::<VolumeType>::into).map(Into::into),
            device_path: device.devname(),
        }))
    }
}

impl From<TypeOfMount> for VolumeType {
    fn from(mount: TypeOfMount) -> Self {
        match mount {
            TypeOfMount::FileSystem => Self::Filesystem,
            TypeOfMount::RawBlock => Self::Rawblock,
        }
    }
}

/// The Grpc server which services a `NodePluginServer`.
pub(crate) struct NodePluginGrpcServer {}

impl NodePluginGrpcServer {
    /// Run `Self` as a tonic server.
    pub(crate) async fn run(endpoint: std::net::SocketAddr) -> anyhow::Result<()> {
        info!(
            "node plugin gRPC server configured at address {:?}",
            endpoint
        );
        Server::builder()
            .add_service(NodePluginServer::new(NodePluginSvc {}))
            .serve_with_shutdown(endpoint, Shutdown::wait())
            .await
            .map_err(|error| {
                use stor_port::transport_api::ErrorChain;
                error!(error = error.full_string(), "NodePluginGrpcServer failed");
                error.into()
            })
    }
}
