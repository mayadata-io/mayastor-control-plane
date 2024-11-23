//! Utility functions for mounting and unmounting filesystems.
use crate::{
    filesystem_ops::FileSystem,
    mount_utils::{bin::flag::MountFlag, MayastorMount},
};
use csi_driver::filesystem::FileSystem as Fs;
use devinfo::mountinfo::{MountInfo, SafeMountIter};

use std::{collections::HashSet, io::Error};
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
        if !set.contains(entry) {
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

/// Mount a device to a directory (mountpoint)
pub(crate) async fn filesystem_mount(
    device: &str,
    target: &str,
    fstype: &FileSystem,
    options: &[String],
) -> Result<MayastorMount, Status> {
    let mut flags = Vec::new();

    let (readonly, value) = parse(options);

    if readonly {
        flags.push(MountFlag::RDONLY.to_string());
    }

    // I'm not certain if it's fine to pass "" so keep existing behaviour
    let mount = if value.is_empty() {
        MayastorMount::builder()
    } else {
        MayastorMount::builder().data(&value)
    }
    .fstype(fstype.as_ref())
    .flags(flags.join(",").as_str())
    .source(device)
    .target(target)
    .mount()
    .await?;

    debug!(
        "Filesystem ({}) on device {} mounted onto target {} (options: {})",
        fstype,
        device,
        target,
        show(options)
    );

    Ok(mount)
}

/// Unmount a device from a directory (mountpoint)
/// Should not be used for removing bind mounts.
pub(crate) async fn filesystem_unmount(target: &str) -> Result<(), Status> {
    let flags: Vec<String> = Vec::new();

    MayastorMount::builder()
        .flags(flags.join(",").as_str())
        .target(target)
        .unmount()
        .await?;

    debug!("Target {} unmounted", target);

    Ok(())
}

/// Bind mount a source path to a target path.
/// Supports both directories and files.
pub(crate) async fn bind_mount(
    source: &str,
    target: &str,
    file: bool,
) -> Result<MayastorMount, Status> {
    let mut flags: Vec<String> = Vec::new();

    flags.push(MountFlag::BIND.to_string());

    if file {
        flags.push(MountFlag::RDONLY.to_string());
    }

    let mount = MayastorMount::builder()
        .fstype("none")
        .flags(flags.join(",").as_str())
        .source(source)
        .target(target)
        .mount()
        .await?;

    debug!("Source {} bind mounted onto target {}", source, target);

    Ok(mount)
}

/// Bind remount a path to modify mount options.
/// Assumes that target has already been bind mounted.
pub(crate) async fn bind_remount(
    target: &str,
    options: &[String],
) -> Result<MayastorMount, Status> {
    let mut flags: Vec<String> = Vec::new();

    let (readonly, value) = parse(options);

    flags.push(MountFlag::BIND.to_string());

    if readonly {
        flags.push(MountFlag::RDONLY.to_string());
    }

    flags.push(MountFlag::REMOUNT.to_string());

    let mount = if value.is_empty() {
        MayastorMount::builder()
    } else {
        MayastorMount::builder().data(&value)
    }
    .fstype("none")
    .flags(flags.join(",").as_str())
    .source("none")
    .target(target)
    .mount()
    .await?;

    debug!(
        "Target {} bind remounted (options: {})",
        target,
        show(options)
    );

    Ok(mount)
}

/// Unmounts a path that has previously been bind mounted.
/// Should not be used for unmounting devices.
pub(crate) async fn bind_unmount(target: &str) -> Result<(), Status> {
    let flags: Vec<String> = Vec::new();

    MayastorMount::builder()
        .flags(flags.join(",").as_str())
        .target(target)
        .unmount()
        .await?;

    debug!("Target {} bind unmounted", target);

    Ok(())
}

/// Remount existing mount as read only or read write.
pub(crate) async fn remount(target: &str, ro: bool) -> Result<MayastorMount, Status> {
    let mut flags: Vec<String> = Vec::new();
    flags.push(MountFlag::REMOUNT.to_string());

    if ro {
        flags.push(MountFlag::RDONLY.to_string());
    }

    let mount = MayastorMount::builder()
        .fstype("none")
        .flags(flags.join(",").as_str())
        .source("")
        .target(target)
        .mount()
        .await?;

    debug!("Target {} remounted with {}", target, flags.join(","));

    Ok(mount)
}

/// Mount a block device
pub(crate) async fn blockdevice_mount(
    source: &str,
    target: &str,
    readonly: bool,
) -> Result<MayastorMount, Status> {
    debug!("Mounting {} ...", source);

    let mut flags: Vec<String> = Vec::new();
    flags.push(MountFlag::BIND.to_string());

    let mount = MayastorMount::builder()
        .fstype("none")
        .flags(flags.join(",").as_str())
        .source(source)
        .target(target)
        .mount()
        .await?;

    info!("Block device {} mounted to {}", source, target);

    if readonly {
        flags.push(MountFlag::REMOUNT.to_string());
        flags.push(MountFlag::RDONLY.to_string());

        let mount = MayastorMount::builder()
            .fstype("")
            .flags(flags.join(",").as_str())
            .source("")
            .target(target)
            .mount()
            .await?;

        info!("Remounted block device {} (readonly) to {}", source, target);
        return Ok(mount);
    }

    Ok(mount)
}

/// Unmount a block device.
pub(crate) async fn blockdevice_unmount(target: &str) -> Result<(), Status> {
    let flags: Vec<String> = Vec::new();

    debug!(
        "Unmounting block device {} (flags={}) ...",
        target,
        flags.join(",")
    );

    MayastorMount::builder()
        .flags(flags.join(",").as_str())
        .target(target)
        .unmount()
        .await?;
    info!("block device at {} has been unmounted", target);
    Ok(())
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
