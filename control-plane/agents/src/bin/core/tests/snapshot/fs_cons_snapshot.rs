#![cfg(test)]

use deployer_cluster::{Cluster, ClusterBuilder};
use std::time::Duration;
use stor_port::types::v0::openapi::models;

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

const VOLUME_UUID: &str = "ec4e66fd-3b33-4439-b504-d49aba53da26";
const SNAPSHOT_UUID: &str = "b04206a4-314a-484e-814e-37c863d92dcc";
const VOLUME_SIZE: u64 = 80u64 * 1024 * 1024;

#[tokio::test]
async fn fs_consistent_snapshot() {
    let cache_period = Duration::from_millis(250);
    let reconcile_period = Duration::from_millis(3000);
    let cluster = ClusterBuilder::builder()
        .with_rest(true)
        .with_io_engines(1)
        .with_tmpfs_pool_ix(0, 104857600)
        .with_csi(true, true)
        .with_csi_registration(true)
        .with_cache_period(&humantime::Duration::from(cache_period).to_string())
        .with_reconcile_period(reconcile_period, reconcile_period)
        .build()
        .await
        .expect("Failed to build cluster");

    let api_client = cluster.rest_v00();
    let volumes_api = api_client.volumes_api();

    let volume = volumes_api
        .put_volume(
            &VOLUME_UUID.parse().unwrap(),
            models::CreateVolumeBody::new(models::VolumePolicy::new(true), 1, VOLUME_SIZE, true),
        )
        .await
        .expect("Failed to create volume");

    preflight_failure(&cluster, &volume).await;
    blockdevice_mount(&cluster, &volume).await;
    unpublished_volume_snapshot(&cluster, &volume).await;
}

/// Takes a snapshot of a raw block volume. This should not fail if quiesceFs is set to true.
/// If quiesceFs is set to true, and the volume happens to be a blockdevice mount we log and
/// continue.
///
/// This function performs several operations on a volume in a cluster:
/// - Publishes the volume using the controller client
/// - Stages and publishes the volume on the node
/// - Creates a snapshot of the volume
/// - Unpublishes and unstages the volume from the node
/// - Unpublishes the volume from the controller
/// - Deletes the snapshot
///
/// # Panics
///
/// This function will panic if any of the following operations fail:
/// - Retrieving the node or controller client
/// - Publishing, staging, unpublishing, or unstaging the volume
/// - Creating or deleting the snapshot
async fn blockdevice_mount(cluster: &Cluster, volume: &models::Volume) {
    let api_client = cluster.rest_v00();
    let volumes_api = api_client.volumes_api();

    let mut node = cluster
        .csi_node_client(0)
        .await
        .expect("Failed to get node client");
    let mut controller = cluster
        .csi_controller_client()
        .await
        .expect("Failed to get controller client");

    let publish_result = controller
        .controller_publish_volume(volume, &cluster.csi_node(0))
        .await
        .expect("Failed to publish volume");

    let uri = publish_result
        .publish_context
        .get("uri")
        .expect("Failed to get uri")
        .to_string();
    let _nvme_io_subsys = DeviceDisconnect(nvmeadm::NvmeTarget::try_from(uri).unwrap());

    let volume = volumes_api
        .get_volume(&VOLUME_UUID.parse().unwrap())
        .await
        .expect("Failed to get volume");

    node.node_stage_volume(&volume, publish_result.publish_context.clone())
        .await
        .expect("Failed to stage volume");
    node.node_publish_volume(&volume, publish_result.publish_context)
        .await
        .expect("Failed to publish volume");

    controller
        .create_snapshot(&volume, &format!("snapshot-{}", SNAPSHOT_UUID), true)
        .await
        .expect("Snapshot creation should not fail if it's a raw");

    node.node_unpublish_volume(&volume)
        .await
        .expect("Failed to unpublish volume");
    node.node_unstage_volume(&volume)
        .await
        .expect("Failed to unstage volume");
    controller
        .controller_unpublish_volume(&volume, &cluster.csi_node(0))
        .await
        .expect("Failed to publish volume");

    controller
        .delete_snapshot(SNAPSHOT_UUID)
        .await
        .expect("Failed to delete snapshot");
}

/// Take a snapshot of an unpublished volume, with quiesceFs set to true.
/// This should not fail, as the volume is not published. We don't quiesce the filesystem for
/// unpublished volumes.
///
/// This function performs several operations on a volume in a cluster:
/// - Publishes the volume using the controller client
/// - Stages and publishes the volume on the node
/// - Unpublishes and unstages the volume from the node
/// - Unpublishes the volume from the controller
/// - Creates a snapshot of the volume
/// - Deletes the snapshot
///
/// # Panics
///
/// This function will panic if any of the following operations fail:
/// - Retrieving the node or controller client
/// - Publishing, staging, unpublishing, or unstaging the volume
/// - Creating or deleting the snapshot
async fn unpublished_volume_snapshot(cluster: &Cluster, volume: &models::Volume) {
    let api_client = cluster.rest_v00();
    let volumes_api = api_client.volumes_api();

    let mut node = cluster
        .csi_node_client(0)
        .await
        .expect("Failed to get node client");
    let mut controller = cluster
        .csi_controller_client()
        .await
        .expect("Failed to get controller client");

    let publish_result = controller
        .controller_publish_volume(volume, &cluster.csi_node(0))
        .await
        .expect("Failed to publish volume");

    let uri = publish_result
        .publish_context
        .get("uri")
        .expect("Failed to get uri")
        .to_string();
    let _nvme_io_subsys = DeviceDisconnect(nvmeadm::NvmeTarget::try_from(uri).unwrap());

    let volume = volumes_api
        .get_volume(&VOLUME_UUID.parse().unwrap())
        .await
        .expect("Failed to get volume");

    node.node_stage_volume(&volume, publish_result.publish_context.clone())
        .await
        .expect("Failed to stage volume");
    node.node_publish_volume(&volume, publish_result.publish_context)
        .await
        .expect("Failed to publish volume");

    node.node_unpublish_volume(&volume)
        .await
        .expect("Failed to unpublish volume");
    node.node_unstage_volume(&volume)
        .await
        .expect("Failed to unstage volume");
    controller
        .controller_unpublish_volume(&volume, &cluster.csi_node(0))
        .await
        .expect("Failed to publish volume");

    controller
        .create_snapshot(&volume, &format!("snapshot-{}", SNAPSHOT_UUID), true)
        .await
        .expect("Snapshot creation should not fail if it's unpublished atm");

    controller
        .delete_snapshot(SNAPSHOT_UUID)
        .await
        .expect("Failed to delete snapshot");
}

/// This takes snapshot of a volume after bringing the io-engine down. In this case, the snapshot
/// creation should fail due to preflight check failure, which is no live path in the subsystem.
///
/// This function performs several operations on a volume in a cluster:
/// - Publishes the volume using the controller client
/// - Stages and publishes the volume on the node with a filesystem type of "ext4"
/// - Simulates a failure by killing the io-engine of the cluster
/// - Attempts to create a snapshot of the volume, which is expected to fail
/// - Restarts the io-engine
/// - Unpublishes and unstages the volume from the node
/// - Unpublishes the volume from the controller
/// - Attempts to delete the snapshot, which is expected to fail
///
/// # Panics
///
/// This function will panic if any of the following operations fail:
/// - Retrieving the node or controller client
/// - Publishing, staging, unpublishing, or unstaging the volume
/// - Killing or starting the io-engine
/// - Creating the snapshot does not fail
/// - Deletes the snapshot fails
async fn preflight_failure(cluster: &Cluster, volume: &models::Volume) {
    let api_client = cluster.rest_v00();
    let volumes_api = api_client.volumes_api();

    let mut node = cluster
        .csi_node_client(0)
        .await
        .expect("Failed to get node client");
    let mut controller = cluster
        .csi_controller_client()
        .await
        .expect("Failed to get controller client");

    let publish_result = controller
        .controller_publish_volume(volume, &cluster.csi_node(0))
        .await
        .expect("Failed to publish volume");

    let uri = publish_result
        .publish_context
        .get("uri")
        .expect("Failed to get uri")
        .to_string();
    let _nvme_io_subsys = DeviceDisconnect(nvmeadm::NvmeTarget::try_from(uri).unwrap());

    let volume = volumes_api
        .get_volume(&VOLUME_UUID.parse().unwrap())
        .await
        .expect("Failed to get volume");

    node.node_stage_volume_fs(&volume, "ext4", publish_result.publish_context.clone())
        .await
        .expect("Failed to stage volume");
    node.node_publish_volume_fs(&volume, "ext4", publish_result.publish_context)
        .await
        .expect("Failed to publish volume");

    cluster
        .composer()
        .kill(&cluster.node(0))
        .await
        .expect("Failed to kill io-engine");

    controller
        .create_snapshot(&volume, &format!("snapshot-{}", SNAPSHOT_UUID), true)
        .await
        .expect_err("Snapshot creation should fail due to preflight check failure");

    cluster
        .composer()
        .start(&cluster.node(0))
        .await
        .expect("Failed to start io-engine");

    node.node_unpublish_volume(&volume)
        .await
        .expect("Failed to unpublish volume");
    node.node_unstage_volume(&volume)
        .await
        .expect("Failed to unstage volume");
    controller
        .controller_unpublish_volume(&volume, &cluster.csi_node(0))
        .await
        .expect("Failed to publish volume");

    controller
        .delete_snapshot(SNAPSHOT_UUID)
        .await
        .expect("Failed to delete snapshot");
}
