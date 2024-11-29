use nix::{
    sys::wait::waitpid,
    unistd::{fork, setsid, ForkResult},
};
use sys_mount::{MountBuilder, UnmountFlags};
use tokio::task::spawn_blocking;
use tonic::Status;

/// Mounts a filesystem/block at `source` to a `target` path in the system.
pub(crate) async fn mount<'a>(
    mount_builder: MountBuilder<'a>,
    device: &str,
    target: &str,
) -> Result<(), Status> {
    unsafe {
        match fork() {
            Ok(ForkResult::Parent { child }) => {
                spawn_blocking(move || {
                    let wait_status = waitpid(child, None).map_err(|errno| {
                        Status::aborted(format!(
                            "Failed to wait for mount child process, errno : {}",
                            errno
                        ))
                    })?;
                    match wait_status {
                        nix::sys::wait::WaitStatus::Exited(_, 0) => Ok(()),
                        _ => Err(Status::aborted("The mount process exited non-gracefully")),
                    }
                })
                .await
                .map_err(|error| {
                    Status::aborted(format!(
                        "Failed to wait thread waiting for mount child process {}",
                        error
                    ))
                })??;
                Ok(())
            }
            Ok(ForkResult::Child) => {
                if let Err(errno) = setsid() {
                    return Err(Status::aborted(format!(
                        "Failed to detach mount child process, errno : {}",
                        errno
                    )));
                }

                mount_builder.mount(device, target).map_err(|error| {
                    Status::aborted(format!("Failed to execute mount command, {}", error))
                })?;

                Ok(())
            }
            Err(error) => Err(Status::aborted(format!(
                "Failed to create mount child process, errno :{}",
                error
            ))),
        }
    }?;
    Ok(())
}

/// Unmounts a filesystem/block from a `target` path in the system.
pub(crate) async fn unmount(target: &str, flags: UnmountFlags) -> Result<(), Status> {
    unsafe {
        match fork() {
            Ok(ForkResult::Parent { child }) => {
                spawn_blocking(move || {
                    let wait_status = waitpid(child, None).map_err(|errno| {
                        Status::aborted(format!(
                            "Failed to wait for unmount child process, errno : {}",
                            errno
                        ))
                    })?;
                    match wait_status {
                        nix::sys::wait::WaitStatus::Exited(_, 0) => Ok(()),
                        _ => Err(Status::aborted(
                            "The umount child process exited non-gracefully",
                        )),
                    }
                })
                .await
                .map_err(|error| {
                    Status::aborted(format!(
                        "Failed to wait for the thread waiting on unmount child process {}",
                        error
                    ))
                })??;
                Ok(())
            }
            Ok(ForkResult::Child) => {
                if let Err(errno) = setsid() {
                    return Err(Status::aborted(format!(
                        "Failed to detach the unmount child process, errno : {}",
                        errno
                    )));
                }

                sys_mount::unmount(target, flags)
                    .map_err(|error| Status::aborted(format!("Failed to unmount: {}", error)))?;

                Ok(())
            }
            Err(errno) => Err(Status::aborted(format!(
                "Failed to create the unmount child process, errno : {}",
                errno
            ))),
        }
    }?;
    Ok(())
}
