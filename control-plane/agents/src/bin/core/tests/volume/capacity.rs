#![cfg(test)]

use crate::volume::RECONCILE_TIMEOUT_SECS;
use deployer_cluster::{Cluster, ClusterBuilder, FindVolumeRequest};
use grpc::operations::volume::traits::VolumeOperations;
use std::{collections::HashMap, convert::TryFrom, time::Duration};
use stor_port::types::v0::{
    openapi::{models, models::PublishVolumeBody},
    transport::{ChildState, Filter, VolumeStatus},
};

#[tokio::test]
async fn online_enospc_child() {
    let cache_period = Duration::from_millis(250);
    let reconcile_period = Duration::from_millis(3000);
    let cluster = ClusterBuilder::builder()
        .with_rest(true)
        .with_io_engines(2)
        .with_pool(0, "malloc:///p1?size_mb=300")
        .with_pool(1, "malloc:///p1?size_mb=100")
        .with_csi(false, true)
        .with_options(|o| o.with_isolated_io_engine(true))
        .with_cache_period(&humantime::Duration::from(cache_period).to_string())
        .with_reconcile_period(reconcile_period, reconcile_period)
        .build()
        .await
        .unwrap();

    let (volume_1, volume_2) =
        common_enospc_builder(&cluster, cache_period, reconcile_period, 2).await;

    let api_client = cluster.rest_v00();
    let pools_api = api_client.pools_api();
    let volumes_api = api_client.volumes_api();

    let volume_1 = volumes_api.get_volume(&volume_1.spec.uuid).await.unwrap();
    assert_eq!(volume_1.state.status, models::VolumeStatus::Degraded);
    let volume_2 = volumes_api.get_volume(&volume_2.spec.uuid).await.unwrap();
    assert_eq!(volume_2.state.status, models::VolumeStatus::Degraded);

    // Now we add a pool which allows us to move the largest replica
    pools_api
        .put_node_pool(
            cluster.node(1).as_str(),
            cluster.pool(1, 1).as_str(),
            models::CreatePoolBody::new(vec!["malloc:///disk?size_mb=200"]),
        )
        .await
        .unwrap();

    let volume_client = cluster.grpc_client().volume();
    wait_till_1_volume_healthy(&volume_client).await;
}

async fn log_thin(cluster: &Cluster) {
    let api_client = cluster.rest_v00();
    let volumes_api = api_client.volumes_api();
    let pools_api = api_client.pools_api();
    let replica_api = api_client.replicas_api();

    let volumes = volumes_api.get_volumes(0, None, None).await.unwrap();

    for volume in volumes.entries {
        let target = volume.state.target.as_ref().unwrap();
        tracing::info!("VolumeStatus: {} => {target:#?}", volume.spec.uuid);
    }

    let pools = pools_api.get_pools(None).await.unwrap();
    let pools = pools
        .into_iter()
        .map(|p| p.state.unwrap())
        .collect::<Vec<_>>();
    tracing::info!("Here's the pools: {pools:#?}");
    for replica in replica_api.get_replicas().await.unwrap() {
        let space = replica.space.as_ref().unwrap();
        let free = (space.capacity_bytes - space.allocated_bytes) / (1024 * 1024);
        let cap = space.capacity_bytes / (1024 * 1024);
        tracing::info!(
            "Replica {}/{} => {}MB free out of {}MB",
            replica.uuid,
            replica.node,
            free,
            cap
        );
    }
}

struct DeviceDisconnect(nvmeadm::NvmeTarget);
impl Drop for DeviceDisconnect {
    fn drop(&mut self) {
        if self.0.disconnect().is_err() {
            std::process::Command::new(env!("SUDO"))
                .args(["nvme", "disconnect-all"])
                .status()
                .unwrap();
        }
    }
}

async fn wait_till_1_volume_healthy(volume_client: &dyn VolumeOperations) {
    let timeout = Duration::from_secs(RECONCILE_TIMEOUT_SECS);
    let start = std::time::Instant::now();
    loop {
        let volumes = volume_client
            .get(Filter::None, false, None, None)
            .await
            .unwrap()
            .entries;

        for volume in &volumes {
            if volume.status().as_ref().unwrap() == &VolumeStatus::Online {
                let children = volume.state().target.unwrap().children;
                if children.iter().all(|c| c.state == ChildState::Online) {
                    return;
                }
            }
        }

        if std::time::Instant::now() > (start + timeout) {
            tracing::error!("Timeout waiting for a volume to be healthy! Current: {volumes:#?}");
            panic!("Timeout waiting for a volume to be healthy!");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[tokio::test]
async fn faulted_nexus_enospc_child() {
    let cache_period = Duration::from_millis(250);
    let reconcile_period = Duration::from_millis(3000);
    let cluster = ClusterBuilder::builder()
        .with_rest(true)
        .with_io_engines(2)
        .with_pool(1, "malloc:///p1?size_mb=100")
        .with_csi(false, true)
        .with_options(|o| o.with_isolated_io_engine(true))
        .with_cache_period(&humantime::Duration::from(cache_period).to_string())
        .with_reconcile_period(reconcile_period, reconcile_period)
        .build()
        .await
        .unwrap();

    let (volume_1, volume_2) =
        common_enospc_builder(&cluster, cache_period, reconcile_period, 1).await;

    let api_client = cluster.rest_v00();
    let volumes_api = api_client.volumes_api();

    let volume_1 = volumes_api.get_volume(&volume_1.spec.uuid).await.unwrap();
    assert_eq!(volume_1.state.status, models::VolumeStatus::Faulted);
    let volume_2 = volumes_api.get_volume(&volume_2.spec.uuid).await.unwrap();
    assert_eq!(volume_2.state.status, models::VolumeStatus::Faulted);

    volumes_api.del_volume(&volume_2.spec.uuid).await.unwrap();

    // After making up space, the other volume nexus should be recreated!

    let volume_client = cluster.grpc_client().volume();
    wait_till_1_volume_healthy(&volume_client).await;

    let volume_1 = volumes_api.get_volume(&volume_1.spec.uuid).await.unwrap();
    assert_eq!(volume_1.state.status, models::VolumeStatus::Online);
}

async fn common_enospc_builder(
    cluster: &Cluster,
    cache_period: Duration,
    reconcile_period: Duration,
    replicas: u8,
) -> (models::Volume, models::Volume) {
    let api_client = cluster.rest_v00();
    let volumes_api = api_client.volumes_api();
    let pools_api = api_client.pools_api();
    let replica_api = api_client.replicas_api();

    let volume_size = 80u64 * 1024 * 1024;
    let mut volume_1 = volumes_api
        .put_volume(
            &"ec4e66fd-3b33-4439-b504-d49aba53da26".parse().unwrap(),
            models::CreateVolumeBody::new(
                models::VolumePolicy::new(true),
                replicas,
                volume_size,
                true,
            ),
        )
        .await
        .unwrap();
    volume_1 = volumes_api
        .put_volume_target(
            &volume_1.spec.uuid,
            PublishVolumeBody::new_all(
                HashMap::new(),
                None,
                cluster.node(0).to_string(),
                models::VolumeShareProtocol::Nvmf,
                None,
                cluster.csi_node(0),
            ),
        )
        .await
        .unwrap();
    let uri = volume_1.state.target.as_ref().unwrap().device_uri.as_str();
    let _drop_target = DeviceDisconnect(nvmeadm::NvmeTarget::try_from(uri).unwrap());

    let mut volume_2 = volumes_api
        .put_volume(
            &"ec4e66fd-3b33-4439-b504-d49aba53da27".parse().unwrap(),
            models::CreateVolumeBody::new(
                models::VolumePolicy::new(true),
                replicas,
                volume_size,
                true,
            ),
        )
        .await
        .unwrap();
    volume_2 = volumes_api
        .put_volume_target(
            &volume_2.spec.uuid,
            PublishVolumeBody::new_all(
                HashMap::new(),
                None,
                cluster.node(0).to_string(),
                models::VolumeShareProtocol::Nvmf,
                None,
                cluster.csi_node(0),
            ),
        )
        .await
        .unwrap();
    let uri = volume_2.state.target.as_ref().unwrap().device_uri.as_str();
    let _drop_target2 = DeviceDisconnect(nvmeadm::NvmeTarget::try_from(uri).unwrap());

    let pools = pools_api.get_pools(None).await.unwrap();
    tracing::info!(?pools, "Here's the pools");

    let replicas = replica_api.get_replicas().await.unwrap();
    tracing::info!(?replicas, "Here's the replicas");

    let mut node = cluster.csi_node_client(0).await.unwrap();
    node.node_stage_volume(&volume_1, HashMap::new())
        .await
        .unwrap();
    let response = node
        .internal()
        .find_volume(FindVolumeRequest {
            volume_id: volume_1.spec.uuid.to_string(),
        })
        .await
        .unwrap();
    tracing::info!(?response);
    let device_path_1 = response.into_inner().device_path;

    node.node_stage_volume(&volume_2, HashMap::new())
        .await
        .unwrap();
    let response = node
        .internal()
        .find_volume(FindVolumeRequest {
            volume_id: volume_2.spec.uuid.to_string(),
        })
        .await
        .unwrap();
    tracing::info!(?response);
    let device_path_2 = response.into_inner().device_path;

    let name = cluster.csi_container(0);
    let dev_path_1 = format!("of={device_path_1}");

    let dd_1 = cluster.composer().exec(
        name.as_str(),
        vec![
            "dd",
            "if=/dev/urandom",
            dev_path_1.as_str(),
            "bs=64k",
            "count=640",
        ],
    );
    let dev_path_2 = format!("of={device_path_2}");
    let dd_2 = cluster.composer().exec(
        name.as_str(),
        vec![
            "dd",
            "if=/dev/urandom",
            dev_path_2.as_str(),
            "bs=64k",
            "count=640",
        ],
    );

    let output = futures::future::join(dd_1, dd_2).await;
    tracing::info!("\n{:?}", output);

    // 1. really need a way to by pass the cache!
    // for now simply wait for the cache period!
    tokio::time::sleep(cache_period * 2).await;
    log_thin(cluster).await;

    let dd_1 = cluster.composer().exec(
        name.as_str(),
        vec!["dd", "if=/dev/urandom", dev_path_1.as_str(), "bs=64k"],
    );
    let dd_2 = cluster.composer().exec(
        name.as_str(),
        vec!["dd", "if=/dev/urandom", dev_path_2.as_str(), "bs=64k"],
    );
    let output = futures::future::join(dd_1, dd_2).await;

    cluster
        .composer()
        .exec(
            cluster.csi_container(0).as_str(),
            vec!["nvme", "disconnect-all"],
        )
        .await
        .unwrap();

    tracing::info!("\n{:?}", output);

    tokio::time::sleep(reconcile_period * 2).await;
    log_thin(cluster).await;

    // no children should be removed as there's no space for the larger replica
    let current_replicas = replica_api.get_replicas().await.unwrap();
    let new_replicas = current_replicas
        .iter()
        .any(|r| !replicas.iter().any(|or| or.uuid == r.uuid));
    assert!(!new_replicas, "There should be no new replicas");

    (volume_1, volume_2)
}
