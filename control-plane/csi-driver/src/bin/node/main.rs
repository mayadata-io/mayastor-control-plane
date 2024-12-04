use crate::error::FsfreezeError;
use std::process::ExitCode;

/// todo: cleanup this module cfg repetition..
#[cfg(target_os = "linux")]
mod block_vol;
mod client;
/// Configuration Parameters.
#[cfg(target_os = "linux")]
pub(crate) mod config;
#[cfg(target_os = "linux")]
mod dev;
#[cfg(target_os = "linux")]
mod error;
/// Filesystem specific operations.
pub(crate) mod filesystem_ops;
#[cfg(target_os = "linux")]
mod filesystem_vol;
#[cfg(target_os = "linux")]
mod findmnt;
#[cfg(target_os = "linux")]
mod format;
pub(crate) mod fsfreeze;
#[cfg(target_os = "linux")]
mod identity;
pub(crate) mod k8s;
#[cfg(target_os = "linux")]
mod main_;
#[cfg(target_os = "linux")]
mod match_dev;
#[cfg(target_os = "linux")]
mod mount;
#[cfg(target_os = "linux")]
mod node;
#[cfg(target_os = "linux")]
mod nodeplugin_grpc;
#[cfg(target_os = "linux")]
mod nodeplugin_nvme;
#[cfg(target_os = "linux")]
mod nodeplugin_svc;
mod registration;
mod runtime;
/// Shutdown event which lets the plugin know it needs to stop processing new events and
/// complete any existing ones before shutting down.
#[cfg(target_os = "linux")]
pub(crate) mod shutdown_event;

#[tokio::main]
#[cfg(target_os = "linux")]
async fn main() -> anyhow::Result<ExitCode> {
    match main_::main().await.map_err(|error| {
        tracing::error!(%error, "Terminated with error");
        error
    }) {
        Ok(_) => Ok(ExitCode::SUCCESS),
        Err(error) => match error.downcast::<FsfreezeError>() {
            Ok(error) => Ok(error.into()),
            Err(error) => Err(error),
        },
    }
}

#[tokio::main]
#[cfg(not(target_os = "linux"))]
async fn main() -> anyhow::Result<()> {
    Ok(())
}
