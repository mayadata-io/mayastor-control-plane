use crate::{
    dev::Device,
    error::FsfreezeError,
    findmnt,
    fsfreeze::bin::ioctl::{fsfreeze_ioctl, fsfreeze_preflight_check},
    mount,
};
use strum_macros::{AsRefStr, Display, EnumString};
use uuid::Uuid;

pub(crate) mod ioctl;

#[derive(EnumString, Clone, Debug, Eq, PartialEq, Display, AsRefStr)]
#[strum(serialize_all = "lowercase")]
pub enum FsFreezeOpt {
    #[strum(serialize = "fs-freeze")]
    Freeze,
    #[strum(serialize = "fs-unfreeze")]
    Unfreeze,
}

/// Issue FIFREEZE and FITHAW ioctl on the mount_path derived from the volume_id based on the
/// command.
pub(crate) async fn fsfreeze(volume_id: &str, command: FsFreezeOpt) -> Result<(), FsfreezeError> {
    let uuid = Uuid::parse_str(volume_id).map_err(|error| FsfreezeError::InvalidVolumeId {
        source: error,
        volume_id: volume_id.to_string(),
    })?;

    let Some(device) =
        Device::lookup(&uuid)
            .await
            .map_err(|error| FsfreezeError::InternalFailure {
                source: error,
                volume_id: volume_id.to_string(),
            })?
    else {
        return Err(FsfreezeError::VolumeNotFound {
            volume_id: volume_id.to_string(),
        });
    };

    let device_path = device.devname();
    if let Some(mnt) = mount::find_mount(Some(&device_path), None) {
        // Make a preflight check, to ensure subsystem has at least one live path.
        let device_nqn = device.devnqn();
        fsfreeze_preflight_check(volume_id, device_nqn).map_err(|errno| {
            FsfreezeError::FsfreezeFailed {
                volume_id: volume_id.to_string(),
                errno,
            }
        })?;

        // Execute the fsfreeze call.
        return fsfreeze_ioctl(command, mnt.dest).await.map_err(|errno| {
            FsfreezeError::FsfreezeFailed {
                volume_id: volume_id.to_string(),
                errno,
            }
        });
    } else {
        // mount::find_mount does not return any matches,
        // for mounts which are bind mounts to block devices
        // (raw block volume).
        // It would be incorrect to return the VolumeNotFound error,
        // if the volume is mounted as a raw block volume on this node.
        // Use findmnt to work out if volume is mounted as a raw
        // block, i.e. we get some matches, and return the
        // BlockDeviceMount error.
        let mountpaths = findmnt::get_mountpaths(&device_path)
            .await
            .map_err(|error| FsfreezeError::InternalFailure {
                source: error,
                volume_id: volume_id.to_string(),
            })?;
        if !mountpaths.is_empty() {
            return Err(FsfreezeError::BlockDeviceMount {
                volume_id: volume_id.to_string(),
            });
        }
    }
    Err(FsfreezeError::VolumeNotMounted {
        volume_id: volume_id.to_string(),
    })
}
