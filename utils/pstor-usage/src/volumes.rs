use crate::resources::{
    FormatSamples, ResourceDelete, ResourceMgr, ResourceSample, ResourceUpdates,
};
use openapi::{apis::Uuid, clients::tower::direct::ApiClient, models, models::PublishVolumeBody};
use std::collections::HashMap;

/// Resource manager for volumes.
#[derive(Default)]
pub(crate) struct VolMgr {
    n_replicas: u8,
    size_bytes: u64,
}
impl VolMgr {
    /// New `ResourceMgr` for volumes with `n_replicas` replicas and size in bytes.
    pub(crate) async fn new_mgr(
        n_replicas: u8,
        size_bytes: u64,
    ) -> anyhow::Result<impl ResourceMgr> {
        Ok(Self {
            n_replicas,
            size_bytes,
        })
    }
}
#[async_trait::async_trait]
impl ResourceMgr for VolMgr {
    type Output = Vec<models::Volume>;
    async fn create(&self, client: &ApiClient, count: u32) -> anyhow::Result<Self::Output> {
        let mut created_volumes = Vec::with_capacity(count as usize);
        for _ in 0 .. count {
            let volume = client
                .volumes_api()
                .put_volume(
                    &Uuid::new_v4(),
                    models::CreateVolumeBody::new(
                        models::VolumePolicy::new(false),
                        self.n_replicas,
                        self.size_bytes,
                        false,
                    ),
                )
                .await?;
            created_volumes.push(volume);
        }
        Ok(created_volumes)
    }
    async fn delete(&self, client: &ApiClient, created: Self::Output) -> anyhow::Result<()> {
        created.delete(client).await
    }
}

#[async_trait::async_trait]
impl FormatSamples for VolMgr {
    fn format(&self, points: Vec<u64>) -> Box<dyn ResourceSample> {
        let replicas = self.n_replicas;
        Box::new(VolumeCount { points, replicas })
    }
}

#[async_trait::async_trait]
impl ResourceDelete for Vec<models::Volume> {
    async fn delete(&self, client: &ApiClient) -> anyhow::Result<()> {
        for volume in self {
            client.volumes_api().del_volume(&volume.spec.uuid).await?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl ResourceUpdates for Vec<models::Volume> {
    async fn modify(&self, client: &ApiClient, count: u32) -> anyhow::Result<()> {
        let nodes = client.nodes_api().get_nodes(None).await?;
        let node_ids = nodes.into_iter().map(|n| n.id).collect::<Vec<_>>();
        let mut node_index = 0;

        for (churns, volume) in self.iter().enumerate() {
            if churns >= count as usize {
                break;
            }

            let node_id = node_ids.get(node_index).expect("Should have nodes");
            client
                .volumes_api()
                .put_volume_target(
                    &volume.spec.uuid,
                    PublishVolumeBody::new_all(
                        HashMap::new(),
                        None,
                        Some(node_id.to_string()),
                        models::VolumeShareProtocol::Nvmf,
                        None,
                        "".to_string(),
                    ),
                )
                .await?;
            node_index = (node_index + 1) % node_ids.len();

            client
                .volumes_api()
                .del_volume_target(&volume.spec.uuid, Some(true), Some(node_id.as_str()))
                .await?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl FormatSamples for Vec<models::Volume> {
    fn format(&self, points: Vec<u64>) -> Box<dyn ResourceSample> {
        let replicas = self
            .first()
            .map(|v| v.spec.num_replicas)
            .unwrap_or_default();
        Box::new(VolumeMod { points, replicas })
    }
}

struct VolumeMod {
    points: Vec<u64>,
    replicas: u8,
}
impl ResourceSample for VolumeMod {
    fn points(&self) -> &Vec<u64> {
        &self.points
    }
    fn points_mut(&mut self) -> &mut Vec<u64> {
        &mut self.points
    }

    fn name(&self) -> String {
        "Volume~Repl Mods".to_string()
    }

    fn format_point(&self, point: u64) -> String {
        format!("{}~{}", point, self.replicas)
    }
}

struct VolumeCount {
    points: Vec<u64>,
    replicas: u8,
}
impl ResourceSample for VolumeCount {
    fn points(&self) -> &Vec<u64> {
        &self.points
    }
    fn points_mut(&mut self) -> &mut Vec<u64> {
        &mut self.points
    }

    fn name(&self) -> String {
        "Volumes ~Repl".to_string()
    }

    fn format_point(&self, point: u64) -> String {
        format!("{} ~{}", point, self.replicas)
    }
}
