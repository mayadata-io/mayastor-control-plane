//! Utility functions for mounting and unmounting filesystems.
use crate::{filesystem_ops::FileSystem, findmnt::DeviceMount, runtime};
use csi_driver::filesystem::FileSystem as Fs;
use devinfo::mountinfo::{MountInfo, SafeMountIter};

use std::{collections::HashSet, io::Error};
use sys_mount::{unmount, FilesystemType, Mount, MountFlags, UnmountFlags};
use tonic::Status;
use tracing::{debug, info};
use uuid::Uuid;

// Simple trait for checking if the readonly (ro) option
// is present in a "list" of options, while allowing for
// flexibility as to the type of "list".
pub(super) trait ReadOnly {
    fn readonly(&self) -> bool;
}

impl ReadOnly for Vec<String> {
    fn readonly(&self) -> bool {
        self.iter().any(|entry| entry == "ro")
    }
}

impl ReadOnly for &str {
    fn readonly(&self) -> bool {
        self.split(',').any(|entry| entry == "ro")
    }
}

/// Return mountinfo matching source and/or destination.
pub(crate) fn find_mount(source: Option<&str>, target: Option<&str>) -> Option<MountInfo> {
    let mut found: Option<MountInfo> = None;

    for mount in SafeMountIter::get().unwrap().flatten() {
        if let Some(value) = source {
            if mount.source.to_string_lossy() == value {
                if let Some(value) = target {
                    if mount.dest.to_string_lossy() == value {
                        found = Some(mount);
                    }
                    continue;
                }
                found = Some(mount);
            }
            continue;
        }
        if let Some(value) = target {
            if mount.dest.to_string_lossy() == value {
                found = Some(mount);
            }
        }
    }

    found.map(MountInfo::from)
}

/// Return all mounts for a matching source.
/// Optionally ignore the given destination path.
pub(crate) fn find_src_mounts(source: &str, dest_ignore: Option<&str>) -> Vec<MountInfo> {
    SafeMountIter::get()
        .unwrap()
        .flatten()
        .filter(|mount| {
            mount.source.to_string_lossy() == source
                && match dest_ignore {
                    None => true,
                    Some(ignore) => ignore != mount.dest.to_string_lossy(),
                }
        })
        .collect()
}

/// Check if options in "first" are also present in "second",
/// but exclude values "ro" and "rw" from the comparison.
pub(super) fn subset(first: &[String], second: &[String]) -> bool {
    let set: HashSet<&String> = second.iter().collect();
    for entry in first {
        if entry == "ro" {
            continue;
        }
        if entry == "rw" {
            continue;
        }
        if set.get(entry).is_none() {
            return false;
        }
    }
    true
}

/// Return supported filesystems.
pub(crate) fn probe_filesystems() -> Vec<FileSystem> {
    vec![Fs::Xfs.into(), Fs::Ext4.into(), Fs::Btrfs.into()]
}

// Utility function to transform a vector of options
// to the format required by sys_mount::Mount::new()
fn parse(options: &[String]) -> (bool, String) {
    let mut list: Vec<&str> = Vec::new();
    let mut readonly: bool = false;

    for entry in options {
        if entry == "ro" {
            readonly = true;
            continue;
        }

        if entry == "rw" {
            continue;
        }

        list.push(entry);
    }

    (readonly, list.join(","))
}

// Utility function used for displaying a list of options.
fn show(options: &[String]) -> String {
    let list: Vec<String> = options
        .iter()
        .filter(|value| value.as_str() != "rw")
        .cloned()
        .collect();

    if list.is_empty() {
        return String::from("none");
    }

    list.join(",")
}

pub(crate) async fn filesystem_mount(
    device: &str,
    target: &str,
    fstype: &FileSystem,
    options: &[String],
) -> Result<Mount, Error> {
    let device = device.to_string();
    let target = target.to_string();
    let fstype = fstype.clone();
    let options: Vec<String> = options.to_vec();

    runtime::spawn_blocking(move || {
        let mut flags = MountFlags::empty();

        let (readonly, value) = parse(&options);

        if readonly {
            flags.insert(MountFlags::RDONLY);
        }

        // I'm not certain if it's fine to pass "" so keep existing behaviour
        let mount_builder = if value.is_empty() {
            Mount::builder()
        } else {
            Mount::builder().data(&value)
        }
        .fstype(FilesystemType::Manual(fstype.as_ref()))
        .flags(flags);

        let mount = mount_builder.mount(&device, &target)?;

        debug!(
            "Filesystem ({}) on device {} mounted onto target {} (options: {})",
            fstype,
            device,
            target,
            show(&options)
        );

        Ok(mount)
    })
    .await?
}

/// Unmount a device from a directory (mountpoint).
/// Should not be used for removing bind mounts.
pub(crate) async fn filesystem_unmount(target: &str) -> Result<(), Error> {
    let target = target.to_string();

    runtime::spawn_blocking(move || {
        let flags = UnmountFlags::empty();

        unmount(&target, flags)?;

        debug!("Target {} unmounted", target);

        Ok(())
    })
    .await?
}

/// Bind mount a source path to a target path.
/// Supports both directories and files.
pub(crate) async fn bind_mount(source: &str, target: &str, file: bool) -> Result<Mount, Error> {
    let source = source.to_string();
    let target = target.to_string();

    runtime::spawn_blocking(move || {
        let mut flags = MountFlags::empty();
        flags.insert(MountFlags::BIND);

        if file {
            flags.insert(MountFlags::RDONLY);
        }

        let mount = Mount::builder()
            .fstype(FilesystemType::Manual("none"))
            .flags(flags)
            .mount(&source, &target)?;

        debug!("Source {} bind mounted onto target {}", source, target);

        Ok(mount)
    })
    .await?
}

/// Bind remount a path to modify mount options.
/// Assumes that target has already been bind mounted.
pub(crate) async fn bind_remount(target: &str, options: &[String]) -> Result<Mount, Error> {
    let target = target.to_string();
    let options = options.to_vec();

    runtime::spawn_blocking(move || {
        let mut flags = MountFlags::empty();
        let (readonly, value) = parse(&options);

        flags.insert(MountFlags::BIND);

        if readonly {
            flags.insert(MountFlags::RDONLY);
        }

        flags.insert(MountFlags::REMOUNT);

        let mount = if value.is_empty() {
            Mount::builder()
        } else {
            Mount::builder().data(&value)
        }
        .fstype(FilesystemType::Manual("none"))
        .flags(flags)
        .mount("none", &target)?;

        debug!(
            "Target {} bind remounted (options: {})",
            target,
            show(&options)
        );

        Ok(mount)
    })
    .await?
}

/// Unmounts a path that has previously been bind mounted.
/// Should not be used for unmounting devices.
pub(crate) async fn bind_unmount(target: &str) -> Result<(), Error> {
    let target = target.to_string();

    runtime::spawn_blocking(move || {
        let flags = UnmountFlags::empty();
        unmount(&target, flags)?;

        debug!("Target {} bind unmounted", target);
        Ok(())
    })
    .await?
}

/// Remount existing mount as read only or read write.
pub(crate) async fn remount(target: &str, ro: bool) -> Result<Mount, Error> {
    let target = target.to_string();

    runtime::spawn_blocking(move || {
        let mut flags = MountFlags::empty();
        flags.insert(MountFlags::REMOUNT);

        if ro {
            flags.insert(MountFlags::RDONLY);
        }

        let mount = Mount::builder()
            .fstype(FilesystemType::Manual("none"))
            .flags(flags)
            .mount("", &target)?;

        debug!("Target {} remounted with {}", target, flags.bits());

        Ok(mount)
    })
    .await?
}

/// Mount a block device
pub(crate) async fn blockdevice_mount(
    source: &str,
    target: &str,
    readonly: bool,
) -> Result<Mount, Error> {
    let source = source.to_string();
    let target = target.to_string();

    runtime::spawn_blocking(move || {
        debug!("Mounting {} ...", source);

        let mut flags = MountFlags::empty();
        flags.insert(MountFlags::BIND);

        let mount = Mount::builder()
            .fstype(FilesystemType::Manual("none"))
            .flags(flags)
            .mount(&source, &target)?;

        info!("Block device {} mounted to {}", source, target);

        if readonly {
            flags.insert(MountFlags::REMOUNT);
            flags.insert(MountFlags::RDONLY);

            let mount = Mount::builder()
                .fstype(FilesystemType::Manual(""))
                .flags(flags)
                .mount("", &target)?;

            info!("Remounted block device {} (readonly) to {}", source, target);
            return Ok(mount);
        }

        Ok(mount)
    })
    .await?
}

/// Unmount a block device.
pub(crate) async fn blockdevice_unmount(target: &str) -> Result<(), Error> {
    let target = target.to_string();

    runtime::spawn_blocking(move || {
        let flags = UnmountFlags::empty();

        debug!(
            "Unmounting block device {} (flags={}) ...",
            target,
            flags.bits()
        );

        unmount(&target, flags)?;
        info!("Block device at {} has been unmounted", target);

        Ok(())
    })
    .await?
}

/// Waits until a device's filesystem is shutdown.
/// This is useful to know if it's safe to detach a device from a node or not as it seems that
/// even after a umount completes the filesystem and more specifically the filesystem's journal
/// might not be completely shutdown.
/// Specifically, this waits for the filesystem (eg: ext4) shutdown and the filesystem's journal
/// shutdown: jbd2.
pub(crate) async fn wait_fs_shutdown(device: &str, fstype: Option<String>) -> Result<(), Error> {
    let device_trim = device.replace("/dev/", "");

    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(2);

    if let Some(fstype) = fstype {
        let proc_fs_str = format!("/proc/fs/{fstype}/{device_trim}");
        let proc_fs = std::path::Path::new(&proc_fs_str);
        wait_file_removal(proc_fs, start, timeout).await?;
    }

    let jbd2_pattern = format!("/proc/fs/jbd2/{device_trim}-*");
    let proc_jbd2 = glob::glob(&jbd2_pattern)
        .expect("valid pattern")
        .next()
        .and_then(|v| v.ok());
    if let Some(proc_jbd2) = proc_jbd2 {
        wait_file_removal(&proc_jbd2, start, timeout).await?;
    }

    Ok(())
}

/// Waits until a file is removed, up to a timeout.
async fn wait_file_removal(
    proc: &std::path::Path,
    start: std::time::Instant,
    timeout: std::time::Duration,
) -> Result<(), Error> {
    let check_interval = std::time::Duration::from_millis(200);
    let proc_str = proc.to_string_lossy().to_string();
    let mut exists = proc.exists();
    while start.elapsed() < timeout && exists {
        tracing::error!(proc = proc_str, "proc entry still exists");
        tokio::time::sleep(check_interval).await;
        exists = proc.exists();
    }
    match exists {
        false => Ok(()),
        true => Err(Error::new(
            std::io::ErrorKind::TimedOut,
            format!("Timed out waiting for '{proc_str}' to be removed"),
        )),
    }
}

/// If the filesystem uuid doesn't match with the provided uuid, unmount the device.
pub(crate) async fn unmount_on_fs_id_diff(
    device_path: &str,
    fs_staging_path: &str,
    volume_uuid: &Uuid,
) -> Result<(), String> {
    if let Ok(probed_uuid) = FileSystem::property(device_path, "UUID") {
        if probed_uuid == volume_uuid.to_string() {
            return Ok(());
        }
    }
    filesystem_unmount(fs_staging_path).await.map_err(|error| {
        format!(
            "Failed to unmount on fs id difference, device {device_path} from {fs_staging_path} for {volume_uuid}, {error}",
        )
    })
}

pub(crate) async fn lazy_unmount_mountpaths(mountpaths: &Vec<DeviceMount>) -> Result<(), Status> {
    for mountpath in mountpaths {
        debug!(
            "Unmounting path: {}, with DETACH flag",
            mountpath.mount_path()
        );
        let target_path = mountpath.mount_path().to_string();

        runtime::spawn_blocking({
            let target_path = target_path.clone();
            move || {
                let mut unmount_flags = UnmountFlags::empty();
                unmount_flags.insert(UnmountFlags::DETACH);
                sys_mount::unmount(target_path, unmount_flags)
                    .map_err(|error| Status::aborted(error.to_string()))
            }
        })
        .await
        .map_err(|error| Status::aborted(error.to_string()))??;
    }
    Ok(())
}
