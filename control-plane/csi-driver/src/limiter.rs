//! Volume Concurrency Limiter Module
//!
//! In general the Cluster Orchestrator (CO) is responsible for ensuring that there is no more
//! than one call “in-flight” per volume at a given time. However, in some circumstances, the
//! CO MAY lose state (for example when the CO crashes and restarts), and MAY issue multiple
//! calls simultaneously for the same volume. The plugin SHOULD handle this as gracefully as
//! possible. The error code ABORTED MAY be returned by the plugin in this case (see the Error
//! Scheme section for details)
use once_cell::sync::OnceCell;
use snafu::Snafu;
use std::{collections::HashSet, sync::Mutex};
use tracing::trace;

/// Volume concurrency limiter error variants.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)), context(suffix(false)))]
pub enum Error {
    #[snafu(display("Existing Csi operation is in progress for volume: {}", id))]
    OperationInProgress { id: String },
    #[snafu(display("Volume id {} not valid", id))]
    InvalidUuid { id: String },
}

impl From<Error> for tonic::Status {
    fn from(error: Error) -> Self {
        match error {
            Error::OperationInProgress { .. } => tonic::Status::aborted(error.to_string()),
            Error::InvalidUuid { .. } => tonic::Status::invalid_argument(error.to_string()),
        }
    }
}

/// This Operation guard is used to ensure there is no more than 1 in-flight operation
/// for a given volume by taking a reservation for the volume's uuid.
/// When this is dropped the reservation is returned and a new operation can be accepted.
pub struct VolumeOpGuard {
    uuid: uuid::Uuid,
}

impl VolumeOpGuard {
    /// Tries to take a new volume reservation and returns error if an existing
    /// operation is already in progress.
    pub fn new_str(uuid: &str) -> Result<Self, Error> {
        Self::new(uuid::Uuid::parse_str(uuid).map_err(|_| Error::InvalidUuid {
            id: uuid.to_string(),
        })?)
    }

    /// Tries to take a new volume reservation and returns error if an existing
    /// operation is already in progress.
    pub fn new(uuid: uuid::Uuid) -> Result<Self, Error> {
        let mut inventory = volume_serializer().lock().unwrap();
        let collision = inventory.get(&uuid).is_some();
        match collision {
            true => {
                trace!(
                    volume.uuid = %uuid,
                    "Operation already exists for volume"
                );
                Err(Error::OperationInProgress {
                    id: uuid.to_string(),
                })
            }
            false => {
                inventory.insert(uuid);
                trace!(volume.uuid = %uuid, "New reservation");
                Ok(Self { uuid })
            }
        }
    }
}

impl Drop for VolumeOpGuard {
    fn drop(&mut self) {
        tracing::info!("Dropping volume: {}", self.uuid);
        let mut hash_set = volume_serializer().lock().unwrap();
        hash_set.remove(&self.uuid);
    }
}

/// Returns or inits a hash set which stores the uuid's of all volumes which
/// have operations in-flight.
/// This can then be used to ensure there is only 1 operation in-flight for a given volume.
fn volume_serializer() -> &'static Mutex<HashSet<uuid::Uuid>> {
    static OPERATION_LIMITER: OnceCell<Mutex<HashSet<uuid::Uuid>>> = OnceCell::new();

    OPERATION_LIMITER.get_or_init(|| Mutex::new(HashSet::new()))
}
