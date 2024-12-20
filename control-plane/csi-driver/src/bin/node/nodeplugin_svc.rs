//! Implement services required by the node plugin
//! find volumes provisioned by us
//! freeze and unfreeze filesystem volumes provisioned by us
use crate::{
    dev::{Detach, Device},
    findmnt,
};
use csi_driver::filesystem::FileSystem as Fs;
use tracing::debug;
use uuid::Uuid;

/// Type of mount.
pub(crate) enum TypeOfMount {
    FileSystem,
    RawBlock,
}

/// Lookup the device by its volume id.
pub(crate) async fn lookup_device(volume_id: &str) -> Result<Box<dyn Detach>, tonic::Status> {
    let uuid = Uuid::parse_str(volume_id)
        .map_err(|error| tonic::Status::invalid_argument(error.to_string()))?;

    Device::lookup(&uuid)
        .await
        .map_err(|error| {
            tonic::Status::internal(format!("volume_id: {}, error: {}", volume_id, error))
        })?
        .ok_or(tonic::Status::not_found(format!(
            "volume_id: {}, not found",
            volume_id
        )))
}

/// Find the `TypeOfMount` for the given volume.
pub(crate) async fn find_mount(
    volume_id: &str,
    device: &dyn Detach,
) -> Result<Option<TypeOfMount>, tonic::Status> {
    let device_path = device.devname();
    let mountpaths = findmnt::get_mountpaths(&device_path)
        .await
        .map_err(|error| tonic::Status::internal(error.to_string()))?;
    debug!(
        volume.uuid = volume_id,
        "Mountpaths for volume: {:?}", mountpaths
    );
    if !mountpaths.is_empty() {
        let fstype = mountpaths[0].fstype();
        for devmount in mountpaths {
            if fstype != devmount.fstype() {
                debug!(
                    volume.uuid = volume_id,
                    "Find volume failed, multiple fstypes {}, {}",
                    fstype,
                    devmount.fstype()
                );
                // This failure is very unlikely but include for
                // completeness
                return Err(tonic::Status::internal(format!(
                    "Inconsistent mount filesystems: volume ID: {}",
                    volume_id
                )));
            }
        }
        debug!(volume.uuid = volume_id, ?fstype, "Found fstype for volume");
        if fstype == Fs::DevTmpFs.into() {
            Ok(Some(TypeOfMount::RawBlock))
        } else {
            Ok(Some(TypeOfMount::FileSystem))
        }
    } else {
        Ok(None)
    }
}
