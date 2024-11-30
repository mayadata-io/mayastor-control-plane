//! Functions for CSI stage, unstage, publish and unpublish filesystem volumes.
use crate::{
    filesystem_ops::FileSystem,
    format::prepare_device,
    mount::{self, subset, ReadOnly},
};
use csi_driver::{
    csi::{
        volume_capability::MountVolume, NodePublishVolumeRequest, NodeStageVolumeRequest,
        NodeUnpublishVolumeRequest, NodeUnstageVolumeRequest,
    },
    filesystem::FileSystem as Fs,
    PublishParams,
};

use std::{fs, io::ErrorKind, path::PathBuf};
use tonic::{Code, Status};
use tracing::{debug, error, info};
use uuid::Uuid;

macro_rules! failure {
    (Code::$code:ident, $msg:literal) => {{ error!($msg); Status::new(Code::$code, $msg) }};
    (Code::$code:ident, $fmt:literal $(,$args:expr)+) => {{ let message = format!($fmt $(,$args)+); error!("{}", message); Status::new(Code::$code, message) }};
}

pub(crate) async fn stage_fs_volume(
    msg: &NodeStageVolumeRequest,
    device_path: &str,
    mnt: &MountVolume,
    filesystems: &[FileSystem],
) -> Result<(), Status> {
    let volume_uuid = Uuid::parse_str(&msg.volume_id).map_err(|error| {
        failure!(
            Code::InvalidArgument,
            "Failed to stage volume {}: not a valid UUID: {}",
            &msg.volume_id,
            error
        )
    })?;

    // Extract the fs_id from the context, will only be set if requested and its a clone/restore.
    let params = PublishParams::try_from(&msg.publish_context)?;
    let fs_id = params.fs_id();

    let fs_staging_path = &msg.staging_target_path;

    // One final check for fs volumes, ignore for block volumes.
    if let Err(err) = fs::create_dir_all(PathBuf::from(&fs_staging_path)) {
        if err.kind() != ErrorKind::AlreadyExists {
            return Err(Status::new(
                Code::Internal,
                format!(
                    "Failed to create mountpoint {} for volume {}: {}",
                    &fs_staging_path, volume_uuid, err
                ),
            ));
        }
    }

    debug!("Staging volume {} to {}", volume_uuid, fs_staging_path);

    let fstype = if mnt.fs_type.is_empty() {
        &filesystems[0]
    } else {
        match filesystems
            .iter()
            .find(|entry| entry.to_string() == mnt.fs_type)
        {
            Some(fstype) => fstype,
            None => {
                return Err(failure!(
                    Code::InvalidArgument,
                    "Failed to stage volume {}: unsupported filesystem type: {}",
                    volume_uuid,
                    mnt.fs_type
                ));
            }
        }
    };

    if let Some(existing) = mount::find_mount(Some(device_path), Some(fs_staging_path)) {
        debug!(
            "Device {} is already mounted onto {}",
            device_path, fs_staging_path
        );
        info!(
            %existing,
            "Volume {} is already staged to {}", volume_uuid, fs_staging_path
        );

        // If clone's fs id change was requested and we were not able to change it in first attempt
        // unmount and continue the stage again.
        let continue_stage = if fs_id.is_some() {
            continue_after_unmount_on_fs_id_diff(fstype ,device_path, fs_staging_path, &volume_uuid).await
                .map_err(|error| {
                    failure!(
                    Code::FailedPrecondition,
                    "Failed to stage volume {}: staging path unmount on fs id difference failed: {}",
                    volume_uuid,
                    error
                )
                })?
        } else {
            false
        };

        if !continue_stage {
            // todo: validate other flags?
            if mnt.mount_flags.readonly() != existing.options.readonly() {
                mount::remount(fs_staging_path, mnt.mount_flags.readonly()).await?;
            }

            return Ok(());
        }
    }

    // abort if device is mounted somewhere else
    if mount::find_mount(Some(device_path), None).is_some() {
        return Err(failure!(
            Code::AlreadyExists,
            "Failed to stage volume {}: device {} is already mounted elsewhere",
            volume_uuid,
            device_path
        ));
    }

    // abort if some another device is mounted on staging_path
    if mount::find_mount(None, Some(fs_staging_path)).is_some() {
        return Err(failure!(
            Code::AlreadyExists,
            "Failed to stage volume {}: another device is already mounted onto {}",
            volume_uuid,
            fs_staging_path
        ));
    }

    let mount_flags = fstype
        .fs_ops()
        .map_err(|error| {
            failure!(
                Code::Internal,
                "Failed to stage volume {}: could not get mount flags for {}, {}",
                volume_uuid,
                fstype,
                error
            )
        })?
        .mount_flags(mnt.mount_flags.clone());

    if let Err(error) =
        prepare_device(fstype, device_path, fs_staging_path, &mount_flags, fs_id).await
    {
        return Err(failure!(
            Code::Internal,
            "Failed to stage volume {}: error preparing device {}: {}",
            volume_uuid,
            device_path,
            error
        ));
    }

    debug!("Mounting device {} onto {}", device_path, fs_staging_path);

    if let Err(error) =
        mount::filesystem_mount(device_path, fs_staging_path, fstype, &mount_flags).await
    {
        return Err(failure!(
            Code::Internal,
            "Failed to stage volume {}: failed to mount device {} onto {}: {}",
            volume_uuid,
            device_path,
            fs_staging_path,
            error
        ));
    }

    info!("Volume {} staged to {}", volume_uuid, fs_staging_path);

    Ok(())
}

/// Unstage a filesystem volume
pub(crate) async fn unstage_fs_volume(msg: &NodeUnstageVolumeRequest) -> Result<(), Status> {
    let volume_id = &msg.volume_id;
    let fs_staging_path = &msg.staging_target_path;

    if let Some(mount) = mount::find_mount(None, Some(fs_staging_path)) {
        debug!(
            "Unstaging filesystem volume {}, unmounting device {:?} from {}",
            volume_id, mount.source, fs_staging_path
        );
        let device = mount.source.to_string_lossy().to_string();
        let mounts = mount::find_src_mounts(&device, Some(fs_staging_path));
        if let Some(unkown_mount) = mounts.first().cloned() {
            for mount in mounts {
                tracing::error!(
                    volume.uuid = %volume_id,
                    "Found unknown bind mount {} for device {:?}",
                    device,
                    mount.dest,
                );
            }

            return Err(failure!(
                Code::Internal,
                "Failed to unstage volume {}: existing unknown bind mount {:?} for device {:?}",
                volume_id,
                unkown_mount.dest,
                unkown_mount.source
            ));
        }

        if let Err(error) = mount::filesystem_unmount(fs_staging_path).await {
            return Err(failure!(
                Code::Internal,
                "Failed to unstage volume {}: failed to unmount device {:?} from {}: {}",
                volume_id,
                mount.source,
                fs_staging_path,
                error
            ));
        }

        mount::wait_fs_shutdown(&device, Some(mount.fstype)).await?;
    }

    Ok(())
}

/// Publish a filesystem volume
pub(crate) async fn publish_fs_volume(
    msg: &NodePublishVolumeRequest,
    mnt: &MountVolume,
    filesystems: &[FileSystem],
) -> Result<(), Status> {
    let target_path = &msg.target_path;
    let volume_id = &msg.volume_id;
    let fs_staging_path = &msg.staging_target_path;

    debug!(
        "Publishing volume {} from {} to {}",
        volume_id, fs_staging_path, target_path
    );

    let staged = mount::find_mount(None, Some(fs_staging_path)).ok_or_else(|| {
        failure!(
            Code::InvalidArgument,
            "Failed to publish volume {}: no mount for staging path {}",
            volume_id,
            fs_staging_path
        )
    })?;

    // TODO: Should also check that the staged "device"
    // corresponds to the the volume uuid

    if !mnt.fs_type.is_empty() && mnt.fs_type != staged.fstype {
        return Err(failure!(
            Code::InvalidArgument,
            "Failed to publish volume {}: filesystem type ({}) does not match staged volume ({})",
            volume_id,
            mnt.fs_type,
            staged.fstype
        ));
    }

    if !filesystems
        .iter()
        .any(|entry| entry.as_ref() == staged.fstype)
    {
        return Err(failure!(
            Code::InvalidArgument,
            "Failed to publish volume {}: unsupported filesystem type: {}",
            volume_id,
            staged.fstype
        ));
    }

    let readonly = staged.options.readonly();

    if readonly && !msg.readonly {
        return Err(failure!(
            Code::InvalidArgument,
            "Failed to publish volume {}: volume is staged as \"ro\" but publish requires \"rw\"",
            volume_id
        ));
    }

    if let Some(mount) = mount::find_mount(None, Some(target_path)) {
        if mount.source != staged.source {
            return Err(failure!(
                Code::AlreadyExists,
                "Failed to publish volume {}: directory {} is already in use",
                volume_id,
                target_path
            ));
        }

        if !subset(&mnt.mount_flags, &mount.options) || msg.readonly != mount.options.readonly() {
            return Err(failure!(
                    Code::AlreadyExists,
                    "Failed to publish volume {}: directory {} is already mounted but with incompatible flags",
                    volume_id,
                    target_path
                ));
        }

        info!(
            "Volume {} is already published to {}",
            volume_id, target_path
        );

        return Ok(());
    }

    debug!("Creating directory {}", target_path);

    if let Err(error) = fs::create_dir_all(PathBuf::from(target_path)) {
        if error.kind() != ErrorKind::AlreadyExists {
            return Err(failure!(
                Code::Internal,
                "Failed to publish volume {}: failed to create directory {}: {}",
                volume_id,
                target_path,
                error
            ));
        }
    }

    debug!("Mounting {} to {}", fs_staging_path, target_path);

    if let Err(error) = mount::bind_mount(fs_staging_path, target_path, false).await {
        return Err(failure!(
            Code::Internal,
            "Failed to publish volume {}: failed to mount {} to {}: {}",
            volume_id,
            fs_staging_path,
            target_path,
            error
        ));
    }

    if msg.readonly && !readonly {
        let mut options = mnt.mount_flags.clone();
        options.push(String::from("ro"));

        debug!("Remounting {} as readonly", target_path);

        if let Err(error) = mount::bind_remount(target_path, &options).await {
            let message = format!(
                "Failed to publish volume {volume_id}: failed to mount {fs_staging_path} to {target_path} as readonly: {error}"
            );

            error!("Failed to remount {}: {}", target_path, error);

            debug!("Unmounting {}", target_path);

            if let Err(error) = mount::bind_unmount(target_path).await {
                error!("Failed to unmount {}: {}", target_path, error);
            }

            return Err(Status::new(Code::Internal, message));
        }
    }

    info!("Volume {} published to {}", volume_id, target_path);

    Ok(())
}

pub(crate) async fn unpublish_fs_volume(msg: &NodeUnpublishVolumeRequest) -> Result<(), Status> {
    // filesystem mount
    let target_path = &msg.target_path;
    let volume_id = &msg.volume_id;

    if mount::find_mount(None, Some(target_path)).is_none() {
        // No mount found for target_path.
        // The idempotency requirement means this is not an error.
        // Just clean up as best we can
        if let Err(error) = fs::remove_dir(PathBuf::from(target_path)) {
            if error.kind() != ErrorKind::NotFound {
                // Return error so that kubelet can retry
                return Err(failure!(
                    Code::Internal,
                    "Failed to remove directory {}: {}",
                    target_path,
                    error
                ));
            }
        }

        info!(
            "Volume {} is already unpublished from {}",
            volume_id, target_path
        );

        return Ok(());
    }

    debug!("Unmounting {}", target_path);

    if let Err(error) = mount::bind_unmount(target_path).await {
        return Err(failure!(
            Code::Internal,
            "Failed to unpublish volume {}: failed to unmount {}: {}",
            volume_id,
            target_path,
            error
        ));
    }

    debug!("Removing directory {}", target_path);

    if let Err(error) = fs::remove_dir(PathBuf::from(target_path)) {
        if error.kind() != ErrorKind::NotFound {
            return Err(failure!(
                Code::Internal,
                "Failed to remove directory {}: {}",
                target_path,
                error
            ));
        }
    }

    info!("Volume {} unpublished from {}", volume_id, target_path);
    Ok(())
}

/// Check if we can continue the staging incase the change fs id failed mid way and we want to retry
/// the flow.
async fn continue_after_unmount_on_fs_id_diff(
    fstype: &FileSystem,
    device_path: &str,
    fs_staging_path: &str,
    volume_uuid: &Uuid,
) -> Result<bool, String> {
    fstype
        .fs_ops()?
        .unmount_on_fs_id_diff(device_path, fs_staging_path, volume_uuid)
        .await?;
    Ok(fstype == &Fs::Xfs.into())
}
