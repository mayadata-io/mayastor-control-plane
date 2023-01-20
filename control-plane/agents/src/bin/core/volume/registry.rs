use crate::controller::registry::Registry;
use agents::errors::SvcError;
use common_lib::types::v0::transport::{
    uri_with_hostnqn, NexusStatus, ReplicaTopology, Volume, VolumeId, VolumeState, VolumeStatus,
};

use crate::controller::reconciler::PollTriggerEvent;
use common_lib::types::v0::store::{replica::ReplicaSpec, volume::VolumeSpec};
use grpc::operations::{PaginatedResult, Pagination};
use std::collections::HashMap;

impl Registry {
    /// Get the volume state for the specified volume.
    pub(crate) async fn volume_state(
        &self,
        volume_uuid: &VolumeId,
    ) -> Result<VolumeState, SvcError> {
        let volume_spec = self.specs().volume_clone(volume_uuid)?;
        let replica_specs = self.specs().volume_replicas_cln(volume_uuid);

        self.volume_state_with_replicas(&volume_spec, &replica_specs)
            .await
    }

    /// Get the volume state for the specified volume.
    /// replicas is a pre-fetched list of replicas from any and all volumes.
    pub(crate) async fn volume_state_with_replicas(
        &self,
        volume_spec: &VolumeSpec,
        replicas: &[ReplicaSpec],
    ) -> Result<VolumeState, SvcError> {
        let replica_specs = replicas
            .iter()
            .filter(|r| r.owners.owned_by(&volume_spec.uuid))
            .collect::<Vec<_>>();

        let nexus_spec = self.specs().volume_target_nexus_rsc(volume_spec);
        let nexus = match nexus_spec {
            None => None,
            Some(spec) => {
                let nexus_id = spec.lock().uuid.clone();
                self.nexus(&nexus_id).await.ok().map(|s| (spec, s))
            }
        };

        // Construct the topological information for the volume replicas.
        let mut replica_topology = HashMap::new();
        for replica_spec in &replica_specs {
            replica_topology.insert(
                replica_spec.uuid.clone(),
                self.replica_topology(replica_spec).await,
            );
        }

        Ok(if let Some((nexus, mut nexus_state)) = nexus {
            let ah = nexus.lock().allowed_hosts.clone();
            nexus_state.device_uri = uri_with_hostnqn(&nexus_state.device_uri, &ah);
            VolumeState {
                uuid: volume_spec.uuid.to_owned(),
                size: nexus_state.size,
                status: match nexus_state.status {
                    NexusStatus::Online
                        if nexus_state.children.len() != volume_spec.num_replicas as usize =>
                    {
                        VolumeStatus::Degraded
                    }
                    _ => nexus_state.status.clone(),
                },
                target: Some(nexus_state),
                replica_topology,
            }
        } else {
            VolumeState {
                uuid: volume_spec.uuid.to_owned(),
                size: volume_spec.size,
                status: if volume_spec.target().is_none() {
                    if replica_specs.len() >= volume_spec.num_replicas as usize {
                        VolumeStatus::Online
                    } else if replica_specs.is_empty() {
                        VolumeStatus::Faulted
                    } else {
                        VolumeStatus::Degraded
                    }
                } else {
                    VolumeStatus::Unknown
                },
                target: None,
                replica_topology,
            }
        })
    }

    /// Construct a replica topology from a replica spec.
    /// If the replica cannot be found, return the default replica topology.
    async fn replica_topology(&self, spec: &ReplicaSpec) -> ReplicaTopology {
        match self.get_replica(&spec.uuid).await {
            Ok(state) => ReplicaTopology::new(Some(state.node), Some(state.pool_id), state.status),
            Err(_) => {
                tracing::trace!(replica.uuid = %spec.uuid, "Replica not found. Constructing default replica topology");
                ReplicaTopology::default()
            }
        }
    }

    /// Get all volumes.
    pub(crate) async fn volumes(&self) -> Vec<Volume> {
        let volume_specs = self.specs().volumes();
        let replicas = self.specs().replicas_cloned();
        let mut volumes = Vec::with_capacity(volume_specs.len());
        for spec in volume_specs {
            if let Ok(state) = self.volume_state_with_replicas(&spec, &replicas).await {
                volumes.push(Volume::new(spec, state));
            }
        }
        volumes
    }

    /// Get a paginated subset of volumes.
    pub(super) async fn paginated_volumes(
        &self,
        pagination: &Pagination,
    ) -> PaginatedResult<Volume> {
        let volume_specs = self.specs().paginated_volumes(pagination);
        let mut volumes = Vec::with_capacity(volume_specs.len());
        let last = volume_specs.last();
        for spec in volume_specs.result() {
            if let Ok(state) = self.volume_state(&spec.uuid).await {
                volumes.push(Volume::new(spec, state));
            }
        }
        PaginatedResult::new(volumes, last)
    }

    /// Return a volume object corresponding to the ID.
    pub(crate) async fn volume(&self, id: &VolumeId) -> Result<Volume, SvcError> {
        Ok(Volume::new(
            self.specs().volume_clone(id)?,
            self.volume_state(id).await?,
        ))
    }

    /// Notify the reconcilers if the volume is degraded.
    pub(crate) async fn notify_if_degraded(&self, volume: &Volume, event: PollTriggerEvent) {
        if volume.status() == Some(VolumeStatus::Degraded) {
            self.notify(event).await;
        }
    }
}
