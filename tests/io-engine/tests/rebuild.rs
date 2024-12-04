use deployer_cluster::{Cluster, ClusterBuilder, FindVolumeRequest};
use grpc::operations::nexus::traits::NexusOperations;
use openapi::{
    apis::{volumes_api, volumes_api::tower::client::direct::Volumes, Url, Uuid},
    models,
    models::PublishVolumeBody,
};
use stor_port::types::v0::transport::{ChildState, Filter};

use std::{collections::HashMap, time::Duration};
use tokio::sync::oneshot::error::TryRecvError;

#[tokio::test]
async fn concurrent_rebuilds() {
    let nr_volumes = 20u32;
    let io_engines = 2u32;
    let gig = 1024 * 1024 * 1024u64;
    let pool_size_bytes = (nr_volumes as u64) * (io_engines as u64 + 1) * gig;
    let rebuild_loops = 1;
    let mut replica_count = 1;

    let cluster = ClusterBuilder::builder()
        .with_options(|o| {
            o.with_io_engines(io_engines)
                .with_isolated_io_engine(true)
                .with_io_engine_env("NVME_QPAIR_CONNECT_ASYNC", "true")
                .with_max_rebuilds(Some(nr_volumes))
        })
        .with_cache_period("10s")
        .with_tmpfs_pool(pool_size_bytes)
        .build()
        .await
        .unwrap();

    let cli = cluster.rest_v00();
    let vol_cli = cli.volumes_api();

    let mut volumes = vec![];
    for i in 0 .. nr_volumes {
        let i = i % io_engines;
        #[allow(clippy::identity_op)]
        let volume = vol_cli
            .put_volume(
                &Uuid::new_v4(),
                models::CreateVolumeBody::new(
                    models::VolumePolicy::new(true),
                    replica_count,
                    1 * gig,
                    false,
                ),
            )
            .await
            .unwrap();
        let volume = vol_cli
            .put_volume_target(
                &volume.spec.uuid,
                PublishVolumeBody::new_all(
                    HashMap::new(),
                    None,
                    Some(cluster.node(i).to_string()),
                    models::VolumeShareProtocol::Nvmf,
                    None,
                    "".to_string(),
                ),
            )
            .await
            .unwrap();

        volumes.push(volume);
    }

    for _ in 0 .. rebuild_loops {
        match replica_count {
            1 => {
                for volume in &volumes {
                    vol_cli
                        .put_volume_replica_count(&volume.spec.uuid, 2)
                        .await
                        .unwrap();
                }
            }
            // reduce and increase the number of replicas again in order to trigger anothe rebuild
            2 => {
                for volume in &volumes {
                    vol_cli
                        .put_volume_replica_count(&volume.spec.uuid, 1)
                        .await
                        .unwrap();
                }
                for volume in &volumes {
                    vol_cli
                        .put_volume_replica_count(&volume.spec.uuid, 2)
                        .await
                        .unwrap();
                }
            }
            _ => {
                unimplemented!()
            }
        };
        replica_count = 2;

        check_volumes(&cluster, vol_cli, &volumes).await;
    }

    async fn check_volumes(
        cluster: &Cluster,
        vol_cli: &dyn volumes_api::tower::client::direct::Volumes,
        volumes: &[models::Volume],
    ) {
        let start = std::time::Instant::now();
        let mut timeout = std::time::Duration::from_secs(120);
        let timeout_slack = timeout;
        let mut added_slack = false;
        let check_interval = std::time::Duration::from_secs(5);
        loop {
            let curr_volumes = vol_cli.get_volumes(0, None, None).await.unwrap().entries;
            assert_eq!(volumes.len(), curr_volumes.len());
            // volumes should either be online or degraded (while rebuilding)
            let not_expected = curr_volumes
                .iter()
                .filter(|v| {
                    matches!(
                        v.state.status,
                        models::VolumeStatus::Unknown | models::VolumeStatus::Faulted
                    )
                })
                .collect::<Vec<_>>();
            assert!(not_expected.is_empty());

            let online = curr_volumes
                .iter()
                .filter(|v| matches!(v.state.status, models::VolumeStatus::Online))
                .collect::<Vec<_>>();

            tracing::info!(
                "{} out of {} volumes have been rebuilt...",
                online.len(),
                volumes.len()
            );

            if online.len() == volumes.len() {
                break;
            }

            if start.elapsed() >= timeout {
                // if the nodes are still responsive allow for a bit more slack if the CI
                // performance is slow.
                let mut nodes_ok = true;
                let nodes = cluster
                    .rest_v00()
                    .nodes_api()
                    .get_nodes(None)
                    .await
                    .unwrap();
                for node in nodes {
                    if let Ok(mut handle) = cluster.grpc_handle(&node.id).await {
                        if handle.ping().await.is_err() {
                            nodes_ok = false;
                            break;
                        }
                    }
                }
                if nodes_ok {
                    tracing::warn!("Timeout has been hit but the nodes are not locked up");
                    if !added_slack {
                        tracing::warn!("Increasing the timeout slack just once since the nodes are not locked up");
                        added_slack = true;
                        timeout += timeout_slack;
                    }
                }
            }

            if start.elapsed() >= timeout {
                assert_eq!(
                    online.len(),
                    volumes.len(),
                    "Volumes not online within {timeout:?}"
                );
            }

            tokio::time::sleep(check_interval).await;
        }
    }
}

#[tokio::test]
async fn loopback_nvmf() {
    let nexus_size = 10 * 1024 * 1024u64;
    let repl_size = nexus_size * 2;
    let repl_size_mb = repl_size / (1024 * 1024);

    let cluster = ClusterBuilder::builder()
        .with_io_engines(1)
        .with_pools(1)
        .with_options(|o| o.with_io_engine_env("NVME_QPAIR_CONNECT_ASYNC", "true"))
        .build()
        .await
        .unwrap();

    let cli = cluster.rest_v00();
    let repl_api = cli.replicas_api();

    let replica = repl_api
        .put_node_pool_replica(
            cluster.node(0).as_str(),
            cluster.pool(0, 0).as_str(),
            &Uuid::new_v4(),
            models::CreateReplicaBody::new(repl_size, false),
        )
        .await
        .unwrap();

    let share_uri = repl_api
        .put_node_pool_replica_share(
            cluster.node(0).as_str(),
            cluster.pool(0, 0).as_str(),
            &replica.uuid,
            None,
        )
        .await
        .unwrap();

    let share_url: Url = share_uri.parse().unwrap();
    assert_eq!(share_url.scheme(), "nvmf");

    let nx_api = cli.nexuses_api();
    let malloc_uri =
        format!("malloc:///ch?size_mb={repl_size_mb}&uuid=68e5fcf2-276a-4b29-8fb6-90bfa841297a");
    let nexus = nx_api
        .put_node_nexus(
            cluster.node(0).as_str(),
            &Uuid::new_v4(),
            models::CreateNexusBody::new(vec![share_uri], nexus_size),
        )
        .await
        .unwrap();

    let ch_api = cli.children_api();

    ch_api
        .put_nexus_child(&nexus.uuid, malloc_uri.as_str())
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn repeated_rebuilds() {
    let nexus_size = 10 * 1024 * 1024u64;
    let repl_size = nexus_size * 2;

    let cluster = ClusterBuilder::builder()
        .with_io_engines(3)
        .with_pool(1, "malloc:///pool?size_mb=100")
        .with_pool(2, "malloc:///pool?size_mb=100")
        .build()
        .await
        .unwrap();

    let cli = cluster.rest_v00();
    let repl_api = cli.replicas_api();

    let replica1 = repl_api
        .put_node_pool_replica(
            cluster.node(1).as_str(),
            cluster.pool(1, 0).as_str(),
            &Uuid::new_v4(),
            models::CreateReplicaBody::new(repl_size, false),
        )
        .await
        .unwrap();
    let share_uri1 = repl_api
        .put_node_pool_replica_share(&replica1.node, &replica1.pool, &replica1.uuid, None)
        .await
        .unwrap();
    let replica2 = repl_api
        .put_node_pool_replica(
            cluster.node(2).as_str(),
            cluster.pool(2, 0).as_str(),
            &Uuid::new_v4(),
            models::CreateReplicaBody::new(repl_size, false),
        )
        .await
        .unwrap();
    let share_uri2 = repl_api
        .put_node_pool_replica_share(&replica2.node, &replica2.pool, &replica2.uuid, None)
        .await
        .unwrap();
    let replica3 = repl_api
        .put_node_pool_replica(
            cluster.node(1).as_str(),
            cluster.pool(1, 0).as_str(),
            &Uuid::new_v4(),
            models::CreateReplicaBody::new(repl_size, false),
        )
        .await
        .unwrap();
    let share_uri3 = repl_api
        .put_node_pool_replica_share(&replica3.node, &replica3.pool, &replica3.uuid, None)
        .await
        .unwrap();

    loop {
        let nx_api = cli.nexuses_api();
        let nexus = nx_api
            .put_node_nexus(
                cluster.node(0).as_str(),
                &Uuid::new_v4(),
                models::CreateNexusBody::new(vec![share_uri1.clone()], nexus_size),
            )
            .await
            .unwrap();

        let ch_api = cli.children_api();

        let mut count = 0;
        loop {
            tracing::info!("Adding {}", share_uri2);
            ch_api
                .put_nexus_child(&nexus.uuid, share_uri2.as_str())
                .await
                .unwrap();
            tracing::info!("Adding {}", share_uri3);
            ch_api
                .put_nexus_child(&nexus.uuid, share_uri3.as_str())
                .await
                .unwrap();
            tracing::info!("Removing {}", share_uri2);
            ch_api
                .del_nexus_child(&nexus.uuid, share_uri2.as_str())
                .await
                .unwrap();
            tracing::info!("Removing {}", share_uri3);
            ch_api
                .del_nexus_child(&nexus.uuid, share_uri3.as_str())
                .await
                .unwrap();

            count += 1;

            if count == 30 {
                break;
            }

            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }

        nx_api.del_nexus(&nexus.uuid).await.unwrap();
    }
}

#[tokio::test]
#[ignore]
async fn repeated_volume_rebuilds() {
    let volume_1_size = 250 * 1024 * 1024u64;

    let cluster = ClusterBuilder::builder()
        .with_io_engines(4)
        .with_csi(false, true)
        .with_pool(1, "malloc:///pool?size_mb=300")
        .with_pool(2, "malloc:///pool?size_mb=300")
        .with_pool(3, "malloc:///pool?size_mb=300")
        .with_cache_period("1s")
        .with_options(|o| o.with_isolated_io_engine(true).with_io_engine_cores(3))
        .build()
        .await
        .unwrap();

    let cli = cluster.rest_v00();
    let volumes_api = cli.volumes_api();

    cluster
        .composer()
        .exec(
            cluster.csi_container(0).as_str(),
            vec!["nvme", "disconnect-all"],
        )
        .await
        .unwrap();

    loop {
        let mut volume_1 = volumes_api
            .put_volume(
                &Uuid::new_v4(),
                models::CreateVolumeBody::new(
                    models::VolumePolicy::new(true),
                    2,
                    volume_1_size,
                    false,
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
        tracing::info!(
            "Volume replicas: {:?}",
            volume_1.state.target.as_ref().unwrap().children
        );

        let (s, r) = tokio::sync::oneshot::channel::<()>();
        let task = run_fio_vol(&cluster, volume_1.clone(), r).await;

        let mut count = 0;
        while count < 40 {
            let replica = match volume_1.spec.num_replicas {
                3 => 2,
                2 => 3,
                _ => panic!("oops"),
            };
            volume_1 = volumes_api
                .put_volume_replica_count(&volume_1.spec.uuid, replica)
                .await
                .unwrap();

            volume_1 = wait_till_volume_online(volume_1, volumes_api)
                .await
                .unwrap();

            if task.is_finished() {
                // no point waiting...
                break;
            }

            count += 1;
        }
        s.send(()).ok();
        tracing::info!("Stopped at count {count}");
        let code = task.await.unwrap();
        cluster
            .composer()
            .exec(
                cluster.csi_container(0).as_str(),
                vec!["nvme", "disconnect-all"],
            )
            .await
            .unwrap();

        assert_eq!(code, Some(0), "Fio Failure");
        volumes_api.del_volume(&volume_1.state.uuid).await.unwrap();
    }
}

async fn wait_till_volume_online(
    volume: models::Volume,
    volume_client: &dyn Volumes,
) -> Result<models::Volume, models::Volume> {
    wait_till_volume_online_tmo(volume, volume_client, Duration::from_secs(30)).await
}

async fn wait_till_volume_online_tmo(
    volume: models::Volume,
    volume_client: &dyn Volumes,
    timeout: Duration,
) -> Result<models::Volume, models::Volume> {
    let start = std::time::Instant::now();
    loop {
        let volume = volume_client.get_volume(&volume.spec.uuid).await.unwrap();

        if volume.state.status == models::VolumeStatus::Online {
            return Ok(volume);
        }

        if std::time::Instant::now() > (start + timeout) {
            tracing::error!(?volume, "Timeout waiting for the volume become online");
            return Err(volume);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

async fn run_fio_vol(
    cluster: &Cluster,
    volume: models::Volume,
    stop: tokio::sync::oneshot::Receiver<()>,
) -> tokio::task::JoinHandle<Option<i64>> {
    run_fio_vol_verify(cluster, volume, stop, None, "30s").await
}
async fn run_fio_vol_verify(
    cluster: &Cluster,
    volume: models::Volume,
    mut stop: tokio::sync::oneshot::Receiver<()>,
    verify: Option<bool>,
    time: &str,
) -> tokio::task::JoinHandle<Option<i64>> {
    let fio_builder = |device: &str| {
        let filename = format!("--filename={device}");
        let time = format!("--runtime={time}");
        vec![
            "fio",
            "--direct=1",
            "--ioengine=libaio",
            "--bs=4k",
            "--iodepth=16",
            "--verify=crc32",
            "--verify_fatal=1",
            "--numjobs=1",
            "--thinktime=500000",
            "--thinktime_blocks=1000",
            "--name=fio",
            filename.as_str(),
        ]
        .into_iter()
        .chain(match verify {
            Some(true) => {
                vec![
                    "--readwrite=read",
                    "--verify_only",
                    "--verify_dump=1",
                    "--verify_state_load=1",
                ]
            }
            Some(false) => {
                vec![
                    "--readwrite=write",
                    "--verify_state_save=1",
                    "--time_based",
                    time.as_str(),
                ]
            }
            None => {
                vec!["--readwrite=randrw", "--verify_async=2"]
            }
        })
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

#[tokio::test]
#[ignore]
async fn repeated_volume_double_rebuilds() {
    let volume_1_size = 3000 * 1024 * 1024u64;
    let pool_size = volume_1_size + 100 * 1024 * 1024u64;

    let cluster = ClusterBuilder::builder()
        .with_io_engines(2)
        .with_csi(false, true)
        .with_tmpfs_pool(pool_size)
        .with_cache_period("1s")
        .with_reconcile_period(Duration::from_secs(3), Duration::from_secs(3))
        .with_options(|o| o.with_isolated_io_engine(true).with_io_engine_cores(3))
        .build()
        .await
        .unwrap();

    let cli = cluster.rest_v00();
    let volumes_api = cli.volumes_api();

    cluster
        .composer()
        .exec(
            cluster.csi_container(0).as_str(),
            vec!["nvme", "disconnect-all"],
        )
        .await
        .unwrap();

    loop {
        let mut volume_1 = volumes_api
            .put_volume(
                &Uuid::new_v4(),
                models::CreateVolumeBody::new(
                    models::VolumePolicy::new(true),
                    1,
                    volume_1_size,
                    false,
                ),
            )
            .await
            .unwrap();

        let replicas = volume_1
            .state
            .replica_topology
            .iter()
            .take(1)
            .map(|(_, v)| v.node.as_ref().unwrap())
            .collect::<Vec<_>>();
        let replica_node = replicas.first().unwrap();

        let node = if replica_node.as_str() == cluster.node(0).as_str() {
            0
        } else {
            1
        };
        volume_1 = volumes_api
            .put_volume_target(
                &volume_1.spec.uuid,
                PublishVolumeBody::new_all(
                    HashMap::new(),
                    None,
                    cluster.node(node).to_string(),
                    models::VolumeShareProtocol::Nvmf,
                    None,
                    cluster.csi_node(0),
                ),
            )
            .await
            .unwrap();
        tracing::info!(
            "Volume replicas: {:?}",
            volume_1.state.target.as_ref().unwrap().children
        );

        let (s, r) = tokio::sync::oneshot::channel::<()>();
        let task = run_fio_vol(&cluster, volume_1.clone(), r).await;

        let mut count = 0;
        while count < 41 {
            let initial = volume_1.spec.num_replicas;
            let replica = match initial {
                1 => 2,
                2 => 1,
                _ => panic!("oops"),
            };
            volume_1 = volumes_api
                .put_volume_replica_count(&volume_1.spec.uuid, replica)
                .await
                .unwrap();

            if replica > initial {
                let (volume, child) = wait_rebuild_50p(volume_1, volumes_api).await;
                volume_1 = volume;

                assert!(child.uri.starts_with("nvmf"), "uri: {}", child.uri);

                let snode = if node == 0 { 1 } else { 0 };
                cluster
                    .composer()
                    .restart(cluster.node(snode).as_str())
                    .await
                    .unwrap();
            }

            volume_1 = wait_till_volume_online(volume_1, volumes_api)
                .await
                .unwrap();

            if task.is_finished() {
                // no point waiting...
                break;
            }

            count += 1;
        }
        s.send(()).ok();
        tracing::info!("Stopped at count {count}");
        let code = task.await.unwrap();
        if code != Some(0) {
            cluster
                .composer()
                .exec(
                    cluster.csi_container(0).as_str(),
                    vec!["nvme", "disconnect-all"],
                )
                .await
                .unwrap();
        }

        assert_eq!(code, Some(0), "Fio Failure");
        volumes_api.del_volume(&volume_1.state.uuid).await.unwrap();
    }
}

async fn wait_rebuild_50p(
    volume: models::Volume,
    volume_client: &dyn Volumes,
) -> (models::Volume, models::Child) {
    wait_rebuild(volume, volume_client, 50).await
}

async fn wait_rebuild(
    volume: models::Volume,
    volume_client: &dyn Volumes,
    progress: u8,
) -> (models::Volume, models::Child) {
    let timeout = std::time::Duration::from_secs(30);
    let start = std::time::Instant::now();
    loop {
        let volume = volume_client.get_volume(&volume.spec.uuid).await.unwrap();
        let target = volume.state.target.as_ref().unwrap();
        let degraded = target
            .children
            .iter()
            .filter(|c| c.state == models::ChildState::Degraded)
            .collect::<Vec<_>>();
        assert!(!degraded.is_empty());
        tracing::info!(
            "Children: {:?}",
            volume.state.target.as_ref().unwrap().children
        );
        if let Some(child) = degraded
            .into_iter()
            .find(|c| c.rebuild_progress.unwrap() > progress)
            .cloned()
        {
            return (volume, child);
        }

        if std::time::Instant::now() > (start + timeout) {
            tracing::error!(?volume.state.target, "Timeout waiting for the volume rebuild>50%");
            panic!("Timeout waiting for the volume rebuild>50%");
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

#[tokio::test]
#[ignore]
async fn fault_rebuild_verify() {
    let volume_1_size = 1000 * 1024 * 1024u64;
    let pool_size = volume_1_size + 100 * 1024 * 1024u64;

    let cluster = ClusterBuilder::builder()
        .with_io_engines(2)
        .with_csi(false, true)
        .with_tmpfs_pool(pool_size)
        .with_cache_period("100ms")
        .with_reconcile_period(Duration::from_secs(3), Duration::from_secs(3))
        .with_options(|o| o.with_isolated_io_engine(true).with_io_engine_cores(1))
        .build()
        .await
        .unwrap();

    let cli = cluster.rest_v00();
    let volumes_api = cli.volumes_api();

    cluster
        .composer()
        .exec(
            cluster.csi_container(0).as_str(),
            vec!["nvme", "disconnect-all"],
        )
        .await
        .unwrap();

    let mut volume_1 = volumes_api
        .put_volume(
            &Uuid::new_v4(),
            models::CreateVolumeBody::new(
                models::VolumePolicy::new(false),
                1,
                volume_1_size,
                false,
            ),
        )
        .await
        .unwrap();

    let replicas = volume_1
        .state
        .replica_topology
        .iter()
        .take(1)
        .map(|(_, v)| v.node.as_ref().unwrap())
        .collect::<Vec<_>>();
    let replica_node = replicas.first().unwrap();

    let node = if replica_node.as_str() == cluster.node(0).as_str() {
        0
    } else {
        1
    };
    volume_1 = volumes_api
        .put_volume_target(
            &volume_1.spec.uuid,
            PublishVolumeBody::new_all(
                HashMap::new(),
                None,
                cluster.node(node).to_string(),
                models::VolumeShareProtocol::Nvmf,
                None,
                cluster.csi_node(0),
            ),
        )
        .await
        .unwrap();
    tracing::info!(
        "Volume replicas: {:?}",
        volume_1.state.target.as_ref().unwrap().children
    );

    volume_1 = volumes_api
        .put_volume_replica_count(&volume_1.spec.uuid, 2)
        .await
        .unwrap();

    let (volume, child) = wait_rebuild(volume_1, volumes_api, 20).await;
    volume_1 = volume;

    assert!(child.uri.starts_with("nvmf"), "uri: {}", child.uri);

    let nexus = volume_1.state.target.as_ref().unwrap().clone();

    // create an IO failure which will trigger a bug if the child is not faulted properly
    cluster
        .composer()
        .restart(cluster.node(if node == 0 { 1 } else { 0 }).as_str())
        .await
        .unwrap();

    // once it's rebuilt it will be available for reads
    volume_1 =
        match wait_till_volume_online_tmo(volume_1, volumes_api, Duration::from_secs(10)).await {
            Ok(_) => panic!("How can we be online?"),
            Err(volume_1) => {
                let children = &volume_1.state.target.as_ref().unwrap().children;

                assert!(children
                    .iter()
                    .any(|c| c.state != models::ChildState::Online));

                let nx_cli = cluster.grpc_client().nexus();
                let nexus = nx_cli
                    .get(Filter::Nexus(nexus.uuid.into()), None)
                    .await
                    .unwrap();
                let nexus = &nexus.into_inner()[0];
                let child = nexus.children.iter().find(|c| !c.state.online()).unwrap();

                // in this situation the rebuilds fails because the child was successfully faulted
                // and so the rebuild cannot get a new handle to the child and fails
                // when trying to write after the resume.
                assert_eq!(child.state, ChildState::Faulted);

                volume_1
            }
        };

    cluster
        .composer()
        .exec(
            cluster.csi_container(0).as_str(),
            vec!["nvme", "disconnect-all"],
        )
        .await
        .unwrap();

    volumes_api.del_volume(&volume_1.state.uuid).await.unwrap();
}

#[tokio::test]
#[ignore]
async fn destroy_rebuilding_nexus() {
    let volume_1_size = 1000 * 1024 * 1024u64;
    let pool_size = volume_1_size + 100 * 1024 * 1024u64;

    let cluster = ClusterBuilder::builder()
        .with_io_engines(2)
        .with_csi(false, true)
        .with_tmpfs_pool(pool_size)
        .with_cache_period("100ms")
        .with_reconcile_period(Duration::from_secs(3), Duration::from_secs(3))
        .with_options(|o| o.with_isolated_io_engine(true).with_io_engine_cores(1))
        .build()
        .await
        .unwrap();

    let cli = cluster.rest_v00();
    let volumes_api = cli.volumes_api();

    let mut volume_1 = volumes_api
        .put_volume(
            &Uuid::new_v4(),
            models::CreateVolumeBody::new(
                models::VolumePolicy::new(false),
                1,
                volume_1_size,
                false,
            ),
        )
        .await
        .unwrap();

    for _ in 0 .. 40 {
        let replicas = volume_1
            .state
            .replica_topology
            .iter()
            .take(1)
            .map(|(_, v)| v.node.as_ref().unwrap())
            .collect::<Vec<_>>();
        let replica_node = replicas.first().unwrap();

        let node = if replica_node.as_str() == cluster.node(0).as_str() {
            0
        } else {
            1
        };
        volume_1 = volumes_api
            .put_volume_target(
                &volume_1.spec.uuid,
                PublishVolumeBody::new_all(
                    HashMap::new(),
                    None,
                    cluster.node(node).to_string(),
                    models::VolumeShareProtocol::Nvmf,
                    None,
                    cluster.csi_node(0),
                ),
            )
            .await
            .unwrap();
        tracing::info!(
            "Volume replicas: {:?}",
            volume_1.state.target.as_ref().unwrap().children
        );

        volume_1 = volumes_api
            .put_volume_replica_count(&volume_1.spec.uuid, 2)
            .await
            .unwrap();

        let (volume, child) = wait_rebuild(volume_1, volumes_api, 20).await;
        volume_1 = volume;

        assert!(child.uri.starts_with("nvmf"), "uri: {}", child.uri);

        volumes_api
            .del_volume_target(
                &volume_1.state.uuid,
                None,
                Some(cluster.csi_node(0).as_str()),
            )
            .await
            .unwrap();

        volume_1 = volumes_api
            .put_volume_replica_count(&volume_1.spec.uuid, 1)
            .await
            .unwrap();
    }

    volumes_api.del_volume(&volume_1.state.uuid).await.unwrap();
}
