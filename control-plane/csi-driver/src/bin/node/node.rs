use crate::{
    block_vol::{publish_block_volume, unpublish_block_volume},
    client::VolumesClientWrapper,
    dev::{sysfs_dev_size, Device},
    filesystem_ops::FileSystem,
    filesystem_vol::{publish_fs_volume, stage_fs_volume, unpublish_fs_volume, unstage_fs_volume},
    mount::find_mount,
};
use csi_driver::{
    csi::volume_capability::{access_mode::Mode, AccessType},
    filesystem::FileSystem as Fs,
    limiter::VolumeOpGuard,
};
use rpc::{
    csi,
    csi::{
        node_server, node_service_capability, NodeExpandVolumeRequest, NodeExpandVolumeResponse,
        NodeGetCapabilitiesRequest, NodeGetCapabilitiesResponse, NodeGetInfoRequest,
        NodeGetInfoResponse, NodeGetVolumeStatsRequest, NodeGetVolumeStatsResponse,
        NodePublishVolumeRequest, NodePublishVolumeResponse, NodeServiceCapability,
        NodeStageVolumeRequest, NodeStageVolumeResponse, NodeUnpublishVolumeRequest,
        NodeUnpublishVolumeResponse, NodeUnstageVolumeRequest, NodeUnstageVolumeResponse, Topology,
        VolumeCapability,
    },
};

use nix::{errno::Errno, sys};
use std::{collections::HashMap, path::Path, time::Duration, vec::Vec};
use tonic::{Code, Request, Response, Status};
use tracing::{debug, error, info, instrument};
use uuid::Uuid;

macro_rules! failure {
    (Code::$code:ident, $msg:literal) => {{ error!($msg); Status::new(Code::$code, $msg) }};
    (Code::$code:ident, $fmt:literal $(,$args:expr)+) => {{ let message = format!($fmt $(,$args)+); error!("{}", message); Status::new(Code::$code, message) }};
}

/// The Csi Node implementation.
#[derive(Clone)]
pub(crate) struct Node {
    node_name: String,
    node_selector: HashMap<String, String>,
    filesystems: Vec<FileSystem>,
    vol_client: Option<VolumesClientWrapper>,
}

impl Node {
    /// Creates new node.
    pub(crate) fn new(
        node_name: String,
        node_selector: HashMap<String, String>,
        filesystems: Vec<FileSystem>,
        vol_client: Option<VolumesClientWrapper>,
    ) -> Node {
        let self_ = Self {
            node_name,
            node_selector,
            filesystems,
            vol_client,
        };
        info!("Node topology segments: {:?}", self_.segments());
        self_
    }
    /// Get the node_name label segment.
    fn node_name_segment(&self) -> (String, String) {
        (csi_driver::node_name_topology_key(), self.node_name.clone())
    }
    /// Get the node selector label segment.
    fn node_selector_segment(&self) -> HashMap<String, String> {
        self.node_selector.clone()
    }
    /// Get the topology segments.
    fn segments(&self) -> HashMap<String, String> {
        self.node_selector_segment()
            .into_iter()
            .chain(vec![self.node_name_segment()])
            .collect()
    }

    /// Get the target URI for the given volume.
    /// If the REST volume client is enabled, use it to fetch the volume and its target URI.
    /// Otherwise, use the publish context.
    async fn volume_uri(
        &self,
        volume_id: &Uuid,
        context: &HashMap<String, String>,
    ) -> Result<String, tonic::Status> {
        let ctx_uri = context.get("uri").ok_or_else(|| {
            failure!(
                Code::InvalidArgument,
                "Failed to stage volume {volume_id}: URI attribute missing from publish context"
            )
        })?;

        let Some(client) = &self.vol_client else {
            return Ok(ctx_uri.to_string());
        };

        let uri = client.volume_uri(volume_id).await?;
        if &uri != ctx_uri {
            tracing::warn!(volume.uri=%uri, ctx.uri=%ctx_uri, "Overriding URI volume context with URI from REST");
        }
        Ok(uri)
    }
}

const ATTACH_TIMEOUT_INTERVAL: Duration = Duration::from_millis(100);
const ATTACH_RETRIES: u32 = 100;

// Determine if given access mode in conjunction with ro mount flag makes
// sense or not. If access mode is not supported or the combination does
// not make sense, return error string.
//
// NOTE: Following is based on our limited understanding of access mode
// meaning. Access mode does not control if the mount is rw/ro (that is
// rather part of the mount flags). Access mode serves as advisory info
// for CO when attaching volumes to pods. It is out of scope of storage
// plugin running on particular node to check that access mode for particular
// publish or stage request makes sense.

/// Check that the access_mode from VolumeCapability is consistent with
/// the readonly status
fn check_access_mode(
    volume_capability: &Option<VolumeCapability>,
    readonly: bool,
) -> Result<(), String> {
    match volume_capability {
        Some(capability) => match &capability.access_mode {
            Some(access) => match Mode::try_from(access.mode) {
                Ok(mode) => match mode {
                    Mode::SingleNodeWriter | Mode::MultiNodeSingleWriter => Ok(()),
                    Mode::SingleNodeReaderOnly | Mode::MultiNodeReaderOnly => {
                        if readonly {
                            return Ok(());
                        }
                        Err(format!("volume capability: invalid combination of access mode ({mode:?}) and mount flag (rw)"))
                    }
                    Mode::Unknown => Err(String::from("volume capability: unknown access mode")),
                    _ => Err(format!(
                        "volume capability: unsupported access mode: {mode:?}"
                    )),
                },
                Err(_) => Err(format!(
                    "volume capability: invalid access mode: {}",
                    access.mode
                )),
            },
            None => Err(String::from("volume capability: missing access mode")),
        },
        None => Err(String::from("missing volume capability")),
    }
}

/// Retrieve the AccessType from VolumeCapability
fn get_access_type(volume_capability: &Option<VolumeCapability>) -> Result<&AccessType, String> {
    match volume_capability {
        Some(capability) => match &capability.access_type {
            Some(access) => Ok(access),
            None => Err(String::from("volume capability: missing access type")),
        },
        None => Err(String::from("missing volume capability")),
    }
}

/// Detach the nexus device from the system, either at volume unstage,
/// or after failed filesystem mount at volume stage.
async fn detach(uuid: &Uuid, errheader: String) -> Result<(), Status> {
    if let Some(device) = Device::lookup(uuid).await.map_err(|error| {
        failure!(
            Code::Internal,
            "{} error locating device: {}",
            &errheader,
            error
        )
    })? {
        let device_path = device.devname();
        debug!("Detaching device {}", device_path);

        let mounts = crate::mount::find_src_mounts(&device_path, None);
        if !mounts.is_empty() {
            return Err(failure!(
                Code::FailedPrecondition,
                "{} device is still mounted {}: {:?}",
                errheader,
                device_path,
                mounts
            ));
        }

        crate::mount::wait_fs_shutdown(&device_path, None).await?;

        if let Err(error) = device.detach().await {
            return Err(failure!(
                Code::Internal,
                "{} failed to detach device {}: {}",
                errheader,
                device_path,
                error
            ));
        }
    }
    Ok(())
}

#[tonic::async_trait]
impl node_server::Node for Node {
    async fn node_get_info(
        &self,
        _request: Request<NodeGetInfoRequest>,
    ) -> Result<Response<NodeGetInfoResponse>, Status> {
        let node_id = self.node_name.clone();

        debug!(node.id = node_id, "NodeGetInfo request");

        Ok(Response::new(NodeGetInfoResponse {
            node_id,
            max_volumes_per_node: 0,
            accessible_topology: Some(Topology {
                segments: self.segments(),
            }),
        }))
    }

    async fn node_get_capabilities(
        &self,
        _request: Request<NodeGetCapabilitiesRequest>,
    ) -> Result<Response<NodeGetCapabilitiesResponse>, Status> {
        let caps = vec![
            node_service_capability::rpc::Type::StageUnstageVolume,
            node_service_capability::rpc::Type::GetVolumeStats,
            node_service_capability::rpc::Type::ExpandVolume,
        ];

        debug!("NodeGetCapabilities request: {:?}", caps);

        // We don't support stage/unstage and expand volume rpcs
        Ok(Response::new(NodeGetCapabilitiesResponse {
            capabilities: caps
                .into_iter()
                .map(|c| NodeServiceCapability {
                    r#type: Some(node_service_capability::Type::Rpc(
                        node_service_capability::Rpc { r#type: c as i32 },
                    )),
                })
                .collect(),
        }))
    }

    /// This RPC is called by the CO when a workload that wants to use the
    /// specified volume is placed (scheduled) on a node. The Plugin SHALL
    /// assume that this RPC will be executed on the node where the volume will
    /// be used. If the corresponding Controller Plugin has
    /// PUBLISH_UNPUBLISH_VOLUME controller capability, the CO MUST guarantee
    /// that this RPC is called after ControllerPublishVolume is called for the
    /// given volume on the given node and returns a success. This operation
    /// MUST be idempotent. If the volume corresponding to the volume_id has
    /// already been published at the specified target_path, and is compatible
    /// with the specified volume_capability and readonly flag, the Plugin MUST
    /// reply 0 OK. If this RPC failed, or the CO does not know if it failed or
    /// not, it MAY choose to call NodePublishVolume again, or choose to call
    /// NodeUnpublishVolume. This RPC MAY be called by the CO multiple times on
    /// the same node for the same volume with possibly different target_path
    /// and/or other arguments if the volume has MULTI_NODE capability (i.e.,
    /// access_mode is either MULTI_NODE_READER_ONLY, MULTI_NODE_SINGLE_WRITER
    /// or MULTI_NODE_MULTI_WRITER).
    #[instrument(err, fields(volume.uuid = request.get_ref().volume_id), skip(self))]
    async fn node_publish_volume(
        &self,
        request: Request<NodePublishVolumeRequest>,
    ) -> Result<Response<NodePublishVolumeResponse>, Status> {
        let msg = request.into_inner();

        let req = csi_driver::trace::CsiRequest::new_info("Node Publish Volume");

        if msg.volume_id.is_empty() {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to publish volume: missing volume id"
            ));
        }
        let _guard = VolumeOpGuard::new_str(&msg.volume_id)?;

        if msg.target_path.is_empty() {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to publish volume {}: missing target path",
                &msg.volume_id
            ));
        }

        if let Err(error) = check_access_mode(&msg.volume_capability, msg.readonly) {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to publish volume {}: {}",
                &msg.volume_id,
                error
            ));
        }

        // Note that the staging path is NOT optional,
        // as we advertise StageUnstageVolume.
        if msg.staging_target_path.is_empty() {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to publish volume {}: missing staging path",
                &msg.volume_id
            ));
        }

        // The CO must ensure that the parent of target path exists,
        // make sure that it exists.
        let target_parent = Path::new(&msg.target_path).parent().unwrap();
        if !target_parent.exists() || !target_parent.is_dir() {
            return Err(Status::new(
                Code::Internal,
                format!(
                    "Failed to find parent dir for mountpoint {}, volume {}",
                    &msg.target_path, &msg.volume_id
                ),
            ));
        }

        match get_access_type(&msg.volume_capability).map_err(|error| {
            failure!(
                Code::InvalidArgument,
                "Failed to publish volume {}: {}",
                &msg.volume_id,
                error
            )
        })? {
            AccessType::Mount(mnt) => {
                publish_fs_volume(&msg, mnt, &self.filesystems).await?;
            }
            AccessType::Block(_) => {
                publish_block_volume(&msg).await?;
            }
        }

        req.info_ok();
        Ok(Response::new(NodePublishVolumeResponse {}))
    }

    /// This RPC is called by the CO when a workload using the specified
    /// volume is removed (unscheduled) from a node.
    /// If the corresponding Controller Plugin has PUBLISH_UNPUBLISH_VOLUME
    /// controller capability, the CO MUST guarantee that this RPC is called
    /// after ControllerPublishVolume is called for the given volume on the
    /// given node and returns a success.
    ///
    /// This operation MUST be idempotent.
    #[instrument(err, fields(volume.uuid = request.get_ref().volume_id), skip(self))]
    async fn node_unpublish_volume(
        &self,
        request: Request<NodeUnpublishVolumeRequest>,
    ) -> Result<Response<NodeUnpublishVolumeResponse>, Status> {
        let msg = request.into_inner();

        let req = csi_driver::trace::CsiRequest::new_info("Node Unpublish Volume");

        if msg.volume_id.is_empty() {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to unpublish volume: missing volume id"
            ));
        }
        let _guard = VolumeOpGuard::new_str(&msg.volume_id)?;

        if msg.target_path.is_empty() {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to unpublish volume {}: missing target path",
                msg.volume_id
            ));
        }

        // target path will have been created previously in node_publish_volume
        // and is one of
        //  1. a directory for filesystem volumes ,
        //  2. a block special file for block volumes.
        //
        // If it does not exist, then a previously unpublish request has
        // succeeded.
        let target_path = Path::new(&msg.target_path);
        if target_path.exists() {
            if target_path.is_dir() {
                unpublish_fs_volume(&msg).await?;
            } else {
                if target_path.is_file() {
                    return Err(Status::new(
                        Code::Unknown,
                        format!(
                            "Failed to unpublish volume {}: {} is a file.",
                            &msg.volume_id, &msg.target_path
                        ),
                    ));
                }

                unpublish_block_volume(&msg).await?;
            }
        }

        req.info_ok();
        Ok(Response::new(NodeUnpublishVolumeResponse {}))
    }

    /// Get volume stats method evaluates and returns capacity metrics.
    #[instrument(err, fields(volume.uuid = request.get_ref().volume_id), skip(self))]
    async fn node_get_volume_stats(
        &self,
        request: Request<NodeGetVolumeStatsRequest>,
    ) -> Result<Response<NodeGetVolumeStatsResponse>, Status> {
        let msg = request.into_inner();
        let req = csi_driver::trace::CsiRequest::new_trace("Node Get Volume Stats");
        if msg.volume_id.is_empty() {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to stage volume: missing volume id"
            ));
        }
        if msg.volume_path.is_empty() {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to stage volume: missing volume path"
            ));
        }
        let _guard = VolumeOpGuard::new_str(&msg.volume_id)?;

        let volume_path = Path::new(&msg.volume_path);
        if volume_path.exists() {
            // Check if it's a filesystem.
            if volume_path.is_dir() {
                match sys::statfs::statfs(&*msg.volume_path) {
                    Ok(info) => {
                        req.trace_ok();
                        Ok(Response::new(NodeGetVolumeStatsResponse {
                            usage: vec![
                                csi::VolumeUsage {
                                    total: info.blocks() as i64 * info.block_size(),
                                    unit: csi::volume_usage::Unit::Bytes as i32,
                                    available: info.blocks_available() as i64 * info.block_size(),
                                    used: (info.blocks() - info.blocks_free()) as i64
                                        * info.block_size(),
                                },
                                csi::VolumeUsage {
                                    total: info.files() as i64,
                                    unit: csi::volume_usage::Unit::Inodes as i32,
                                    available: info.files_free() as i64,
                                    used: (info.files() - info.files_free()) as i64,
                                },
                            ],
                            volume_condition: None,
                        }))
                    }
                    Err(err) => match err {
                        Errno::ENOENT => Err(Status::new(Code::NotFound, err.to_string())),
                        Errno::EIO => Err(Status::new(Code::Internal, err.to_string())),
                        Errno::ENOSYS => Err(Status::new(Code::Unavailable, err.to_string())),
                        Errno::ENOTDIR => Err(Status::new(Code::Internal, err.to_string())),
                        _ => Err(Status::new(Code::InvalidArgument, err.to_string())),
                    },
                }
            } else {
                Ok(Response::new(NodeGetVolumeStatsResponse {
                    usage: vec![],
                    volume_condition: None,
                }))
            }
        } else {
            Err(Status::new(Code::NotFound, "volume path doesn't exist"))
        }
    }

    #[instrument(err, fields(volume.uuid = request.get_ref().volume_id), skip(self))]
    async fn node_expand_volume(
        &self,
        request: Request<NodeExpandVolumeRequest>,
    ) -> Result<Response<NodeExpandVolumeResponse>, Status> {
        let args = request.into_inner();
        let req = csi_driver::trace::CsiRequest::new_info("Node Expand Volume");

        //===============================CsiAccessType=============================================
        // A type alias for better readability, and also easier conversions
        // amongst the various error types in this crate.
        type CsiAccessTypeError = String;

        // An enum to assess the state of volume when expecting it to be a filesystem type.
        enum CsiAccessType {
            SupportedFilesystem(FileSystem),
            UnsupportedFilesystem(CsiAccessTypeError),
            NotAFilesystem,
            Unknown(CsiAccessTypeError),
        }

        // From VolumeCapability translate volume access-type into one of few known states.
        impl From<&Option<VolumeCapability>> for CsiAccessType {
            fn from(volume_capability: &Option<VolumeCapability>) -> Self {
                match get_access_type(volume_capability) {
                    Ok(access_type) => match access_type {
                        AccessType::Mount(mv) => match mv.fs_type.to_lowercase().as_str() {
                            "ext4" => Self::SupportedFilesystem(FileSystem::from(Fs::Ext4)),
                            "xfs" => Self::SupportedFilesystem(FileSystem::from(Fs::Xfs)),
                            "btrfs" => Self::SupportedFilesystem(FileSystem::from(Fs::Btrfs)),
                            alien_fs => Self::UnsupportedFilesystem(format!(
                                "'{alien_fs}' is not a supported filesystem"
                            )),
                        },
                        AccessType::Block(_) => Self::NotAFilesystem,
                    },
                    Err(error) => {
                        Self::Unknown(format!("couldn't determine CSI AccessType type: {}", error))
                    }
                }
            }
        }
        //===============================CsiAccessType=============================================

        let vol_uuid = Uuid::parse_str(args.volume_id.as_str()).map_err(|error| {
            failure!(
                Code::InvalidArgument,
                "Malformed volume UUID '{}': {}",
                args.volume_id,
                error
            )
        })?;

        if args.volume_path.is_empty() {
            return Err(failure!(Code::InvalidArgument, "'volume_path' is empty"));
        }

        let required_bytes = args
            .capacity_range
            .as_ref()
            .ok_or_else(|| {
                failure!(
                    Code::InvalidArgument,
                    "Cannot expand volume '{}': invalid request {:?}: missing CapacityRange",
                    args.volume_id,
                    args
                )
            })?
            .required_bytes;

        let _guard = VolumeOpGuard::new(vol_uuid)?;

        let dev_path = Device::lookup(&vol_uuid)
            .await
            .map_err(|error| {
                failure!(
                    Code::Internal,
                    "device lookup error for volume '{}': {}",
                    vol_uuid,
                    error
                )
            })?
            .ok_or_else(|| {
                failure!(
                    Code::InvalidArgument,
                    "failed to find a device for volume {}",
                    vol_uuid
                )
            })?
            .devname();

        // Get device size.
        // The underlying block device should already have been expanded to the
        // required size as a part of the ControllerExpandVolume call.
        let device_capacity = sysfs_dev_size(dev_path.as_str()).map_err(|error| {
            failure!(
                Code::Internal,
                "failed to find the device size of device {}: {}",
                dev_path,
                error
            )
        })? as i64;

        // The NVMf volume target capacity is often less than the requested capacity. This
        // difference should be no more than 10MiB in size. We should trim the required capacity
        // down by 10MiB so that the capacity of the device is verified to be greater than or
        // equal to this corrected capacity. This is required because we are not comparing the
        // required capacity against the REST API Volume resource.
        // 10MiB includes addition leeway on top of the 5MiB required.
        const MAX_NEXUS_CAPACITY_DIFFERENCE: i64 = 10 * 1024 * 1024;
        // Ensure device_capacity is greater than or equal to required_bytes.
        if device_capacity < (required_bytes - MAX_NEXUS_CAPACITY_DIFFERENCE) {
            return Err(failure!(
                Code::FailedPrecondition,
                "block device capacity is lower than the required"
            ));
        }

        // This is what we will return for all success and no-op cases.
        // The true capacity of the device will lead to retries from the CO. Returning the
        // requested value to suppress the retries.
        let success_result = Ok(Response::new(NodeExpandVolumeResponse {
            capacity_bytes: required_bytes,
        }));

        // The CSI spec, as of v1.8.0, treats volume_capability as an optional field, so
        // in an attempt to be as spec-compliant as possible, we must have a plan-B for the
        // filesystem identification part.
        let filesystem_handle = match CsiAccessType::from(&args.volume_capability) {
            // volume_capability AccessType says this is not a 'Mount' type. Nothing to do.
            CsiAccessType::NotAFilesystem => {
                debug!(
                    "Filesystem expansion not required as volume {vol_uuid} \
                    is not a Filesystem type volume",
                );
                return success_result;
            }
            // volume_capability says that the filesystem is something we don't know about.
            CsiAccessType::UnsupportedFilesystem(fs_err) => {
                error!("Cannot expand filesystem: {}", &fs_err);
                return Err(Status::invalid_argument(fs_err));
            }
            // volume_capability has identified a supported filesystem 🎉.
            CsiAccessType::SupportedFilesystem(fs_type) => fs_type,
            // volume_capability hasn't come through for us, we're on our own.
            // Try to find the mount path at volume_path. As an extension, also validates that
            // volume_path is in fact a filesystem.
            CsiAccessType::Unknown(_) => match find_mount(None, Some(args.volume_path.as_str())) {
                // Not a filesystem.
                None => return success_result,
                // Try to generate a supported filesystem type.
                Some(mount_info) => FileSystem::try_from(&mount_info).map_err(|error| {
                    failure!(
                        Code::InvalidArgument,
                        "failed to find a supported filesystem type: {}",
                        error
                    )
                })?,
            },
        };

        // Expand the filesystem.
        filesystem_handle
            .fs_ops()
            .map_err(|err| failure!(Code::InvalidArgument, "{}", err))?
            .expand(&args.volume_path, Some(dev_path))
            .await
            .map_err(|err| failure!(Code::Internal, "{}", err))?;

        tracing::info!(size_bytes = required_bytes, "{}", req.log_str());
        success_result
    }

    #[instrument(err, fields(volume.uuid = request.get_ref().volume_id), skip(self))]
    async fn node_stage_volume(
        &self,
        request: Request<NodeStageVolumeRequest>,
    ) -> Result<Response<NodeStageVolumeResponse>, Status> {
        let msg = request.into_inner();

        let req = csi_driver::trace::CsiRequest::new_info("Node Stage Volume");

        if msg.volume_id.is_empty() {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to stage volume: missing volume id"
            ));
        }

        if msg.staging_target_path.is_empty() {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to stage volume {}: missing staging path",
                &msg.volume_id
            ));
        }

        if let Err(error) = check_access_mode(
            &msg.volume_capability,
            // relax the check a bit by pretending all stage mounts are ro
            true,
        ) {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to stage volume {}: {}",
                &msg.volume_id,
                error
            ));
        };

        let access_type = match get_access_type(&msg.volume_capability) {
            Ok(accesstype) => accesstype,
            Err(error) => {
                return Err(failure!(
                    Code::InvalidArgument,
                    "Failed to stage volume {}: {}",
                    &msg.volume_id,
                    error
                ));
            }
        };

        let uuid = Uuid::parse_str(&msg.volume_id).map_err(|error| {
            failure!(
                Code::Internal,
                "Failed to stage volume {}: not a valid UUID: {}",
                &msg.volume_id,
                error
            )
        })?;
        let _guard = VolumeOpGuard::new(uuid)?;

        // We cannot fully rely on the URI stored in the `publish_context` because it may change
        // if the target gets moved to another node.
        // It's possible that a pod gets restaged on the same node, and thus skipping the controller
        // publish call, leaving us with a bad URI:
        // https://github.com/openebs/mayastor/issues/1781
        // In order to fix this issue we need to fetch the volume uri from the control-plane.
        let uri = self.volume_uri(&uuid, &msg.publish_context).await?;

        // Note checking existence of staging_target_path, is delegated to
        // code handling those volume types where it is relevant.

        // All checks complete, now attach, if not attached already.
        debug!("Volume {} has URI {}", &msg.volume_id, uri);

        let mut device = Device::parse(&uri).map_err(|error| {
            failure!(
                Code::Internal,
                "Failed to stage volume {}: error parsing URI {}: {}",
                &msg.volume_id,
                uri,
                error
            )
        })?;
        device
            .parse_parameters(&msg.publish_context)
            .await
            .map_err(|error| {
                failure!(
                    Code::InvalidArgument,
                    "Failed to parse storage class parameters for volume {}: {}",
                    &msg.volume_id,
                    error
                )
            })?;

        let device_path = match device.find().await.map_err(|error| {
            failure!(
                Code::Internal,
                "Failed to stage volume {}: error locating device for URI {}: {}",
                &msg.volume_id,
                uri,
                error
            )
        })? {
            Some(devpath) => devpath,
            None => {
                debug!("Attaching volume {}", &msg.volume_id);
                // device.attach is idempotent, so does not restart the attach
                // process
                if let Err(error) = device.attach().await {
                    return Err(failure!(
                        Code::Internal,
                        "Failed to stage volume {}: attach failed: {}",
                        &msg.volume_id,
                        error
                    ));
                }

                let devpath =
                    Device::wait_for_device(&*device, ATTACH_TIMEOUT_INTERVAL, ATTACH_RETRIES)
                        .await
                        .map_err(|error| {
                            failure!(
                                Code::Unavailable,
                                "Failed to stage volume {}: {}",
                                &msg.volume_id,
                                error
                            )
                        })?;

                device.fixup().await.map_err(|error| {
                    failure!(
                        Code::Internal,
                        "Could not set parameters on staged device {}: {}",
                        &msg.volume_id,
                        error
                    )
                })?;

                devpath
            }
        };

        // Attach successful, now stage mount if required.
        match access_type {
            AccessType::Mount(mnt) => {
                if let Err(fsmount_error) =
                    stage_fs_volume(&msg, &device_path, mnt, &self.filesystems).await
                {
                    let mounts = crate::mount::find_src_mounts(&device_path, None);
                    // If the device is mounted elsewhere, don't detach it!
                    if mounts.is_empty() {
                        detach(
                            &uuid,
                            format!(
                                "Failed to stage volume {}: {};",
                                &msg.volume_id, fsmount_error
                            ),
                        )
                        .await?;
                    }
                    return Err(fsmount_error);
                }
            }
            AccessType::Block(_) => {
                // block volumes are not staged
            }
        }

        req.info_ok();
        Ok(Response::new(NodeStageVolumeResponse {}))
    }

    #[instrument(err, fields(volume.uuid = request.get_ref().volume_id), skip(self))]
    async fn node_unstage_volume(
        &self,
        request: Request<NodeUnstageVolumeRequest>,
    ) -> Result<Response<NodeUnstageVolumeResponse>, Status> {
        let msg = request.into_inner();

        let req = csi_driver::trace::CsiRequest::new_info("Node Unstage Volume");

        if msg.volume_id.is_empty() {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to unstage volume: missing volume id"
            ));
        }

        if msg.staging_target_path.is_empty() {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to unstage volume {}: missing staging path",
                &msg.volume_id
            ));
        }

        debug!("Unstaging volume {}", &msg.volume_id);

        let uuid = Uuid::parse_str(&msg.volume_id).map_err(|error| {
            failure!(
                Code::Internal,
                "Failed to unstage volume {}: not a valid UUID: {}",
                &msg.volume_id,
                error
            )
        })?;
        let _guard = VolumeOpGuard::new(uuid)?;

        // All checks complete, stage unmount if required.

        // unstage_fs_volume checks for mounted filesystems
        // at the staging directory and umounts if any are
        // found.
        unstage_fs_volume(&msg).await?;

        // Sometimes when disconnecting we see page read errors due to ENXIO.
        // There seems to be some race in the kernel when removing a device with queued IOs.
        // While this is not strictly an issue, it may confuse or hide other problems.
        // Sleeping between umount and disconnect seems to alleviate this.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        // unmounts (if any) are complete.
        // If the device is attached, detach the device.
        // Device::lookup will return None for nbd devices,
        // this is correct, as the attach for nbd is a no-op.
        detach(
            &uuid,
            format!("Failed to unstage volume {}:", &msg.volume_id),
        )
        .await?;

        req.info_ok();
        Ok(Response::new(NodeUnstageVolumeResponse {}))
    }
}
