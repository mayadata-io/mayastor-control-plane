use std::str::FromStr;
use strum_macros::{AsRefStr, Display, EnumString};

/// Sys mount's MountFlags representation for conversion.
#[derive(EnumString, Clone, Debug, Eq, PartialEq, Display, AsRefStr)]
#[strum(serialize_all = "lowercase")]
#[allow(clippy::upper_case_acronyms)]
pub(crate) enum MountFlag {
    BIND,
    DIRSYNC,
    MANDLOCK,
    MOVE,
    NOATIME,
    NODEV,
    NODIRATIME,
    NOEXEC,
    NOSUID,
    RDONLY,
    REC,
    RELATIME,
    REMOUNT,
    SILENT,
    STRICTATIME,
    SYNCHRONOUS,
}

struct MountFlags(Vec<MountFlag>);

/// Sys mount's UnmountFlags representation for conversion.
#[derive(EnumString, Clone, Debug, Eq, PartialEq, Display, AsRefStr)]
#[strum(serialize_all = "lowercase")]
#[allow(clippy::upper_case_acronyms)]
pub(crate) enum UnmountFlag {
    FORCE,
    DETACH,
    EXPIRE,
    NOFOLLOW,
}

struct UnmountFlags(Vec<UnmountFlag>);

impl FromStr for MountFlags {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let flags: Vec<&str> = s.split(',').collect();
        let mut mount_flags: Vec<MountFlag> = Vec::with_capacity(flags.len());

        for flag in flags {
            if !flag.trim().is_empty() {
                match flag.parse::<MountFlag>() {
                    Ok(parsed_flag) => mount_flags.push(parsed_flag),
                    Err(_) => return Err(format!("Invalid mount flag: {}", flag)),
                }
            }
        }

        Ok(MountFlags(mount_flags))
    }
}

impl FromStr for UnmountFlags {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let flags: Vec<&str> = s.split(',').collect();
        let mut unmount_flags: Vec<UnmountFlag> = Vec::with_capacity(flags.len());

        for flag in flags {
            if !flag.trim().is_empty() {
                match flag.parse::<UnmountFlag>() {
                    Ok(parsed_flag) => unmount_flags.push(parsed_flag),
                    Err(_) => return Err(format!("Invalid unmount flag: {}", flag)),
                }
            }
        }

        Ok(UnmountFlags(unmount_flags))
    }
}

/// Converts comma seperated mount flag from cli input to sys mount's Mountflags
pub(crate) fn parse_mount_flags(flags_str: &str) -> Result<sys_mount::MountFlags, String> {
    let mount_flags = MountFlags::from_str(flags_str)?;
    let mut _mount_flags = sys_mount::MountFlags::empty();
    for flag in mount_flags.0 {
        match flag {
            MountFlag::BIND => _mount_flags.insert(sys_mount::MountFlags::BIND),
            MountFlag::DIRSYNC => _mount_flags.insert(sys_mount::MountFlags::DIRSYNC),
            MountFlag::MANDLOCK => _mount_flags.insert(sys_mount::MountFlags::MANDLOCK),
            MountFlag::MOVE => _mount_flags.insert(sys_mount::MountFlags::MOVE),
            MountFlag::NOATIME => _mount_flags.insert(sys_mount::MountFlags::NOATIME),
            MountFlag::NODEV => _mount_flags.insert(sys_mount::MountFlags::NODEV),
            MountFlag::NODIRATIME => _mount_flags.insert(sys_mount::MountFlags::NODIRATIME),
            MountFlag::NOEXEC => _mount_flags.insert(sys_mount::MountFlags::NOEXEC),
            MountFlag::NOSUID => _mount_flags.insert(sys_mount::MountFlags::NOSUID),
            MountFlag::RDONLY => _mount_flags.insert(sys_mount::MountFlags::RDONLY),
            MountFlag::REC => _mount_flags.insert(sys_mount::MountFlags::REC),
            MountFlag::RELATIME => _mount_flags.insert(sys_mount::MountFlags::RELATIME),
            MountFlag::REMOUNT => _mount_flags.insert(sys_mount::MountFlags::REMOUNT),
            MountFlag::SILENT => _mount_flags.insert(sys_mount::MountFlags::SILENT),
            MountFlag::STRICTATIME => _mount_flags.insert(sys_mount::MountFlags::STRICTATIME),
            MountFlag::SYNCHRONOUS => _mount_flags.insert(sys_mount::MountFlags::SYNCHRONOUS),
        }
    }
    Ok(_mount_flags)
}

/// Converts comma seperated mount flag from cli input to sys mount's Unmountflags
pub(crate) fn parse_unmount_flags(flags_str: &str) -> Result<sys_mount::UnmountFlags, String> {
    let unmount_flags = UnmountFlags::from_str(flags_str)?;
    let mut _unmount_flags = sys_mount::UnmountFlags::empty();
    for flag in unmount_flags.0 {
        match flag {
            UnmountFlag::FORCE => _unmount_flags.insert(sys_mount::UnmountFlags::FORCE),
            UnmountFlag::DETACH => _unmount_flags.insert(sys_mount::UnmountFlags::DETACH),
            UnmountFlag::EXPIRE => _unmount_flags.insert(sys_mount::UnmountFlags::EXPIRE),
            UnmountFlag::NOFOLLOW => _unmount_flags.insert(sys_mount::UnmountFlags::NOFOLLOW),
        }
    }
    Ok(_unmount_flags)
}
