//! Definition of DeviceError used by the attach and detach code.
use nix::errno::Errno;
use nvmeadm::{error::NvmeError, nvmf_discovery};
use snafu::Snafu;
use std::{process::ExitCode, string::FromUtf8Error};
use tokio::task::JoinError;

/// A Device Attach/Detach error.
/// todo: should this be an enum?
pub(crate) struct DeviceError {
    pub(crate) message: String,
    pub(crate) source: DeviceErrorSource,
}

/// Possible device error kind sources.
#[allow(dead_code)]
pub(crate) enum DeviceErrorSource {
    None,
    NvmeError(NvmeError),
    StdIoError(std::io::Error),
    StdIntParseError(std::num::ParseIntError),
    UuidError(uuid::Error),
    NvmfConnectArgsBuilderError(nvmf_discovery::ConnectArgsBuilderError),
    GenericError(anyhow::Error),
    SerdeError(serde_json::error::Error),
    FromUtf8Error(FromUtf8Error),
}

impl DeviceError {
    /// Return a new `Self` with the given message.
    pub(crate) fn new(message: &str) -> DeviceError {
        DeviceError {
            message: String::from(message),
            source: DeviceErrorSource::None,
        }
    }
}

impl std::fmt::Debug for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for DeviceError {
    fn description(&self) -> &str {
        &self.message
    }
}

impl From<std::io::Error> for DeviceError {
    fn from(error: std::io::Error) -> DeviceError {
        DeviceError {
            message: format!("{error}"),
            source: DeviceErrorSource::StdIoError(error),
        }
    }
}

impl From<std::num::ParseIntError> for DeviceError {
    fn from(error: std::num::ParseIntError) -> DeviceError {
        DeviceError {
            message: format!("{error}"),
            source: DeviceErrorSource::StdIntParseError(error),
        }
    }
}

impl From<uuid::Error> for DeviceError {
    fn from(error: uuid::Error) -> DeviceError {
        DeviceError {
            message: format!("{error}"),
            source: DeviceErrorSource::UuidError(error),
        }
    }
}

impl From<nvmf_discovery::ConnectArgsBuilderError> for DeviceError {
    fn from(error: nvmf_discovery::ConnectArgsBuilderError) -> DeviceError {
        DeviceError {
            message: format!("{error}"),
            source: DeviceErrorSource::NvmfConnectArgsBuilderError(error),
        }
    }
}

impl From<String> for DeviceError {
    fn from(message: String) -> DeviceError {
        DeviceError {
            message,
            source: DeviceErrorSource::None,
        }
    }
}

impl From<serde_json::error::Error> for DeviceError {
    fn from(error: serde_json::error::Error) -> DeviceError {
        DeviceError {
            message: format!("{error}"),
            source: DeviceErrorSource::SerdeError(error),
        }
    }
}

impl From<FromUtf8Error> for DeviceError {
    fn from(error: FromUtf8Error) -> DeviceError {
        DeviceError {
            message: format!("{error}"),
            source: DeviceErrorSource::FromUtf8Error(error),
        }
    }
}

impl From<nvmeadm::error::NvmeError> for DeviceError {
    fn from(error: nvmeadm::error::NvmeError) -> DeviceError {
        DeviceError {
            message: format!("{error}"),
            source: DeviceErrorSource::NvmeError(error),
        }
    }
}

impl From<anyhow::Error> for DeviceError {
    fn from(error: anyhow::Error) -> DeviceError {
        DeviceError {
            message: error.to_string(),
            source: DeviceErrorSource::GenericError(error),
        }
    }
}

impl From<DeviceError> for tonic::Status {
    fn from(dev_error: DeviceError) -> Self {
        match dev_error.source {
            DeviceErrorSource::NvmeError(error) => match error {
                NvmeError::IoFailed { .. } | NvmeError::ConnectFailed { .. } => {
                    tonic::Status::aborted(dev_error.message)
                }
                NvmeError::ConnectInProgress => tonic::Status::already_exists(dev_error.message),
                _ => tonic::Status::internal(dev_error.message),
            },
            _ => tonic::Status::internal(dev_error.message),
        }
    }
}

/// Error for the fsfreeze operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)), context(suffix(false)))]
pub(crate) enum FsfreezeError {
    #[snafu(display("Cannot find volume: volume ID: {volume_id}"))]
    VolumeNotFound { volume_id: String },
    #[snafu(display("Volume is not mounted: volume ID: {volume_id}"))]
    VolumeNotMounted { volume_id: String },
    #[snafu(display("Invalid volume ID: {volume_id}, {source}"))]
    InvalidVolumeId {
        source: uuid::Error,
        volume_id: String,
    },
    #[snafu(display("fsfreeze failed: volume ID: {volume_id}, {errno}"))]
    FsfreezeFailed { volume_id: String, errno: Errno },
    #[snafu(display("Internal failure: volume ID: {volume_id}, {source}"))]
    InternalFailure {
        source: DeviceError,
        volume_id: String,
    },
    #[snafu(display("Not a filesystem mount: volume ID: {volume_id}"))]
    BlockDeviceMount { volume_id: String },
}

impl From<FsfreezeError> for ExitCode {
    fn from(value: FsfreezeError) -> Self {
        match value {
            FsfreezeError::VolumeNotFound { .. } => ExitCode::from(Errno::ENODEV as u8),
            FsfreezeError::VolumeNotMounted { .. } => ExitCode::from(Errno::ENOENT as u8),
            FsfreezeError::InvalidVolumeId { .. } => ExitCode::from(Errno::EINVAL as u8),
            FsfreezeError::FsfreezeFailed { errno, .. } => ExitCode::from(errno as u8),
            FsfreezeError::InternalFailure { .. } => ExitCode::from(Errno::ELIBACC as u8),
            FsfreezeError::BlockDeviceMount { .. } => ExitCode::from(Errno::EMEDIUMTYPE as u8),
        }
    }
}

/// Error for the mount operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)), context(suffix(false)))]
pub(crate) enum MountError {
    #[snafu(display("Failed mount, error: {error}"))]
    MountFailed { error: std::io::Error },
    #[snafu(display("Failed unmount, error: {error}"))]
    UnmountFailed { error: std::io::Error },
    #[snafu(display("Failed to wait for thread termination, error: {join_error}"))]
    FailedWaitForThread { join_error: JoinError },
}

impl From<MountError> for ExitCode {
    fn from(value: MountError) -> Self {
        match value {
            MountError::MountFailed { .. } => ExitCode::from(Errno::ELIBACC as u8),
            MountError::UnmountFailed { .. } => ExitCode::from(Errno::ELIBACC as u8),
            MountError::FailedWaitForThread { .. } => ExitCode::from(Errno::ESRCH as u8),
        }
    }
}

/// Error for the csi driver operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)), context(suffix(false)))]
pub(crate) enum CsiDriverError {
    #[snafu(display("{source}"))]
    Fsfreeze { source: FsfreezeError },
    #[snafu(display("{source}"))]
    Mount { source: MountError },
    #[snafu(display("Not a valid mayastor csi driver command"))]
    InvalidCsiDriverCommand,
}

impl From<FsfreezeError> for CsiDriverError {
    fn from(value: FsfreezeError) -> Self {
        Self::Fsfreeze { source: value }
    }
}

impl From<MountError> for CsiDriverError {
    fn from(value: MountError) -> Self {
        Self::Mount { source: value }
    }
}

impl From<CsiDriverError> for ExitCode {
    fn from(value: CsiDriverError) -> Self {
        match value {
            CsiDriverError::Fsfreeze { source } => source.into(),
            CsiDriverError::Mount { source } => source.into(),
            CsiDriverError::InvalidCsiDriverCommand => ExitCode::from(Errno::EINVAL as u8),
        }
    }
}
