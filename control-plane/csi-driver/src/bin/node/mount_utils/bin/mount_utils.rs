use crate::error::MountError;

use sys_mount::{FilesystemType, Mount, MountFlags, UnmountFlags};

/// Calls sys mount's mount in a spawn blocking.
pub(crate) async fn mount(
    device_path: String,
    target: String,
    data: Option<String>,
    mount_flags: MountFlags,
    fstype: Option<String>,
) -> Result<(), MountError> {
    let blocking_task = tokio::task::spawn_blocking(move || {
        let mut mount = Mount::builder();

        if let Some(data_str) = data.as_deref().filter(|d| !d.is_empty()) {
            mount = mount.data(data_str);
        }

        mount
            .flags(mount_flags)
            .fstype(FilesystemType::Manual(fstype.unwrap_or_default().as_ref()))
            .mount(device_path, target)
            .map_err(|error| MountError::MountFailed { error })
    });

    match blocking_task.await {
        Ok(result) => match result {
            Ok(_mount) => Ok(()),
            Err(error) => Err(error),
        },
        Err(error) => Err(MountError::FailedWaitForThread { join_error: error }),
    }
}

/// Calls sys mount's unmount in a spawn blocking.
pub(crate) async fn unmount(target: String, unmount_flags: UnmountFlags) -> Result<(), MountError> {
    let blocking_task = tokio::task::spawn_blocking(move || {
        sys_mount::unmount(target, unmount_flags)
            .map_err(|error| MountError::UnmountFailed { error })
    });

    match blocking_task.await {
        Ok(result) => match result {
            Ok(_) => Ok(()),
            Err(error) => Err(error),
        },
        Err(error) => Err(MountError::FailedWaitForThread { join_error: error }),
    }
}
