//! The internal node plugin gRPC service.
//! This provides access to functionality that needs to be executed on the same
//! node as a IoEngine CSI node plugin, but it is not possible to do so within
//! the CSI framework. This service must be deployed on all nodes the
//! IoEngine CSI node plugin is deployed.

use crate::{
    dev::Device,
    findmnt,
    fsfreeze::{fsfreeze, FsFreezeOpt},
    mount::lazy_unmount_mountpaths,
    nodeplugin_svc,
    nodeplugin_svc::{find_mount, lookup_device},
    runtime,
    shutdown_event::Shutdown,
};
use csi_driver::{
    limiter::VolumeOpGuard,
    node::internal::{
        node_plugin_server::{NodePlugin, NodePluginServer},
        FindVolumeReply, FindVolumeRequest, ForceUnstageVolumeReply, ForceUnstageVolumeRequest,
        FreezeFsReply, FreezeFsRequest, UnfreezeFsReply, UnfreezeFsRequest, VolumeType,
    },
};
use nodeplugin_svc::TypeOfMount;
use nvmeadm::{error::NvmeError, nvmf_subsystem::Subsystem};
use utils::nvme_target_nqn_prefix;

use tonic::{transport::Server, Request, Response, Status};
use tracing::{debug, error, info};
use uuid::Uuid;

#[derive(Debug, Default)]
pub(crate) struct NodePluginSvc {}

#[tonic::async_trait]
impl NodePlugin for NodePluginSvc {
    #[tracing::instrument(err, skip_all)]
    async fn freeze_fs(
        &self,
        request: Request<FreezeFsRequest>,
    ) -> Result<Response<FreezeFsReply>, Status> {
        let volume_id = request.into_inner().volume_id;
        fsfreeze(&volume_id, FsFreezeOpt::Freeze).await?;
        Ok(Response::new(FreezeFsReply {}))
    }

    #[tracing::instrument(err, skip_all)]
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

    #[tracing::instrument(err, fields(volume.uuid = request.get_ref().volume_id), skip(self, request))]
    async fn force_unstage_volume(
        &self,
        request: Request<ForceUnstageVolumeRequest>,
    ) -> Result<Response<ForceUnstageVolumeReply>, Status> {
        let volume_id = request.into_inner().volume_id;

        let _guard = VolumeOpGuard::new_str(&volume_id)?;

        debug!("Starting cleanup for volume: {volume_id}");

        let nqn = format!("{}:{volume_id}", nvme_target_nqn_prefix());
        match Subsystem::try_from_nqn(&nqn) {
            Ok(subsystem_paths) => {
                for subsystem_path in subsystem_paths {
                    info!("Processing subsystem: {subsystem_path:?}");
                    if !subsystem_path.state.contains("deleting") {
                        runtime::spawn_blocking(move || {
                            subsystem_path
                                .disconnect()
                                .map_err(|error| Status::aborted(error.to_string()))
                        })
                        .await
                        .map_err(|error| Status::aborted(error.to_string()))??;
                    }
                }
                Err(Status::internal(format!(
                    "Cleanup initiated for all stale entries of: {volume_id}. Returning error for validation on retry."
                )))
            }
            Err(NvmeError::NqnNotFound { .. }) => {
                let uuid = Uuid::parse_str(&volume_id).map_err(|error| {
                    Status::invalid_argument(format!("Invalid volume UUID: {volume_id}, {error}"))
                })?;

                if let Ok(Some(device)) = Device::lookup(&uuid).await {
                    let mountpaths = findmnt::get_mountpaths(&device.devname())?;
                    debug!(
                        "Device: {} found, with mount paths: {}, issuing unmount",
                        device.devname(),
                        mountpaths
                            .iter()
                            .map(|devmount| devmount.to_string())
                            .collect::<Vec<String>>()
                            .join(", ")
                    );
                    lazy_unmount_mountpaths(&mountpaths).await?;
                } else {
                    let mountpaths = findmnt::get_csi_mountpaths(&volume_id).await?;
                    debug!(
                        "Device was not found, detected mount paths: {}, issuing unmount",
                        mountpaths
                            .iter()
                            .map(|devmount| devmount.to_string())
                            .collect::<Vec<String>>()
                            .join(", ")
                    );
                    lazy_unmount_mountpaths(&mountpaths).await?;
                }
                Ok(Response::new(ForceUnstageVolumeReply {}))
            }
            Err(error) => Err(Status::aborted(error.to_string())),
        }
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
