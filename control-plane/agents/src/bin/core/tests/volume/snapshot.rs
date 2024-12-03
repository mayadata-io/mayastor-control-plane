use crate::volume::helpers::wait_node_online;
use deployer_cluster::{Cluster, ClusterBuilder, FindVolumeRequest};
use grpc::operations::{
    pool::traits::PoolOperations,
    replica::traits::ReplicaOperations,
    volume::traits::{CreateVolumeSnapshot, DestroyVolumeSnapshot, VolumeOperations},
};
use std::{collections::HashMap, time::Duration};
use stor_port::{
    transport_api::{ReplyErrorKind, TimeoutOptions},
    types::v0::{
        openapi::models,
        transport::{
            CreateReplica, CreateVolume, DestroyPool, DestroyReplica, DestroyVolume, Filter,
            PublishVolume, ReplicaId, SetVolumeReplica, SnapshotId, Volume, VolumeShareProtocol,
            VolumeStatus,
        },
    },
};

#[tokio::test]
async fn snapshot() {
    let cluster = ClusterBuilder::builder()
        .with_rest(true)
        .with_io_engines(1)
        .with_pools(1)
        .with_cache_period("1s")
        .with_reconcile_period(Duration::from_secs(1), Duration::from_secs(1))
        .build()
        .await
        .unwrap();

    let vol_cli = cluster.grpc_client().volume();

    let volume = vol_cli
        .create(
            &CreateVolume {
                uuid: "1e3cf927-80c2-47a8-adf0-95c486bdd7b7".try_into().unwrap(),
                size: 60 * 1024 * 1024,
                replicas: 1,
                thin: false,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    assert!(!volume.spec().thin);
    assert!(!volume.spec().as_thin(), "Volume should not be thin!");

    let replica_snapshot = vol_cli
        .create_snapshot(
            &CreateVolumeSnapshot::new(volume.uuid(), SnapshotId::new()),
            None,
        )
        .await
        .unwrap();

    tracing::info!("Replica Snapshot: {replica_snapshot:?}");

    let volumes = vol_cli
        .get(Filter::Volume(volume.uuid().clone()), false, None, None)
        .await
        .unwrap();
    let volume = &volumes.entries[0];

    assert!(!volume.spec().thin);
    assert!(volume.spec().as_thin(), "Volume should be thin!");

    // Get snapshot by source id and snapshot id.
    let snaps = vol_cli
        .get_snapshots(
            Filter::VolumeSnapshot(
                volume.uuid().clone(),
                replica_snapshot.spec().snap_id.clone(),
            ),
            false,
            None,
            None,
        )
        .await
        .expect("Error while listing snapshot...");

    tracing::info!("List Snapshot by sourceid and snapid: {snaps:?}");

    // Get snapshot by snapshot id.
    let snaps = vol_cli
        .get_snapshots(
            Filter::Snapshot(replica_snapshot.spec().snap_id.clone()),
            false,
            None,
            None,
        )
        .await
        .expect("Error while listing snapshot...");

    tracing::info!("List Snapshot by snapid: {snaps:?}");
    assert!(!snaps.entries().is_empty());

    vol_cli
        .destroy_snapshot(&DestroyVolumeSnapshot::from(&replica_snapshot), None)
        .await
        .unwrap();

    tracing::info!("Deleted Snapshot: {}", replica_snapshot.spec().snap_id);

    assert!(!volume.spec().thin);
    assert!(volume.spec().as_thin(), "Volume should still be thin!");

    let volumes = vol_cli
        .get(Filter::Volume(volume.uuid().clone()), false, None, None)
        .await
        .unwrap();
    let volume = &volumes.entries[0];

    // Get snapshot by snapshot id. Shouldn't be found.
    vol_cli
        .get_snapshots(
            Filter::Snapshot(replica_snapshot.spec().snap_id.clone()),
            false,
            None,
            None,
        )
        .await
        .expect_err("Snapshot not found - expected");

    let volume = vol_cli
        .publish(
            &PublishVolume {
                uuid: volume.uuid().clone(),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    let nexus_snapshot = vol_cli
        .create_snapshot(
            &CreateVolumeSnapshot::new(volume.uuid(), SnapshotId::new()),
            None,
        )
        .await
        .unwrap();

    tracing::info!("Nexus Snapshot: {nexus_snapshot:?}");
    vol_cli
        .destroy_snapshot(&DestroyVolumeSnapshot::from(&nexus_snapshot), None)
        .await
        .unwrap();

    pool_destroy_validation(&cluster).await;
    thin_provisioning(&cluster, volume).await;
}

async fn thin_provisioning(cluster: &Cluster, volume: Volume) {
    let vol_cli = cluster.grpc_client().volume();
    let pool_cli = cluster.grpc_client().pool();

    let pools = pool_cli
        .get(Filter::Pool(cluster.pool(0, 0)), None)
        .await
        .unwrap();
    let pool = pools.0.first().unwrap();
    let pool_state = pool.state().unwrap();
    let watermark = 16 * 1024 * 1024;
    let free_space = pool_state.capacity - pool_state.used - watermark;

    // Take up the entire pool free space
    let repl_cli = cluster.grpc_client().replica();
    let mut req = CreateReplica {
        node: cluster.node(0),
        uuid: ReplicaId::new(),
        pool_id: cluster.pool(0, 0),
        size: free_space,
        thin: false,
        ..Default::default()
    };
    let replica = repl_cli.create(&req, None).await.unwrap();

    // Try to take a snapshot now, should fail!
    let error = vol_cli
        .create_snapshot(
            &CreateVolumeSnapshot::new(volume.uuid(), SnapshotId::new()),
            None,
        )
        .await
        .expect_err("No free space");
    assert_eq!(error.kind, ReplyErrorKind::ResourceExhausted);

    repl_cli
        .destroy(&DestroyReplica::from(replica), None)
        .await
        .unwrap();

    req.size = free_space / 2;
    let replica = repl_cli.create(&req, None).await.unwrap();

    let snapshot = vol_cli
        .create_snapshot(
            &CreateVolumeSnapshot::new(volume.uuid(), SnapshotId::new()),
            None,
        )
        .await
        .unwrap();

    repl_cli
        .destroy(&DestroyReplica::from(replica), None)
        .await
        .unwrap();
    vol_cli
        .destroy_snapshot(&DestroyVolumeSnapshot::from(&snapshot), None)
        .await
        .unwrap();

    // create 2 replicas of pool cap, exceeding pool commitment of 2.5x
    req.thin = true;
    req.size = pool_state.capacity - watermark;
    let _replica = repl_cli.create(&req, None).await.unwrap();

    // 2x the pool capacity is ok
    let _snapshot = vol_cli
        .create_snapshot(
            &CreateVolumeSnapshot::new(volume.uuid(), SnapshotId::new()),
            None,
        )
        .await
        .unwrap();

    req.uuid = ReplicaId::new();
    let _replica = repl_cli.create(&req, None).await.unwrap();

    let error = vol_cli
        .create_snapshot(
            &CreateVolumeSnapshot::new(volume.uuid(), SnapshotId::new()),
            None,
        )
        .await
        .expect_err("Pool over-committed");
    assert_eq!(error.kind, ReplyErrorKind::ResourceExhausted);
}

async fn pool_destroy_validation(cluster: &Cluster) {
    let vol_cli = cluster.grpc_client().volume();
    let pool_cli = cluster.grpc_client().pool();

    let volume = vol_cli
        .create(
            &CreateVolume {
                uuid: "0e3cf927-80c2-47a8-adf0-95c486bdd7b7".try_into().unwrap(),
                size: 5242880,
                replicas: 1,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    let replica_snapshot = vol_cli
        .create_snapshot(
            &CreateVolumeSnapshot::new(volume.uuid(), SnapshotId::new()),
            None,
        )
        .await
        .unwrap();

    vol_cli
        .destroy(&DestroyVolume::new(volume.uuid()), None)
        .await
        .unwrap();

    let destroy_pool = DestroyPool::new(cluster.node(0), cluster.pool(0, 0));
    let del_error = pool_cli
        .destroy(&destroy_pool, None)
        .await
        .expect_err("Snapshot Exists");
    assert_eq!(del_error.kind, ReplyErrorKind::InUse);

    vol_cli
        .destroy_snapshot(&DestroyVolumeSnapshot::from(&replica_snapshot), None)
        .await
        .unwrap();
}

#[tokio::test]
async fn snapshot_timeout() {
    fn grpc_timeout_opts(timeout: Duration) -> TimeoutOptions {
        TimeoutOptions::default()
            .with_max_retries(0)
            .with_req_timeout(timeout)
            .with_min_req_timeout(None)
    }
    let req_timeout = Duration::from_millis(800);
    let grpc_timeout = Duration::from_millis(500);

    let cluster = ClusterBuilder::builder()
        .with_rest(true)
        .with_io_engines(2)
        .with_tmpfs_pool_ix(1, 100 * 1024 * 1024)
        .with_cache_period("10s")
        .with_reconcile_period(Duration::from_secs(10), Duration::from_secs(10))
        .with_req_timeouts(req_timeout, req_timeout)
        .with_grpc_timeouts(grpc_timeout_opts(grpc_timeout))
        .build()
        .await
        .unwrap();

    let vol_cli = cluster.grpc_client().volume();
    let volume = vol_cli
        .create(
            &CreateVolume {
                uuid: "1e3cf927-80c2-47a8-adf0-95c486bdd7b7".try_into().unwrap(),
                size: 5242880,
                replicas: 1,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    let snapshot_id = SnapshotId::new();
    cluster.composer().pause(&cluster.node(1)).await.unwrap();
    let replica_snapshot = vol_cli
        .create_snapshot(
            &CreateVolumeSnapshot::new(volume.uuid(), snapshot_id.clone()),
            None,
        )
        .await
        .expect_err("timeout");
    tracing::info!("Replica Snapshot Error: {replica_snapshot:?}");
    cluster.composer().pause("core").await.unwrap();
    cluster.composer().thaw(&cluster.node(1)).await.unwrap();
    tokio::time::sleep(req_timeout - grpc_timeout).await;
    cluster.restart_core_with_liveness(None).await.unwrap();

    let snapshots = vol_cli
        .get_snapshots(Filter::Snapshot(snapshot_id.clone()), false, None, None)
        .await
        .unwrap();
    let snapshot = &snapshots.entries[0];

    assert_eq!(snapshot.meta().txn_id().as_str(), "1");
    assert_eq!(snapshot.meta().transactions().len(), 1);
    let tx = snapshot.meta().transactions().get("1").unwrap();
    assert_eq!(tx[0].status().to_string().as_str(), "Deleting");

    let grpc = cluster.grpc_client();
    wait_node_online(&grpc.node(), cluster.node(1))
        .await
        .unwrap();

    let snapshot = vol_cli
        .create_snapshot(&CreateVolumeSnapshot::new(volume.uuid(), snapshot_id), None)
        .await
        .unwrap();
    tracing::info!("Volume Snapshot: {snapshot:?}");

    assert_eq!(snapshot.meta().txn_id().as_str(), "2");
    assert_eq!(snapshot.meta().transactions().len(), 2);
    let tx = snapshot.meta().transactions().get("1").unwrap();
    assert_eq!(tx[0].status().to_string().as_str(), "Deleting");
    let tx = snapshot.meta().transactions().get("2").unwrap();
    assert_eq!(tx[0].status().to_string().as_str(), "Created");

    let grpc = cluster
        .new_grpc_client(grpc_timeout_opts(req_timeout))
        .await;
    let vol_cli_req = grpc.volume();
    let volume = vol_cli_req
        .publish(
            &PublishVolume {
                uuid: volume.uuid().clone(),
                target_node: Some(cluster.node(0)),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    let snapshot_id = SnapshotId::new();
    cluster.composer().pause(&cluster.node(0)).await.unwrap();
    let replica_snapshot = vol_cli
        .create_snapshot(
            &CreateVolumeSnapshot::new(volume.uuid(), snapshot_id.clone()),
            None,
        )
        .await
        .expect_err("timeout");
    tracing::info!("Replica Snapshot Error: {replica_snapshot:?}");
    tokio::time::sleep(req_timeout - grpc_timeout).await;

    cluster.composer().thaw(&cluster.node(0)).await.unwrap();
    wait_node_online(&grpc.node(), cluster.node(0))
        .await
        .unwrap();

    let snapshots = vol_cli
        .get_snapshots(Filter::Snapshot(snapshot_id.clone()), false, None, None)
        .await
        .unwrap();
    let snapshot = &snapshots.entries[0];
    tracing::info!("Snapshot: {snapshot:?}");

    assert_eq!(snapshot.meta().txn_id().as_str(), "1");
    assert!(snapshot.meta().transactions().is_empty());

    let snapshot = vol_cli
        .create_snapshot(&CreateVolumeSnapshot::new(volume.uuid(), snapshot_id), None)
        .await
        .unwrap();
    tracing::info!("Snapshot: {snapshot:?}");

    assert_eq!(snapshot.meta().txn_id().as_str(), "2");
    assert_eq!(snapshot.meta().transactions().len(), 1);
    let tx = snapshot.meta().transactions().get("2").unwrap();
    assert_eq!(tx[0].status().to_string().as_str(), "Created");
}

#[tokio::test]
async fn unknown_snapshot_garbage_collector() {
    let cluster = ClusterBuilder::builder()
        .with_rest(false)
        .with_io_engines(1)
        .with_pools(1)
        .with_cache_period("50ms")
        .with_reconcile_period(Duration::from_millis(50), Duration::from_millis(50))
        .build()
        .await
        .unwrap();

    let vol_cli = cluster.grpc_client().volume();
    let volume = vol_cli
        .create(
            &CreateVolume {
                uuid: "1e3cf927-80c2-47a8-adf0-95c486bdd7b7".try_into().unwrap(),
                size: 60 * 1024 * 1024,
                replicas: 1,
                thin: false,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    // Create two snapshot id's that we would use to create leaked resources.
    let parent_repl_snapid = SnapshotId::new();
    let parent_nexus_snapid = SnapshotId::new();

    // Create one replica snapshot.
    vol_cli
        .create_snapshot(
            &CreateVolumeSnapshot::new(volume.uuid(), parent_repl_snapid.clone()),
            None,
        )
        .await
        .unwrap();

    // Publish the volume to take nexus snapshot.
    let volume = vol_cli
        .publish(
            &PublishVolume {
                uuid: volume.uuid().clone(),
                target_node: Some(cluster.node(0)),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    // Create a nexus snapshot.
    vol_cli
        .create_snapshot(
            &CreateVolumeSnapshot::new(volume.uuid(), parent_nexus_snapid.clone()),
            None,
        )
        .await
        .unwrap();

    // Stop the core agent so that we can create the leaked resources.
    cluster.composer().stop("core").await.unwrap();

    // Extract the nexus and replica to be used to create leaked resources.
    let volume_state = volume.state();
    let replica = volume_state.replica_topology.iter().next().unwrap();
    let nexus = volume_state.target.unwrap();

    // Create the rpc handle.
    let mut rpc_handle = cluster
        .grpc_handle(&cluster.node(0))
        .await
        .expect("Should get handle");
    // At this point no leaked resources create so, snapshot count is 2.
    let snaps = rpc_handle
        .list_replica_snaps(None, None)
        .await
        .expect("Should get replica snaps");
    assert_eq!(snaps.snapshots.len(), 2);

    // Create the first leaked replica snapshot, with the existing replica snapshot's id.
    rpc_handle
        .create_replica_snap(
            &format!("{parent_repl_snapid}/322"),
            volume.spec().uuid.as_str(),
            "322",
            replica.0.as_str(),
            SnapshotId::new().as_str(),
        )
        .await
        .unwrap();
    let snaps = rpc_handle
        .list_replica_snaps(None, None)
        .await
        .expect("Should get replica snaps");
    assert_eq!(snaps.snapshots.len(), 3);

    // Create the first leaked replica snapshot, with the existing nexus snapshot's id.
    rpc_handle
        .create_nexus_snap(
            nexus.uuid.as_str(),
            &format!("{parent_nexus_snapid}/766"),
            volume.spec().uuid.as_str(),
            "766",
            replica.0.as_str(),
            SnapshotId::new().as_str(),
        )
        .await
        .unwrap();
    let snaps = rpc_handle
        .list_replica_snaps(None, None)
        .await
        .expect("Should get replica snaps");
    assert_eq!(snaps.snapshots.len(), 4);

    // Create the third leaked replica snapshot, with the snapshot id that no longer exists.
    rpc_handle
        .create_nexus_snap(
            nexus.uuid.as_str(),
            &format!("{}/54", SnapshotId::new()),
            volume.spec().uuid.as_str(),
            "54",
            replica.0.as_str(),
            SnapshotId::new().as_str(),
        )
        .await
        .unwrap();
    let snaps = rpc_handle
        .list_replica_snaps(None, None)
        .await
        .expect("Should get replica snaps");
    assert_eq!(snaps.snapshots.len(), 5);

    // Create the third leaked replica snapshot, with invalid name. This would not be garbage
    // collected.
    rpc_handle
        .create_nexus_snap(
            nexus.uuid.as_str(),
            "snap_54",
            volume.spec().uuid.as_str(),
            "54",
            replica.0.as_str(),
            SnapshotId::new().as_str(),
        )
        .await
        .unwrap();
    let snaps = rpc_handle
        .list_replica_snaps(None, None)
        .await
        .expect("Should get replica snaps");
    assert_eq!(snaps.snapshots.len(), 6);

    cluster
        .restart_core_with_liveness(None)
        .await
        .expect("Core agent should have been up");

    let snaps = rpc_handle
        .list_replica_snaps(None, None)
        .await
        .expect("Should get replica snaps");
    assert_eq!(snaps.snapshots.len(), 3);
}

#[tokio::test]
async fn snapshot_upgrade() {
    let mb = 1024 * 1024;
    let gc_period = Duration::from_millis(200);
    let cluster = ClusterBuilder::builder()
        .with_rest(true)
        .with_tmpfs_pool_ix(0, 100 * mb)
        .with_tmpfs_pool_ix(1, 100 * mb)
        .with_cache_period("200ms")
        .with_reconcile_period(gc_period, gc_period)
        .with_options(|b| {
            b.with_io_engines(2)
                .with_idle_io_engines(2)
                .with_io_engine_tag("v2.6.1")
        })
        .build()
        .await
        .unwrap();

    let vol_cli = cluster.grpc_client().volume();

    let volume_1 = vol_cli
        .create(
            &CreateVolume {
                uuid: "1e3cf927-80c2-47a8-adf0-95c486bdd7b7".try_into().unwrap(),
                size: 16 * mb,
                replicas: 1,
                thin: false,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    let volume_2 = vol_cli
        .create(
            &CreateVolume {
                uuid: "1e3cf927-80c2-47a8-adf0-95c486bdd7b8".try_into().unwrap(),
                size: 16 * mb,
                replicas: 2,
                thin: false,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    let volume_1 = vol_cli
        .publish(
            &PublishVolume {
                uuid: volume_1.uuid().clone(),
                share: Some(VolumeShareProtocol::Nvmf),
                target_node: Some(cluster.node(0)),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    let _snapshot = vol_cli
        .create_snapshot(
            &CreateVolumeSnapshot::new(volume_1.uuid(), SnapshotId::new()),
            None,
        )
        .await
        .unwrap();
    let error = vol_cli
        .create_snapshot(
            &CreateVolumeSnapshot::new(volume_2.uuid(), SnapshotId::new()),
            None,
        )
        .await
        .expect_err("Can't take nr snapshot");
    assert_eq!(error.kind, ReplyErrorKind::FailedPrecondition);

    let error = vol_cli
        .set_replica(
            &SetVolumeReplica {
                uuid: volume_1.uuid().clone(),
                replicas: 2,
            },
            None,
        )
        .await
        .expect_err("No rebuild on older version!");
    assert_eq!(error.kind, ReplyErrorKind::FailedPrecondition);

    cluster
        .replace_node(cluster.node(0), cluster.node(2))
        .await
        .unwrap();
    cluster.wait_pool_online(cluster.pool(0, 0)).await.unwrap();
    cluster
        .replace_node(cluster.node(1), cluster.node(3))
        .await
        .unwrap();
    cluster.wait_pool_online(cluster.pool(1, 0)).await.unwrap();

    tokio::time::sleep(gc_period * 5).await;

    let _volume = vol_cli
        .set_replica(
            &SetVolumeReplica {
                uuid: volume_1.uuid().clone(),
                replicas: 2,
            },
            None,
        )
        .await
        .expect("After upgrade this should work!");

    vol_cli
        .create_snapshot(
            &CreateVolumeSnapshot::new(volume_2.uuid(), SnapshotId::new()),
            None,
        )
        .await
        .expect("Now we can take nr snapshot");
}

#[tokio::test]
#[ignore]
async fn nr_snapshot() {
    let kb = 1024 * 1024;
    let gb = kb * 1024;
    let cluster = ClusterBuilder::builder()
        .with_rest(true)
        .with_io_engines(3)
        .with_csi(true, true)
        .with_tmpfs_pool_ix(1, gb)
        .with_tmpfs_pool_ix(2, gb)
        .with_cache_period("1s")
        .with_reconcile_period(Duration::from_secs(1), Duration::from_secs(1))
        .with_options(|b| {
            b.with_isolated_io_engine(true)
                .with_agents_env("DISABLE_TARGET_ACC", "true")
        })
        .build()
        .await
        .unwrap();

    let vol_cli = cluster.grpc_client().volume();

    let volume = vol_cli
        .create(
            &CreateVolume {
                uuid: "1e3cf927-80c2-47a8-adf0-95c486bdd7b7".try_into().unwrap(),
                size: 100 * kb,
                replicas: 1,
                thin: false,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    assert!(!volume.spec().thin);
    assert!(!volume.spec().as_thin(), "Volume should not be thin!");

    let volume = vol_cli
        .publish(
            &PublishVolume {
                uuid: volume.uuid().clone(),
                share: Some(VolumeShareProtocol::Nvmf),
                target_node: Some(cluster.node(0)),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    let api = cluster.rest_v00();
    let rest_cli = api.volumes_api();
    let volume_grpc = volume;
    let volume = rest_cli.get_volume(volume_grpc.uuid()).await.unwrap();

    tracing::info!("Taking Snapshot");
    let snapshot = vol_cli
        .create_snapshot(
            &CreateVolumeSnapshot::new(volume_grpc.uuid(), SnapshotId::new()),
            None,
        )
        .await
        .unwrap();
    tracing::info!("Done");

    let (s, r) = tokio::sync::oneshot::channel::<()>();
    let task = run_fio_vol(&cluster, volume.clone(), r).await;
    drop(s);

    tracing::info!("Setting Replica Count");
    vol_cli
        .set_replica(
            &SetVolumeReplica {
                uuid: volume_grpc.uuid().clone(),
                replicas: 2,
            },
            None,
        )
        .await
        .unwrap();

    task.await.ok();
    tracing::info!("FIO COMPLETE");

    let volumes = loop {
        let volumes = vol_cli
            .get(
                Filter::Volume(volume_grpc.uuid().clone()),
                false,
                None,
                None,
            )
            .await
            .unwrap();
        let volume = &volumes.entries[0];
        if volume.state().status == VolumeStatus::Online {
            break volumes;
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    };

    let volume = &volumes.entries[0];

    tracing::info!("Volume Status: {}", volume.state().status);

    let mut cksums = vec![];

    let mut r1 = ReplicaId::new();
    let mut r2 = ReplicaId::new();
    for (id, topology) in volume.state().replica_topology {
        let node_id = topology.node().as_ref().unwrap();

        if node_id == &cluster.node(1) {
            r1 = id.clone();
        } else if node_id == &cluster.node(2) {
            r2 = id.clone();
        }

        let mut node = cluster.grpc_handle(node_id.as_str()).await.unwrap();
        let cksum = node.checksum(id.as_str()).await.unwrap();
        cksums.push(cksum);
    }
    tracing::info!(
        "\nsudo nvme connect -t tcp -s 8420 -a 10.1.0.7 -n nqn.2019-05.io.openebs:{r1}; \
    sudo nvme connect -t tcp -s 8420 -a 10.1.0.8 -n nqn.2019-05.io.openebs:{r2}"
    );

    tracing::info!("CheckSums: {cksums:#?}");
    assert_eq!(cksums.len(), 2, "Expected 2 replicas");
    assert_eq!(cksums[0], cksums[1], "CheckSum Mismatch!! {:?}", cksums);
    tokio::time::sleep(Duration::from_secs(2)).await;

    let snap_cli = api.snapshots_api();
    snap_cli
        .del_volume_snapshot(volume_grpc.uuid(), snapshot.state().uuid())
        .await
        .unwrap();
    let v_api = api.volumes_api();
    v_api.del_volume(volume_grpc.uuid()).await.unwrap();
}

async fn run_fio_vol(
    cluster: &Cluster,
    volume: models::Volume,
    stop: tokio::sync::oneshot::Receiver<()>,
) -> tokio::task::JoinHandle<Option<i64>> {
    run_fio_vol_verify(cluster, volume, stop).await
}
async fn run_fio_vol_verify(
    cluster: &Cluster,
    volume: models::Volume,
    mut stop: tokio::sync::oneshot::Receiver<()>,
) -> tokio::task::JoinHandle<Option<i64>> {
    let fio_builder = |device: &str| {
        let filename = format!("--filename={device}");
        vec![
            "fio",
            "--direct=1",
            "--ioengine=libaio",
            "--bs=4k",
            "--iodepth=16",
            "--loops=1",
            "--numjobs=1",
            "--name=fio",
            "--readwrite=randwrite",
            filename.as_str(),
        ]
        .into_iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
    };

    let mut node = cluster.csi_node_client(0).await.unwrap();
    node.node_stage_volume(&volume, HashMap::new())
        .await
        .unwrap();

    let response = node
        .internal()
        .find_volume(FindVolumeRequest {
            volume_id: volume.spec.uuid.to_string(),
        })
        .await
        .unwrap();

    let device_path = response.into_inner().device_path;
    let device_path = device_path.trim_end();
    let fio_cmd = fio_builder(device_path);
    let fio_cmdline = fio_cmd
        .iter()
        .fold(String::new(), |acc, next| format!("{acc} {next}"));
    let composer = cluster.composer().clone();

    tokio::spawn(async move {
        use tokio::sync::oneshot::error::TryRecvError;
        let code = loop {
            let (code, out) = composer.exec("csi-node-1", fio_cmd.clone()).await.unwrap();
            tracing::info!("{}: {}, code: {:?}", fio_cmdline, out, code);
            if code != Some(0) {
                return code;
            }
            assert_eq!(code, Some(0));

            if stop.try_recv().is_ok() || matches!(stop.try_recv(), Err(TryRecvError::Closed)) {
                break code;
            }
        };

        node.node_unstage_volume(&volume).await.unwrap();
        code
    })
}
