use deployer_cluster::{Cluster, ClusterBuilder};
use grpc::{
    context::Context,
    operations::{
        node::traits::NodeOperations, pool::traits::PoolOperations,
        registry::traits::RegistryOperations, replica::traits::ReplicaOperations,
    },
};
use itertools::Itertools;
use std::{collections::HashMap, convert::TryFrom, thread::sleep, time::Duration};
use stor_port::{
    pstor::{etcd::Etcd, StoreObj},
    transport_api::{ReplyError, ReplyErrorKind, ResourceKind, TimeoutOptions},
    types::v0::{
        openapi::{
            apis::StatusCode,
            clients::tower::Error,
            models,
            models::{
                CreateReplicaBody, CreateVolumeBody, Pool, PoolState, PublishVolumeBody,
                VolumePolicy,
            },
        },
        store::{
            pool::PoolLabel,
            replica::{ReplicaSpec, ReplicaSpecKey},
        },
        transport::{
            CreatePool, CreateReplica, DestroyPool, DestroyReplica, Filter, GetSpecs, NexusId,
            NodeId, Protocol, Replica, ReplicaId, ReplicaName, ReplicaOwners, ReplicaShareProtocol,
            ReplicaStatus, ShareReplica, UnshareReplica, VolumeId,
        },
    },
};

#[tokio::test]
async fn pool() {
    let cluster = ClusterBuilder::builder()
        .with_rest(false)
        .with_agents(vec!["core"])
        .build()
        .await
        .unwrap();

    let node_client = cluster.grpc_client().node();
    let pool_client = cluster.grpc_client().pool();
    let rep_client = cluster.grpc_client().replica();

    let io_engine = cluster.node(0);
    let nodes = node_client.get(Filter::None, false, None).await.unwrap();
    tracing::info!("Nodes: {:?}", nodes);

    let pool = pool_client
        .create(
            &CreatePool {
                node: io_engine.clone(),
                id: "pooloop".into(),
                disks: vec!["malloc:///disk0?size_mb=100".into()],
                labels: None,
            },
            None,
        )
        .await
        .unwrap();
    let _pool2 = pool_client
        .create(
            &CreatePool {
                node: io_engine.clone(),
                id: "pooloop2".into(),
                disks: vec!["malloc:///disk1?size_mb=100".into()],
                labels: None,
            },
            None,
        )
        .await
        .unwrap();
    tracing::info!("Pools: {:?}", pool);

    let pools = pool_client.get(Filter::None, None).await.unwrap();
    tracing::info!("Pools: {:?}", pools);

    let replica = rep_client
        .create(
            &CreateReplica {
                node: io_engine.clone(),
                uuid: ReplicaId::try_from("cf36a440-74c6-4042-b16c-4f7eddfc24da").unwrap(),
                entity_id: None,
                pool_id: "pooloop".into(),
                pool_uuid: None,
                size: 12582912, /* actual size will be a multiple of 4MB so just
                                 * create it like so */
                thin: true,
                share: Protocol::None,
                name: None,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    tracing::info!("Replicas: {:?}", replica);

    let replicas = rep_client.get(Filter::None, None).await.unwrap();
    tracing::info!("Replicas: {:?}", replicas);

    let uri = replica.uri.clone();
    assert_eq!(
        replica,
        Replica {
            node: io_engine.clone(),
            name: ReplicaName::from("cf36a440-74c6-4042-b16c-4f7eddfc24da"),
            uuid: ReplicaId::try_from("cf36a440-74c6-4042-b16c-4f7eddfc24da").unwrap(),
            entity_id: None,
            pool_id: "pooloop".into(),
            pool_uuid: replica.pool_uuid.clone(),
            thin: true,
            size: 12582912,
            space: replica.space.clone(),
            share: Protocol::None,
            uri,
            status: ReplicaStatus::Online,
            allowed_hosts: vec![],
            kind: Default::default(),
        }
    );

    let uri = rep_client
        .share(
            &ShareReplica {
                node: io_engine.clone(),
                uuid: ReplicaId::try_from("cf36a440-74c6-4042-b16c-4f7eddfc24da").unwrap(),
                pool_id: "pooloop".into(),
                pool_uuid: None,
                protocol: ReplicaShareProtocol::Nvmf,
                name: None,
                allowed_hosts: vec![],
            },
            None,
        )
        .await
        .unwrap();

    let mut replica_updated = replica;
    replica_updated.uri = uri;
    replica_updated.share = Protocol::Nvmf;
    let replica = rep_client.get(Filter::None, None).await.unwrap();
    let replica = replica.0.first().unwrap();
    assert_eq!(replica, &replica_updated);

    let error = pool_client
        .destroy(
            &DestroyPool {
                node: io_engine.clone(),
                id: "pooloop".into(),
            },
            None,
        )
        .await
        .expect_err("Should fail to destroy a pool that is in use.");

    assert!(matches!(
        error,
        ReplyError {
            kind: ReplyErrorKind::InUse,
            resource: ResourceKind::Pool,
            ..
        }
    ));

    let error = rep_client
        .destroy(
            &DestroyReplica {
                node: io_engine.clone(),
                uuid: ReplicaId::try_from("cf36a440-74c6-4042-b16c-4f7eddfc24da").unwrap(),
                pool_id: "pooloop2".into(),
                pool_uuid: None,
                name: None,
                ..Default::default()
            },
            None,
        )
        .await
        .expect_err("wrong pool");
    assert_eq!(error.kind, ReplyErrorKind::Aborted);

    rep_client
        .destroy(
            &DestroyReplica {
                node: io_engine.clone(),
                uuid: ReplicaId::try_from("cf36a440-74c6-4042-b16c-4f7eddfc24da").unwrap(),
                pool_id: "pooloop".into(),
                pool_uuid: None,
                name: None,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    let error = rep_client
        .destroy(
            &DestroyReplica {
                node: io_engine.clone(),
                uuid: ReplicaId::try_from("cf36a440-74c6-4042-b16c-4f7eddfc24da").unwrap(),
                pool_id: "pooloop".into(),
                pool_uuid: None,
                name: None,
                ..Default::default()
            },
            None,
        )
        .await
        .expect_err("already deleted");
    assert_eq!(error.kind, ReplyErrorKind::NotFound);

    assert!(rep_client
        .get(Filter::None, None)
        .await
        .unwrap()
        .0
        .is_empty());

    pool_client
        .destroy(
            &DestroyPool {
                node: io_engine.clone(),
                id: "pooloop".into(),
            },
            None,
        )
        .await
        .unwrap();
    pool_client
        .destroy(
            &DestroyPool {
                node: io_engine.clone(),
                id: "pooloop2".into(),
            },
            None,
        )
        .await
        .unwrap();

    let error = rep_client
        .destroy(
            &DestroyReplica {
                node: io_engine.clone(),
                uuid: ReplicaId::try_from("cf36a440-74c6-4042-b16c-4f7eddfc24da").unwrap(),
                pool_id: "pooloop".into(),
                pool_uuid: None,
                name: None,
                ..Default::default()
            },
            None,
        )
        .await
        .expect_err("pool not loaded");
    assert_eq!(error.kind, ReplyErrorKind::FailedPrecondition);

    assert!(pool_client
        .get(Filter::None, None)
        .await
        .unwrap()
        .0
        .is_empty());
}

/// The tests below revolve around transactions and are dependent on the core agent's command line
/// arguments for timeouts.
/// This is required because as of now, we don't have a good mocking strategy

/// default timeout options for every rpc request
fn grpc_timeout_opts() -> TimeoutOptions {
    TimeoutOptions::default()
        .with_max_retries(0)
        .with_req_timeout(Duration::from_millis(250))
}

/// Get the replica spec
async fn replica_spec(replica: &Replica, client: &dyn RegistryOperations) -> Option<ReplicaSpec> {
    client
        .get_specs(&GetSpecs {}, None)
        .await
        .unwrap()
        .replicas
        .iter()
        .find(|r| r.uuid == replica.uuid)
        .cloned()
}

/// Tests replica share and unshare operations as a transaction
#[tokio::test]
async fn replica_transaction() {
    let cluster = ClusterBuilder::builder()
        .with_rest(false)
        .with_pools(1)
        .with_agents(vec!["core"])
        .with_req_timeouts(Duration::from_millis(250), Duration::from_millis(500))
        .with_grpc_timeouts(grpc_timeout_opts())
        .build()
        .await
        .unwrap();
    let io_engine = cluster.node(0);

    let registry_client = cluster.grpc_client().registry();
    let node_client = cluster.grpc_client().node();
    let pool_client = cluster.grpc_client().pool();
    let rep_client = cluster.grpc_client().replica();

    let nodes = node_client.get(Filter::None, false, None).await.unwrap();
    tracing::info!("Nodes: {:?}", nodes);

    let pools = pool_client.get(Filter::None, None).await.unwrap();
    tracing::info!("Pools: {:?}", pools);

    let replica = rep_client
        .create(
            &CreateReplica {
                node: io_engine.clone(),
                uuid: ReplicaId::new(),
                pool_id: cluster.pool(0, 0),
                pool_uuid: None,
                size: 12582912,
                thin: false,
                share: Protocol::None,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    async fn check_operation(
        replica: &Replica,
        protocol: Protocol,
        registry_client: &dyn RegistryOperations,
    ) {
        // operation in progress
        assert!(replica_spec(replica, registry_client)
            .await
            .unwrap()
            .operation
            .is_some());
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        // operation is completed
        assert!(replica_spec(replica, registry_client)
            .await
            .unwrap()
            .operation
            .is_none());
        assert_eq!(
            replica_spec(replica, registry_client).await.unwrap().share,
            protocol
        );
    }

    // pause io_engine
    cluster.composer().pause(io_engine.as_str()).await.unwrap();

    let _ = rep_client
        .share(
            &ShareReplica::from(&replica),
            Some(Context::new(grpc_timeout_opts())),
        )
        .await
        .expect_err("io_engine down");

    check_operation(&replica, Protocol::None, &registry_client).await;

    // unpause io_engine
    cluster.composer().thaw(io_engine.as_str()).await.unwrap();

    // now it should be shared successfully
    let uri = rep_client
        .share(&ShareReplica::from(&replica), None)
        .await
        .unwrap();
    println!("Share uri: {uri}");

    cluster.composer().pause(io_engine.as_str()).await.unwrap();

    let _ = rep_client
        .unshare(
            &UnshareReplica::from(&replica),
            Some(Context::new(grpc_timeout_opts())),
        )
        .await
        .expect_err("io_engine down");

    check_operation(&replica, Protocol::Nvmf, &registry_client).await;

    cluster.composer().thaw(io_engine.as_str()).await.unwrap();

    rep_client
        .unshare(&UnshareReplica::from(&replica), None)
        .await
        .unwrap();

    assert_eq!(
        replica_spec(&replica, &registry_client)
            .await
            .unwrap()
            .share,
        Protocol::None
    );
}

/// Tests Store Write Failures for Replica Operations
/// As it stands, the tests expects the operation to not be undone, and
/// a reconcile thread should eventually sync the specs when the store reappears
async fn replica_op_transaction_store(
    replica: &Replica,
    cluster: &Cluster,
    (store_timeout, reconcile_period, grpc_timeout): (Duration, Duration, Duration),
    protocol: Protocol,
    share: Option<ShareReplica>,
    unshare: Option<UnshareReplica>,
) {
    let io_engine = cluster.node(0);

    // pause io_engine
    cluster.composer().pause(io_engine.as_str()).await.unwrap();

    let rep_client = cluster.grpc_client().replica();
    let registry_client = cluster.grpc_client().registry();

    if share.clone().is_some() {
        rep_client
            .share(&share.as_ref().unwrap().clone(), None)
            .await
            .expect_err("io_engine down");
    }
    if unshare.clone().is_some() {
        rep_client
            .unshare(&unshare.as_ref().unwrap().clone(), None)
            .await
            .expect_err("io_engine down");
    }

    // ensure the share will succeed but etcd store will fail
    // by pausing etcd and releasing the io_engine
    cluster.composer().pause("etcd").await.unwrap();
    cluster.composer().thaw(io_engine.as_str()).await.unwrap();

    // hopefully we have enough time before the store times out
    let spec = replica_spec(replica, &registry_client).await.unwrap();
    assert!(spec.operation.unwrap().result.is_none());

    // let the store write time out
    tokio::time::sleep(grpc_timeout + store_timeout).await;

    // and now we have a result but the operation is still pending until
    // we can sync the spec
    let spec = replica_spec(replica, &registry_client).await.unwrap();
    assert!(spec.operation.unwrap().result.is_some());

    // thaw etcd allowing the worker thread to sync the "dirty" spec
    cluster.composer().thaw("etcd").await.unwrap();

    // wait for the reconciler to do its thing
    tokio::time::sleep(reconcile_period * 2).await;

    // and now we've sync and the pending operation is no more
    let spec = replica_spec(replica, &registry_client).await.unwrap();
    assert!(spec.operation.is_none() && spec.share == protocol);

    if share.clone().is_some() {
        rep_client
            .share(&share.as_ref().unwrap().clone(), None)
            .await
            .expect_err("already done");
    }
    if unshare.clone().is_some() {
        rep_client
            .unshare(&unshare.as_ref().unwrap().clone(), None)
            .await
            .expect_err("already done");
    }
}

/// Tests replica share and unshare operations when the store is temporarily down
#[tokio::test]
async fn replica_transaction_store() {
    let store_timeout = Duration::from_millis(250);
    let reconcile_period = Duration::from_millis(250);
    let grpc_timeout = Duration::from_millis(350);
    let cluster = ClusterBuilder::builder()
        .with_rest(false)
        .with_pools(1)
        .with_agents(vec!["core"])
        .with_req_timeouts(grpc_timeout, grpc_timeout)
        .with_reconcile_period(reconcile_period, reconcile_period)
        .with_store_timeout(store_timeout)
        .with_grpc_timeouts(grpc_timeout_opts())
        .build()
        .await
        .unwrap();
    let rep_client = cluster.grpc_client().replica();
    let io_engine = cluster.node(0);

    let replica = rep_client
        .create(
            &CreateReplica {
                node: io_engine.clone(),
                uuid: ReplicaId::new(),
                pool_id: cluster.pool(0, 0),
                pool_uuid: None,
                size: 12582912,
                thin: false,
                share: Protocol::None,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    replica_op_transaction_store(
        &replica,
        &cluster,
        (store_timeout, reconcile_period, grpc_timeout),
        Protocol::Nvmf,
        Some(ShareReplica::from(&replica)),
        None,
    )
    .await;

    replica_op_transaction_store(
        &replica,
        &cluster,
        (store_timeout, reconcile_period, grpc_timeout),
        Protocol::None,
        None,
        Some(UnshareReplica::from(&replica)),
    )
    .await;
}

const RECONCILE_TIMEOUT_SECS: u64 = 7;
const POOL_FILE_NAME: &str = "disk1.img";
const POOL_FILE_NAME_2: &str = "disk2.img";
const POOL_SIZE_BYTES: u64 = 128 * 1024 * 1024;

/// Creates a pool on a io_engine instance, which will have both spec and state.
/// Stops/Kills the io_engine container. At some point we will have no pool state, because the node
/// is gone. We then restart the node and the pool reconciler will then recreate the pool! At this
/// point, we'll have a state again.
#[tokio::test]
async fn reconciler_missing_pool_state() {
    let disk = deployer_cluster::TmpDiskFile::new(POOL_FILE_NAME, POOL_SIZE_BYTES);

    let cluster = ClusterBuilder::builder()
        .with_rest(true)
        .with_agents(vec!["core"])
        .with_io_engines(1)
        .with_pool(0, disk.uri())
        .with_cache_period("1s")
        .with_reconcile_period(Duration::from_secs(1), Duration::from_millis(1))
        .build()
        .await
        .unwrap();

    let node_client = cluster.grpc_client().node();

    let nodes = node_client.get(Filter::None, false, None).await.unwrap();
    tracing::info!("Nodes: {:?}", nodes);

    let client = cluster.rest_v00();
    let pools_api = client.pools_api();
    let volumes_api = client.volumes_api();

    // create volume to fill up some of the pool space
    for _ in 0 .. 10 {
        let body = CreateVolumeBody::new(VolumePolicy::default(), 1, 8388608u64, false);
        let volume = VolumeId::new();
        volumes_api.put_volume(&volume, body).await.unwrap();
    }
    let replicas = client.replicas_api().get_replicas().await.unwrap();

    let pool = pools_api
        .get_pool(cluster.pool(0, 0).as_str())
        .await
        .unwrap();
    tracing::info!("Pool: {:#?}", pool);

    assert!(pool.spec.is_some());
    assert!(pool.state.is_some());

    let maya = cluster.node(0);
    async fn pool_checker(cluster: &Cluster, state: Option<&PoolState>) {
        let maya = cluster.node(0);
        let tm = Duration::from_secs(3);

        let pool = wait_till_pool_state(cluster, (0, 0), false, tm).await;
        assert!(pool.state.is_none());

        cluster.composer().restart(maya.as_str()).await.unwrap();
        let pool = wait_till_pool_state(cluster, (0, 0), state.is_some(), tm).await;
        // the state should be the same as it was before
        assert_eq!(pool.state.as_ref(), state);
    }

    // let's stop the io_engine container, gracefully
    cluster.composer().stop(maya.as_str()).await.unwrap();
    pool_checker(&cluster, pool.state.as_ref()).await;

    // now kill it, so there's no deregistration message
    cluster.composer().kill(maya.as_str()).await.unwrap();

    // move pool disk to another location and replace it with another disk.
    // this means import should fail as we cannot import from that disk!
    assert_ne!(POOL_FILE_NAME, POOL_FILE_NAME_2);
    let mut disk = disk.into_inner().unwrap();
    disk.rename(POOL_FILE_NAME_2).unwrap();

    let new_disk = deployer_cluster::TmpDiskFile::new(POOL_FILE_NAME, POOL_SIZE_BYTES);
    pool_checker(&cluster, None).await;

    // move original disk back and now import should succeed!
    drop(new_disk);
    disk.rename(POOL_FILE_NAME).unwrap();

    pool_checker(&cluster, pool.state.as_ref()).await;

    // we should have also "imported" the same replicas, perhaps in a different order...
    let current_replicas = client.replicas_api().get_replicas().await.unwrap();
    assert_eq!(
        replicas
            .iter()
            .sorted_by(|a, b| a.uuid.cmp(&b.uuid))
            .collect::<Vec<_>>(),
        current_replicas
            .iter()
            .sorted_by(|a, b| a.uuid.cmp(&b.uuid))
            .collect::<Vec<_>>()
    );
}

/// Wait until the specified pool state option presence matches the `has_state` flag
async fn wait_till_pool_state(
    cluster: &Cluster,
    pool: (u32, u32),
    has_state: bool,
    timeout: Duration,
) -> Pool {
    let pool_id = cluster.pool(pool.0, pool.1);
    let client = cluster.rest_v00();
    let pools_api = client.pools_api();
    let start = std::time::Instant::now();
    loop {
        let pool = pools_api.get_pool(pool_id.as_str()).await.unwrap();

        if has_state && pool.state.is_some() {
            return pool;
        }

        if std::time::Instant::now() > (start + timeout) {
            if !has_state && pool.state.is_none() {
                return pool;
            }
            panic!(
                "Timeout waiting for the pool to have 'has_state': '{has_state}'. Pool: '{pool:#?}'"
            );
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Creates a cluster with two nodes, two pools, core and rest with reconciler period 1 for both
/// busy and idle. Kills node 1 and deletes the pool on it. Somehow the pool is struck in Deleting
/// state. Now the node 1 is brought back and the reconciler Deletes the pool in Deleting state so
/// that pools with same spec can be created.
#[tokio::test]
async fn reconciler_deleting_pool_on_node_down() {
    const POOL_SIZE_BYTES: u64 = 128 * 1024 * 1024;

    let cluster = ClusterBuilder::builder()
        .with_rest(true)
        .with_agents(vec!["core"])
        .with_io_engines(2)
        .with_tmpfs_pool(POOL_SIZE_BYTES)
        .with_tmpfs_pool(POOL_SIZE_BYTES)
        .with_reconcile_period(Duration::from_secs(1), Duration::from_secs(1))
        .build()
        .await
        .unwrap();

    let node_client = cluster.grpc_client().node();
    let nodes = node_client.get(Filter::None, false, None).await.unwrap();
    tracing::info!("Nodes: {:?}", nodes);

    let pool_1_id = cluster.pool(0, 0);
    let pool_2_id = cluster.pool(1, 1);
    let node_1_id = cluster.node(0);
    let client = cluster.rest_v00();
    let timeout = Duration::from_secs(RECONCILE_TIMEOUT_SECS);
    let pools_api = client.pools_api();
    let start = std::time::Instant::now();

    // Kill the io_engine node 1
    cluster.composer().kill(node_1_id.as_str()).await.unwrap();

    // Delete the pool on the io_engine node 1
    let _ = cluster
        .rest_v00()
        .pools_api()
        .del_node_pool(node_1_id.as_str(), pool_1_id.as_str())
        .await;

    let pool_1_status_after_delete = pools_api
        .get_pool(pool_1_id.as_str())
        .await
        .unwrap()
        .spec
        .unwrap()
        .status
        .to_string();

    let pool_2_status = pools_api
        .get_pool(pool_2_id.as_str())
        .await
        .unwrap()
        .spec
        .unwrap()
        .status
        .to_string();

    // The below infers only one node is down and one pool is in Deleting state
    // and the other pools are unaffected.
    assert_eq!(pool_1_status_after_delete, "Deleting");
    assert_eq!(pool_2_status, "Created");

    // Start the node once again
    cluster.composer().start(node_1_id.as_str()).await.unwrap();

    // The reconciler would delete the pool in Deleting state.
    loop {
        match pools_api.get_pool(pool_1_id.as_str()).await {
            Ok(_) => {}
            Err(err) => {
                if let Error::Response(err) = err {
                    if err.status() == StatusCode::NOT_FOUND {
                        break;
                    }
                }
            }
        }
        if std::time::Instant::now() > (start + timeout) {
            panic!("Timeout waiting for the pool delete");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // The other pools are unaffected by reconciler action
    let pool_2_status_after_reconciler_action = pools_api
        .get_pool(pool_2_id.as_str())
        .await
        .unwrap()
        .spec
        .unwrap()
        .status
        .to_string();
    assert_eq!(pool_2_status_after_reconciler_action, "Created");
}

/// Tests that resources in the deleting state are eventually deleted
#[tokio::test]
async fn reconciler_deleting_dirty_pool() {
    let reconcile_period = Duration::from_millis(250);
    let grpc_timeout = TimeoutOptions::default()
        .with_max_retries(0)
        .with_req_timeout(Duration::from_millis(250))
        .with_min_req_timeout(None);
    let req_timeout = grpc_timeout.base_timeout() * 2;
    let store_timeout = Duration::from_millis(300);
    let cluster = ClusterBuilder::builder()
        .with_rest(false)
        .with_pools(1)
        .with_agents(vec!["core"])
        .with_req_timeouts(req_timeout, req_timeout)
        .with_reconcile_period(reconcile_period, reconcile_period)
        .with_store_timeout(store_timeout)
        .with_grpc_timeouts(grpc_timeout.clone())
        .build()
        .await
        .unwrap();
    let node = cluster.node(0);
    let pool = cluster.pool(0, 0);

    let node_client = cluster.grpc_client().node();
    let pool_client = cluster.grpc_client().pool();

    let nodes = node_client.get(Filter::None, false, None).await.unwrap();
    tracing::info!("Nodes: {:?}", nodes);

    let pools = pool_client.get(Filter::None, None).await.unwrap();
    tracing::info!("Pools: {:?}", pools);

    // 1. Pause the node, so the destroy call will timeout
    cluster.composer().pause(node.as_str()).await.unwrap();

    let _ = pool_client
        .destroy(
            &DestroyPool {
                node: node.clone(),
                id: pool.clone(),
            },
            None,
        )
        .await
        .expect_err("timeout since the node is down");

    // 2. Pause ETCD so we fail to undo the operation
    cluster.composer().pause("etcd").await.unwrap();

    tokio::time::sleep(req_timeout - grpc_timeout.base_timeout()).await;

    // 3. Bring the node back so we can delete the pool
    cluster.composer().thaw(node.as_str()).await.unwrap();

    // 4. allow for the store write to time out (plus some slack)
    tokio::time::sleep(store_timeout * 2).await;

    // 5. Bring ETCD back up so we can resume operations
    cluster.composer().thaw("etcd").await.unwrap();

    // 6. The pool should "eventually" be deleted
    wait_pool_deleted(&cluster, node, reconcile_period * 4).await;

    async fn wait_pool_deleted(cluster: &Cluster, node: NodeId, timeout: Duration) {
        let pool_client = cluster.grpc_client().pool();
        let start = std::time::Instant::now();
        loop {
            let pools = pool_client
                .get(Filter::Node(node.clone()), None)
                .await
                .unwrap();
            let pools = pools.into_inner();

            if pools.is_empty() {
                return;
            }

            if std::time::Instant::now() > (start + timeout) {
                panic!("Timeout waiting for the pool to be deleted. Actual: '{pools:#?}'");
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

#[tokio::test]
async fn disown_unused_replicas() {
    const POOL_SIZE_BYTES: u64 = 128 * 1024 * 1024;
    let reconcile_period = Duration::from_millis(200);
    let cluster = ClusterBuilder::builder()
        .with_rest(true)
        .with_agents(vec!["core"])
        .with_io_engines(1)
        .with_tmpfs_pool(POOL_SIZE_BYTES)
        .with_cache_period("1s")
        .with_reconcile_period(reconcile_period, reconcile_period)
        .build()
        .await
        .unwrap();

    let rest_api = cluster.rest_v00();
    let volumes_api = rest_api.volumes_api();
    let node = cluster.node(0).to_string();

    let volume = volumes_api
        .put_volume(
            &"1e3cf927-80c2-47a8-adf0-95c481bdd7b7".parse().unwrap(),
            models::CreateVolumeBody::new(models::VolumePolicy::default(), 1, 5242880u64, false),
        )
        .await
        .unwrap();

    let volume = volumes_api
        .put_volume_target(
            &volume.spec.uuid,
            PublishVolumeBody::new_all(
                HashMap::new(),
                None,
                node.clone().to_string(),
                models::VolumeShareProtocol::Nvmf,
                None,
                cluster.csi_node(0),
            ),
        )
        .await
        .unwrap();

    cluster.composer().pause(&node).await.unwrap();
    volumes_api
        .del_volume_target(&volume.spec.uuid, Some(false))
        .await
        .expect_err("io-engine is down");
    cluster.composer().kill(&node).await.unwrap();

    let volume = volumes_api.get_volume(&volume.spec.uuid).await.unwrap();
    tracing::info!("Volume: {:?}", volume);

    assert!(volume.spec.target.is_some(), "Unpublish failed");

    let specs = cluster.rest_v00().specs_api().get_specs().await.unwrap();
    let replica = specs.replicas.first().cloned().unwrap();
    assert!(replica.owners.volume.is_some());
    assert!(replica.owners.nexuses.is_empty());

    // allow the reconcile to run - it should not disown the replica
    tokio::time::sleep(reconcile_period * 12).await;

    let specs = cluster.rest_v00().specs_api().get_specs().await.unwrap();
    let replica = specs.replicas.first().cloned().unwrap();
    // we should still be part of the volume
    assert!(replica.owners.volume.is_some());
    assert!(replica.owners.nexuses.is_empty());
}

#[tokio::test]
async fn test_disown_missing_replica_owners() {
    let cluster = ClusterBuilder::builder()
        .with_rest(true)
        .with_agents(vec!["core"])
        .with_io_engines(1)
        .with_pools(1)
        .with_cache_period("1s")
        .with_reconcile_period(Duration::from_secs(1), Duration::from_secs(1))
        .build()
        .await
        .unwrap();

    // Create a replica. This will save the replica spec to the persistent store.
    let replica_id = ReplicaId::new();
    cluster
        .rest_v00()
        .replicas_api()
        .put_pool_replica(
            "io-engine-1-pool-1",
            &replica_id,
            CreateReplicaBody {
                share: None,
                size: 5242880,
                thin: false,
                allowed_hosts: None,
            },
        )
        .await
        .expect("Failed to create replica.");

    // Check the replica exists.
    let num_replicas = cluster
        .rest_v00()
        .replicas_api()
        .get_replicas()
        .await
        .expect("Failed to get replicas.")
        .len();
    assert_eq!(num_replicas, 1);

    // Modify the replica spec in the store so that the replica has a volume and nexus owner;
    // neither of which exist.
    let mut etcd = Etcd::new("0.0.0.0:2379").await.unwrap();
    let mut replica: ReplicaSpec = etcd
        .get_obj(&ReplicaSpecKey::from(&replica_id))
        .await
        .unwrap();
    replica.managed = true;
    replica.owners = ReplicaOwners::new(Some(VolumeId::new()), vec![NexusId::new()]);

    // Persist the modified replica spec to the store
    etcd.put_obj(&replica)
        .await
        .expect("Failed to store modified replica.");

    // Restart the core agent so that it reloads the modified replica spec from the persistent
    // store.
    cluster.restart_core().await;

    // Allow time for the core agent to restart.
    sleep(Duration::from_secs(2));

    // The replica should be removed because none of its parents exist.
    let num_replicas = cluster
        .rest_v00()
        .replicas_api()
        .get_replicas()
        .await
        .expect("Failed to get replicas.")
        .len();
    assert_eq!(num_replicas, 0);
}

#[tokio::test]
async fn destroy_after_restart() {
    const POOL_SIZE_BYTES: u64 = 128 * 1024 * 1024;

    let cluster = ClusterBuilder::builder()
        .with_io_engines(1)
        .with_tmpfs_pool(POOL_SIZE_BYTES)
        .with_reconcile_period(Duration::from_secs(10), Duration::from_secs(10))
        .build()
        .await
        .unwrap();

    let client = cluster.grpc_client();

    let pools = client
        .pool()
        .get(Filter::Pool(cluster.pool(0, 0)), None)
        .await
        .unwrap();
    let pool = pools.into_inner().first().cloned().unwrap();

    cluster
        .composer()
        .restart(cluster.node(0).as_str())
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(1)).await;

    let destroy = DestroyPool {
        node: cluster.node(0),
        id: cluster.pool(0, 0),
    };
    let create = CreatePool {
        node: cluster.node(0),
        id: "bob".into(),
        disks: pool.state().cloned().unwrap().disks,
        labels: None,
    };

    client.pool().destroy(&destroy, None).await.unwrap();
    let pool = client.pool().create(&create, None).await.unwrap();

    assert_eq!(pool.state().unwrap().id, create.id);
}

#[tokio::test]
async fn slow_create() {
    const POOL_SIZE_BYTES: u64 = 128 * 1024 * 1024;

    let vg = deployer_cluster::lvm::VolGroup::new("slow-pool", POOL_SIZE_BYTES).unwrap();
    let lvol = vg.create_lvol("lvol0", POOL_SIZE_BYTES / 2).unwrap();
    lvol.suspend().unwrap();
    {
        let cluster = ClusterBuilder::builder()
            .with_io_engines(1)
            .with_reconcile_period(Duration::from_millis(250), Duration::from_millis(250))
            .with_cache_period("200ms")
            .with_options(|o| o.with_io_engine_devices(vec![lvol.path()]))
            .with_req_timeouts(Duration::from_millis(500), Duration::from_millis(500))
            .compose_build(|b| b.with_clean(true))
            .await
            .unwrap();

        let client = cluster.grpc_client();

        let create = CreatePool {
            node: cluster.node(0),
            id: "bob".into(),
            disks: vec![lvol.path().into()],
            labels: Some(PoolLabel::from([("a".into(), "b".into())])),
        };

        let error = client
            .pool()
            .create(&create, None)
            .await
            .expect_err("device suspended");
        assert_eq!(error.kind, ReplyErrorKind::Cancelled);

        lvol.resume().unwrap();

        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(30);
        loop {
            if std::time::Instant::now() > (start + timeout) {
                panic!("Timeout waiting for the pool");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;

            let pools = client
                .pool()
                .get(Filter::Pool(create.id.clone()), None)
                .await
                .unwrap();

            let Some(pool) = pools.0.first() else {
                continue;
            };
            let Some(pool_spec) = pool.spec() else {
                continue;
            };
            if !pool_spec.status.created() {
                continue;
            }
            break;
        }
        let destroy = DestroyPool::from(create.clone());
        client.pool().destroy(&destroy, None).await.unwrap();

        // Now we try to recreate using an API call, rather than using the reconciler
        lvol.suspend().unwrap();

        let error = client
            .pool()
            .create(&create, None)
            .await
            .expect_err("device suspended");
        assert_eq!(error.kind, ReplyErrorKind::Cancelled);

        lvol.resume().unwrap();

        let pool = client.pool().create(&create, None).await.unwrap();
        assert!(pool.spec().unwrap().status.created());
    }
}
