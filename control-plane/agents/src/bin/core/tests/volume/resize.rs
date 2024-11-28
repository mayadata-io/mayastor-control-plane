use deployer_cluster::{Cluster, ClusterBuilder};
use grpc::operations::{
    nexus::traits::NexusOperations, replica::traits::ReplicaOperations,
    volume::traits::VolumeOperations,
};
use std::time::Duration;
use stor_port::{
    transport_api::{ReplyError, ReplyErrorKind},
    types::v0::transport::{
        CreateVolume, DestroyVolume, Filter, PublishVolume, ReplicaId, ResizeReplica, ResizeVolume,
        Volume, VolumeId, VolumeShareProtocol,
    },
};

use uuid::Uuid;

const SIZE: u64 = 50 * 1024 * 1024; // 50MiB
const EXPANDED_SIZE: u64 = 2 * SIZE; // 100MiB
const CAPACITY_LIMIT_DIFF: u64 = 20 * 1024 * 1024; // 20MiB

/// Validate that the size of volume and replicas are as per expected_size
/// Return true if validation is successful, otherwise false.
async fn validate_resized_volume(
    cluster: &Cluster,
    uuid: &VolumeId,
    resized_volume: &Volume,
    expected_size: u64,
    published: bool,
) {
    let nex_cli = cluster.grpc_client().nexus();
    let repl_cli = cluster.grpc_client().replica();
    assert!(resized_volume.spec().uuid == *uuid);
    assert!(resized_volume.spec().size == expected_size);

    let replicas = repl_cli
        .get(Filter::Volume(uuid.clone()), None)
        .await
        .unwrap();
    // Compare >= since replicas have some additional book-keeping space.
    replicas
        .into_inner()
        .iter()
        .for_each(|r| assert!(r.size >= resized_volume.spec().size));

    if published {
        let nexus = nex_cli
            .get(Filter::Volume(uuid.clone()), None)
            .await
            .unwrap()
            .0;
        assert!(nexus.len() == 1);
        assert!(nexus[0].size == expected_size);
        tracing::info!("Validated Resized Nexus: {:?}", nexus[0]);
    }
}

#[tokio::test]
async fn resize_unpublished_and_published() {
    let cluster = ClusterBuilder::builder()
        .with_rest(false)
        .with_io_engines(3)
        .with_pool(0, "malloc:///p1?size_mb=200")
        .with_pool(1, "malloc:///p1?size_mb=200")
        .with_pool(2, "malloc:///p1?size_mb=200")
        .with_cache_period("1s")
        .with_reconcile_period(Duration::from_secs(1), Duration::from_secs(1))
        .with_options(|o| o.with_isolated_io_engine(true))
        .build()
        .await
        .unwrap();

    let vol_cli = cluster.grpc_client().volume();

    // Create the volume.
    let volume_size_orig = 50 * 1024 * 1024;
    let volume = vol_cli
        .create(
            &CreateVolume {
                uuid: "de3cf927-80c2-47a8-adf0-95c486bdd7b7".try_into().unwrap(),
                size: volume_size_orig,
                replicas: 3,
                thin: false,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    // Unpublished volume
    assert!(volume.spec().active_config().is_none() && volume.spec().num_replicas == 3);

    let resized_volume = vol_cli
        .resize(
            &ResizeVolume {
                uuid: volume.uuid().clone(),
                requested_size: EXPANDED_SIZE,
                cluster_capacity_limit: None,
            },
            None,
        )
        .await
        .unwrap();

    tracing::info!("Resized Unpublished {resized_volume:?}");
    validate_resized_volume(
        &cluster,
        &volume.spec().uuid,
        &resized_volume,
        2 * volume_size_orig,
        false,
    )
    .await;

    let _ = vol_cli
        .destroy(
            &DestroyVolume {
                uuid: volume.uuid().clone(),
            },
            None,
        )
        .await;

    // Test resizing a published volume.
    resize_published(&cluster).await;
}

// Resizing a published volume should throw error that volume is in-use.
async fn resize_published(cluster: &Cluster) {
    let vol_cli = cluster.grpc_client().volume();
    // Create and publish the volume.
    let volume_size_orig = 50 * 1024 * 1024;
    let volume = vol_cli
        .create(
            &CreateVolume {
                uuid: "df3cf927-80c2-47a8-adf0-95c486bdd7b7".try_into().unwrap(),
                size: SIZE,
                replicas: 1,
                thin: false,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    vol_cli
        .publish(
            &PublishVolume {
                uuid: volume.spec().uuid,
                target_node: Some(cluster.node(0)),
                share: Some(VolumeShareProtocol::Nvmf),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    let resized_volume = vol_cli
        .resize(
            &ResizeVolume {
                uuid: volume.uuid().clone(),
                requested_size: EXPANDED_SIZE,
                cluster_capacity_limit: None,
            },
            None,
        )
        .await
        .unwrap();

    tracing::info!("Resized Published {resized_volume:?}");
    validate_resized_volume(
        cluster,
        &volume.spec().uuid,
        &resized_volume,
        2 * volume_size_orig,
        true,
    )
    .await;
}

// Try to resize a volume. When any one of the replica can't be resized due to
// insufficient capacity on  pool, the volume resize should fail and volume size
// should remain unchanged.
#[tokio::test]
async fn resize_on_no_capacity_pool() {
    let cluster = ClusterBuilder::builder()
        .with_rest(false)
        .with_io_engines(3)
        .with_pool(0, "malloc:///p1?size_mb=200")
        .with_pool(1, "malloc:///p1?size_mb=200")
        .with_pool(2, "malloc:///p1?size_mb=100")
        .with_cache_period("1s")
        .with_reconcile_period(Duration::from_secs(1), Duration::from_secs(1))
        .with_options(|o| o.with_isolated_io_engine(true))
        .build()
        .await
        .unwrap();

    let vol_cli = cluster.grpc_client().volume();

    let volume = vol_cli
        .create(
            &CreateVolume {
                uuid: "de3cf927-80c2-47a8-adf0-95c486bdd7b7".try_into().unwrap(),
                size: SIZE,
                replicas: 3,
                thin: false,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    let resized_volume = vol_cli
        .resize(
            &ResizeVolume {
                uuid: volume.uuid().clone(),
                requested_size: EXPANDED_SIZE,
                cluster_capacity_limit: None,
            },
            None,
        )
        .await
        .expect_err("Expected error due to insufficient pool capacity");

    tracing::info!("Volume resize error: {resized_volume:?}");
    let v_arr = vol_cli
        .get(Filter::Volume(volume.spec().uuid), false, None, None)
        .await
        .unwrap();
    let vol_obj = &v_arr.entries[0];
    // Size shouldn't have changed.
    assert!(vol_obj.spec().size == volume.spec().size);

    // try a resize again, this time setting cluster capacity limit.
    let _ = vol_cli
        .resize(
            &ResizeVolume {
                uuid: volume.uuid().clone(),
                requested_size: EXPANDED_SIZE,
                cluster_capacity_limit: Some(EXPANDED_SIZE + CAPACITY_LIMIT_DIFF),
            },
            None,
        )
        .await
        .expect_err("Expected error due to insufficient pool capacity");

    let v_arr = vol_cli
        .get(Filter::Volume(volume.spec().uuid), false, None, None)
        .await
        .unwrap();
    let vol_obj = &v_arr.entries[0];
    // Size shouldn't have changed.
    assert!(vol_obj.spec().size == volume.spec().size);

    // TODO: Add reclaim monitor validations for replicas that got resized as part
    // of this failed volume resize.
}

#[tokio::test]
async fn resize_with_cluster_capacity_limit() {
    let cluster = ClusterBuilder::builder()
        .with_rest(false)
        .with_io_engines(2)
        .with_pool(0, "malloc:///p1?size_mb=200")
        .with_pool(1, "malloc:///p1?size_mb=200")
        .with_cache_period("1s")
        .with_reconcile_period(Duration::from_secs(1), Duration::from_secs(1))
        .with_options(|o| o.with_isolated_io_engine(true))
        .build()
        .await
        .unwrap();

    let vol_cli = cluster.grpc_client().volume();

    // resize(shrink) within a valid capacity limit
    grpc_resize_volume_with_limit(
        &vol_cli,
        Some(EXPANDED_SIZE),
        Some(ReplyErrorKind::NotAcceptable {}),
        false,
    )
    .await;
    // resize(expand) exceeding the capacity limit
    grpc_resize_volume_with_limit(
        &vol_cli,
        Some(EXPANDED_SIZE - CAPACITY_LIMIT_DIFF),
        Some(ReplyErrorKind::CapacityLimitExceeded {}),
        true,
    )
    .await;

    // resize(expand) within the capacity limit
    grpc_resize_volume_with_limit(
        &vol_cli,
        Some(EXPANDED_SIZE + CAPACITY_LIMIT_DIFF),
        None,
        true,
    )
    .await;
    // resize a new volume, but reduce the limit set previously. The limit balance
    // calculations are expected to work based on reduced limit value now.
    grpc_resize_volume_with_limit(
        &vol_cli,
        Some(EXPANDED_SIZE + CAPACITY_LIMIT_DIFF / 2),
        None,
        true,
    )
    .await;
}

// Take 600MiB pool. Create five volumes of 50MiB each, totalling a usage of 250MiB.
// Set cluster capacity limit to 400MiB and expand all five volumes to 100MiB.
// Since remaining limit is 150MiB, three volumes should successfully expand and
// two must fail to expand.
#[tokio::test]
async fn resize_with_cluster_capacity_limit_concurrent() {
    let cluster = ClusterBuilder::builder()
        .with_rest(false)
        .with_io_engines(2)
        .with_pool(0, "malloc:///p1?size_mb=600")
        .with_cache_period("1s")
        .with_reconcile_period(Duration::from_secs(1), Duration::from_secs(1))
        .with_options(|o| o.with_isolated_io_engine(true))
        .build()
        .await
        .unwrap();

    let num_volumes = 5;
    let mut success = 0;
    let mut failure = 0;
    let vol_cli = cluster.grpc_client().volume();
    let volume_ids = create_volumes(&vol_cli, 5).await;
    let mut results = Vec::with_capacity(num_volumes);

    // Create a channel to collect results from the concurrent tasks
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<Volume, ReplyError>>(num_volumes);
    // For each task
    let refc_vol_cli = std::sync::Arc::new(vol_cli);

    for volume_id in volume_ids {
        let refp_vol_cli = std::sync::Arc::clone(&refc_vol_cli);
        let tx = tx.clone();
        tokio::spawn(async move {
            let hdl = refp_vol_cli
                .resize(
                    &ResizeVolume {
                        uuid: volume_id.into(),
                        requested_size: EXPANDED_SIZE,
                        cluster_capacity_limit: Some(4 * EXPANDED_SIZE),
                    },
                    None,
                )
                .await;

            // Send the result to the channel
            tx.send(hdl).await.unwrap();
        });
    }
    // Collect results from the channel
    for _ in 0..num_volumes {
        results.push(rx.recv().await.unwrap());
    }

    results.iter().for_each(|r| {
        if r.is_ok() {
            success += 1;
        } else {
            failure += 1;
        }
    });
    assert_eq!(success, 3);
    assert_eq!(failure, 2);
}

async fn grpc_resize_volume_with_limit(
    volume_client: &dyn VolumeOperations,
    capacity: Option<u64>,
    expected_error: Option<ReplyErrorKind>,
    expand: bool,
) {
    let vol_uuid = Uuid::new_v4();
    let new_size = match expand {
        true => EXPANDED_SIZE,
        false => SIZE / 2,
    };

    let volume = volume_client
        .create(
            &CreateVolume {
                uuid: vol_uuid.into(),
                size: SIZE,
                replicas: 2,
                thin: false,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    let result = volume_client
        .resize(
            &ResizeVolume {
                uuid: volume.uuid().clone(),
                requested_size: new_size,
                cluster_capacity_limit: capacity,
            },
            None,
        )
        .await;

    match result {
        Ok(resized_volume) => {
            assert!(resized_volume.spec().uuid == volume.spec().uuid);
            assert!(resized_volume.spec().size == EXPANDED_SIZE);
            volume_client
                .destroy(
                    &DestroyVolume {
                        uuid: resized_volume.uuid().into(),
                    },
                    None,
                )
                .await
                .unwrap();
            assert!(expected_error.is_none());
        }
        Err(e) => {
            assert_eq!(expected_error, Some(e.kind)); // wrong error
                                                      // Volume not needed anymore.
            volume_client
                .destroy(
                    &DestroyVolume {
                        uuid: volume.uuid().into(),
                    },
                    None,
                )
                .await
                .unwrap();
        }
    }
}

// Creates count number of volumes, and return the uuid of volume to be resized.
async fn create_volumes(volume_client: &dyn VolumeOperations, count: u64) -> Vec<Uuid> {
    let mut volumes = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let vol_uuid = Uuid::new_v4();
        let volume = volume_client
            .create(
                &CreateVolume {
                    uuid: vol_uuid.into(),
                    size: SIZE,
                    replicas: 1,
                    thin: false,
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();

        volumes.push(volume.uuid().into())
    }

    volumes
}

// Scenario - One of the replica of the volume is bigger than the volume spec, indicating
// a volume expand partially failed. The expanded replica's space should get reclaimed.
// If the volume has a target, then for the above scenario, the expanded replica size
// must not be reclaimed.
#[tokio::test]
async fn resize_replica_space_reclaim() {
    let reconcile_period = Duration::from_secs(1);
    let vol_uuid = Uuid::new_v4();
    let cluster = ClusterBuilder::builder()
        .with_rest(false)
        .with_io_engines(2)
        .with_pool(0, "malloc:///p1?size_mb=200")
        .with_pool(1, "malloc:///p1?size_mb=200")
        .with_cache_period("500ms")
        .with_reconcile_period(reconcile_period, reconcile_period)
        .with_options(|o| o.with_isolated_io_engine(true))
        .build()
        .await
        .unwrap();

    let vol_cli = cluster.grpc_client().volume();
    let repl_cli = cluster.grpc_client().replica();

    let volume = vol_cli
        .create(
            &CreateVolume {
                uuid: vol_uuid.into(),
                size: SIZE,
                replicas: 2,
                thin: false,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    let replicas = repl_cli
        .get(Filter::Volume(volume.uuid().clone()), None)
        .await
        .unwrap();
    assert_eq!(replicas.0.len(), 2);

    // Let's pick a replica for resizing
    let replica = replicas.0.first().unwrap();
    let repl_orig_size = replica.size;
    let _ = repl_cli
        .resize(
            &ResizeReplica {
                node: replica.node.clone(),
                pool_id: replica.pool_id.clone(),
                uuid: replica.uuid.clone(),
                name: None,
                requested_size: EXPANDED_SIZE,
            },
            None,
        )
        .await
        .unwrap();

    // Expanded replica space must get reclaimed by reconciler
    wait_expanded_replica_space_reclaimed(
        &cluster,
        &replica.uuid,
        repl_orig_size,
        reconcile_period * 5,
        true,
    )
    .await;

    // Publish the volume now
    vol_cli
        .publish(
            &PublishVolume {
                uuid: volume.spec().uuid,
                target_node: Some(cluster.node(0)),
                share: Some(VolumeShareProtocol::Nvmf),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    // Expand the replica again
    let _ = repl_cli
        .resize(
            &ResizeReplica {
                node: replica.node.clone(),
                pool_id: replica.pool_id.clone(),
                uuid: replica.uuid.clone(),
                name: None,
                requested_size: EXPANDED_SIZE,
            },
            None,
        )
        .await
        .unwrap();

    // Expanded replica space must NOT get reclaimed by reconciler
    // because a volume target/nexus is found
    wait_expanded_replica_space_reclaimed(
        &cluster,
        &replica.uuid,
        repl_orig_size,
        reconcile_period * 5,
        false,
    )
    .await;

    async fn wait_expanded_replica_space_reclaimed(
        cluster: &Cluster,
        replica: &ReplicaId,
        repl_orig_size: u64,
        timeout: Duration,
        expect_reclaim: bool,
    ) {
        let repl_client = cluster.grpc_client().replica();
        let start = std::time::Instant::now();
        loop {
            let replica = &repl_client
                .get(Filter::Replica(replica.clone()), None)
                .await
                .unwrap()
                .0[0];

            if expect_reclaim && (replica.size == repl_orig_size) {
                // Size should get reverted back to original
                break;
            }
            if std::time::Instant::now() > (start + timeout) {
                if expect_reclaim {
                    panic!("{:?} - Timeout while waiting to reconcile expanded replica size. Replica '{replica:#?}'", chrono::Utc::now());
                }
                // !expect_reclaim - replica size must not have been reclaimed to original.
                assert!(replica.size != repl_orig_size);
                break;
            }

            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}
