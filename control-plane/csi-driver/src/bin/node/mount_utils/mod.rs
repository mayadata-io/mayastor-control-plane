pub(crate) mod bin;

use tokio::process::Command;
use tonic::Status;
use tracing::debug;

const CSI_NODE_BINARY: &str = "csi-node";
const MOUNT: &str = "mount";
const UNMOUNT: &str = "unmount";
const SOURCE: &str = "--source";
const DATA: &str = "--data";
const FSTYPE: &str = "--fstype";
const TARGET: &str = "--target";
const MOUNT_FLAGS: &str = "--mount-flags";
const UNMOUNT_FLAGS: &str = "--unmount-flags";

/// Builder for mounting and unmounting mayastor devices by spawning a process.
#[derive(Debug)]
pub(crate) struct MayastorMount {
    binary_name: String,
    operation: String,
    source: String,
    target: String,
    data: Option<String>,
    fstype: Option<String>,
    flags: String,
}

impl std::fmt::Display for MayastorMount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "operation: {}, args: source={}, target={}, data={}, fstype={}, flags={}",
            self.operation,
            self.source,
            self.target,
            self.data.as_deref().unwrap_or("None"),
            self.fstype.as_deref().unwrap_or("None"),
            self.flags
        )
    }
}

impl MayastorMount {
    /// Get an initialized builder for MayastorMount.
    pub(crate) fn builder() -> Self {
        Self {
            binary_name: CSI_NODE_BINARY.to_string(),
            operation: String::new(),
            source: String::new(),
            target: String::new(),
            data: None,
            fstype: None,
            flags: String::new(),
        }
    }

    /// Set mount source.
    pub(crate) fn source(mut self, path: &str) -> Self {
        self.source = path.into();
        self
    }

    /// Set mount/umount target.
    pub(crate) fn target(mut self, path: &str) -> Self {
        self.target = path.into();
        self
    }

    /// Options to apply for the file system on mount.
    pub(crate) fn data(mut self, data: &str) -> Self {
        self.data = Some(data.into());
        self
    }

    /// The file system that is to be mounted.
    pub(crate) fn fstype(mut self, fstype: &str) -> Self {
        self.fstype = Some(fstype.into());
        self
    }

    /// Mount flags for the mount syscall.
    pub(crate) fn flags(mut self, flags: &str) -> Self {
        self.flags = flags.into();
        self
    }

    /// Mounts a filesystem/block at `source` to a `target` path in the system.
    pub(crate) async fn mount(mut self) -> Result<Self, Status> {
        self.operation = MOUNT.into();
        let mut command = Command::new(&self.binary_name);
        command
            .arg(&self.operation)
            .arg(SOURCE)
            .arg(&self.source)
            .arg(TARGET)
            .arg(&self.target)
            .arg(MOUNT_FLAGS)
            .arg(&self.flags);

        if let Some(data) = &self.data {
            command.arg(DATA).arg(data);
        }
        if let Some(fstype) = &self.fstype {
            command.arg(FSTYPE).arg(fstype);
        }

        debug!("Issuing {}", self);
        spawn_and_wait_for_output(command).await.map(|_| self)
    }

    /// Unmounts a filesystem/block from a `target` path in the system.
    pub(crate) async fn unmount(mut self) -> Result<(), Status> {
        self.operation = UNMOUNT.into();

        let mut command = Command::new(&self.binary_name);
        command
            .arg(&self.operation)
            .arg(TARGET)
            .arg(&self.target)
            .arg(UNMOUNT_FLAGS)
            .arg(&self.flags);

        debug!("Issuing {}", self);
        spawn_and_wait_for_output(command).await
    }
}

async fn spawn_and_wait_for_output(mut command: Command) -> Result<(), Status> {
    let child = command
        .spawn()
        .map_err(|_| Status::aborted("Failed to spawn"))?;

    match child.wait_with_output().await {
        Ok(output) => {
            if output.status.success() {
                return Ok(());
            }

            Err(Status::aborted(format!(
                "Command failed: {}",
                String::from_utf8(output.stderr).unwrap()
            )))
        }
        Err(error) => Err(Status::aborted(format!(
            "Failed to execute command: {}",
            error
        ))),
    }
}
