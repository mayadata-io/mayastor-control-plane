#![cfg(test)]

use super::RECONCILE_TIMEOUT_SECS;
use crate::volume::helpers::volume_children;
use deployer_cluster::{Cluster, ClusterBuilder};
use grpc::operations::{
    nexus::traits::NexusOperations, node::traits::NodeOperations, volume::traits::VolumeOperations,
};
use std::{collections::HashMap, convert::TryInto, time::Duration};
use stor_port::types::v0::{
    openapi::{
        apis::{StatusCode, Uuid},
        models,
        models::PublishVolumeBody,
        tower::client::Error,
    },
    store::{nexus::ReplicaUri, nexus_child::NexusChild},
    transport::{
        strip_queries, CreateNexus, CreateVolume, DestroyVolume, Filter, NexusId, PublishVolume,
        ReplicaId, VolumeId, VolumeShareProtocol,
    },
};

#[tokio::test]
async fn garbage_collection() {
    let reconcile_period = Duration::from_millis(500);
    let cluster = ClusterBuilder::builder()
        .with_rest(true)
        .with_io_engines(3)
        .with_tmpfs_pool(POOL_SIZE_BYTES)
        .with_cache_period("1s")
        .with_reconcile_period(reconcile_period, reconcile_period)
        .build()
        .await
        .unwrap();

    let node_client = cluster.grpc_client().node();
    let nodes = node_client.get(Filter::None, false, None).await.unwrap();
    tracing::info!("Nodes: {:?}", nodes);

    unused_nexus_reconcile(&cluster).await;
    unused_reconcile(&cluster).await;
    deleting_volume_reconcile(&cluster).await;
    offline_replicas_reconcile(&cluster, reconcile_period).await;
}

async fn deleting_volume_reconcile(cluster: &Cluster) {
    let client = cluster.grpc_client().volume();
    let volume = client
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

    client
        .publish(
            &PublishVolume {
                uuid: volume.uuid().clone(),
                share: None,
                target_node: None,
                publish_context: HashMap::new(),
                frontend_nodes: vec![],
            },
            None,
        )
        .await
        .unwrap();

    // 1. Pause etcd
    cluster.composer().pause("etcd").await.unwrap();

    // 2. Attempt to delete the volume
    client
        .destroy(
            &DestroyVolume {
                uuid: volume.uuid().to_owned(),
            },
            None,
        )
        .await
        .expect_err("ETCD is paused...");

    // 3. Bring back etcd
    cluster.composer().thaw("etcd").await.unwrap();

    // 4. Wait for volume deletion
    wait_till_volume_deleted(cluster).await;

    // 5. Volume replicas and nexuses should have been deleted as well
    let specs = cluster.rest_v00().specs_api().get_specs().await.unwrap();
    assert!(specs.nexuses.is_empty());
    let nexuses = cluster.rest_v00().nexuses_api().get_nexuses().await;
    assert!(nexuses.unwrap().is_empty());
    assert!(specs.replicas.is_empty());
    let replicas = cluster.rest_v00().replicas_api().get_replicas().await;
    assert!(replicas.unwrap().is_empty());
}

/// Wait for a volume to reach the provided status
async fn wait_till_volume_deleted(cluster: &Cluster) {
    let timeout = Duration::from_secs(RECONCILE_TIMEOUT_SECS);
    let client = cluster.grpc_client().volume();
    let start = std::time::Instant::now();
    loop {
        let volumes = client.get(Filter::None, false, None, None).await.unwrap();
        if volumes.entries.is_empty() {
            return;
        }

        if std::time::Instant::now() > (start + timeout) {
            panic!("Timeout waiting for the volumes to be deleted current: '{volumes:?}'");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn offline_replicas_reconcile(cluster: &Cluster, reconcile_period: Duration) {
    let rest_api = cluster.rest_v00();
    let volumes_api = rest_api.volumes_api();

    let volume = volumes_api
        .put_volume(
            &"1e3cf927-80c2-47a8-adf0-95c481bdd7b7".parse().unwrap(),
            models::CreateVolumeBody::new(models::VolumePolicy::default(), 2, 5242880u64, false),
        )
        .await
        .unwrap();

    let nodes = rest_api.nodes_api().get_nodes(None).await.unwrap();
    let replica_nodes = rest_api.replicas_api().get_replicas().await.unwrap();
    let replica_nodes = replica_nodes
        .into_iter()
        .map(|r| r.node)
        .collect::<Vec<_>>();
    let free_node = nodes
        .into_iter()
        .find_map(|n| {
            if replica_nodes.iter().all(|repl_node| repl_node != &n.id) {
                Some(n.id)
            } else {
                None
            }
        })
        .unwrap();

    // 1. publish on the node with no replicas
    let volume = volumes_api
        .put_volume_target(
            &volume.spec.uuid,
            PublishVolumeBody::new_all(
                HashMap::new(),
                None,
                free_node.clone().to_string(),
                models::VolumeShareProtocol::Nvmf,
                None,
                cluster.csi_node(0),
            ),
        )
        .await
        .unwrap();

    tracing::info!("Volume: {:?}", volume);
    let volume_state = volume.state;

    let volume_id = volume_state.uuid;
    let volume = volumes_api.get_volume(&volume_id).await.unwrap();
    assert_eq!(volume.state.status, models::VolumeStatus::Online);

    // 2. kill all replica nodes
    for node in replica_nodes.iter().filter(|n| n != &&free_node) {
        cluster.composer().stop(node).await.unwrap();
    }

    // 3. wait for volume to become Faulted
    wait_till_volume_status(cluster, &volume.spec.uuid, models::VolumeStatus::Faulted).await;

    // 4. restart the core-agent
    cluster.restart_core().await;

    cluster.volume_service_liveness(None).await.unwrap();
    wait_till_volume_status(cluster, &volume.spec.uuid, models::VolumeStatus::Faulted).await;

    // 5. After the reconcilers run, replicas should not have been disowned
    tokio::time::sleep(reconcile_period * 3).await;

    let volume = volumes_api.get_volume(&volume.spec.uuid).await.unwrap();
    assert_eq!(volume.state.status, models::VolumeStatus::Faulted);

    let replicas = rest_api.specs_api().get_specs().await.unwrap().replicas;
    assert_eq!(replicas.len(), 2);
    assert_eq!(
        replicas
            .iter()
            .filter(|r| r.owners.volume.as_ref() == Some(&volume.spec.uuid))
            .count(),
        2
    );

    volumes_api.del_volume(&volume_id).await.unwrap();
}

async fn unused_nexus_reconcile(cluster: &Cluster) {
    let rest_api = cluster.rest_v00();
    let volumes_api = rest_api.volumes_api();
    let nexus_client = cluster.grpc_client().nexus();

    let volume = volumes_api
        .put_volume(
            &"1e3cf927-80c2-47a8-adf0-95c481bdd7b7".parse().unwrap(),
            models::CreateVolumeBody::new(models::VolumePolicy::default(), 2, 5242880u64, false),
        )
        .await
        .unwrap();

    let volume = volumes_api
        .put_volume_target(
            &volume.spec.uuid,
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

    tracing::info!("Volume: {:?}", volume);
    let volume_state = volume.state;

    let volume_id = volume_state.uuid;
    let volume = volumes_api.get_volume(&volume_id).await.unwrap();
    assert_eq!(volume.state.status, models::VolumeStatus::Online);

    let mut create_nexus = CreateNexus {
        node: cluster.node(0),
        uuid: NexusId::new(),
        size: volume.spec.size,
        children: vec![
            "malloc:///test?size_mb=10&uuid=b9558b8c-cb22-47f3-b33b-583db25b5a8c".into(),
        ],
        managed: true,
        owner: None,
        config: None,
    };
    let nexus = nexus_client.create(&create_nexus, None).await.unwrap();
    let nexus = wait_till_nexus_state(cluster, &nexus.uuid, None).await;
    assert_eq!(nexus, None, "nexus should be gone");

    create_nexus.owner = Some(VolumeId::new());
    let nexus = nexus_client.create(&create_nexus, None).await.unwrap();
    let nexus = wait_till_nexus_state(cluster, &nexus.uuid, None).await;
    assert_eq!(nexus, None, "nexus should be gone");

    volumes_api.del_volume(&volume_id).await.unwrap();
}

async fn unused_reconcile(cluster: &Cluster) {
    let rest_api = cluster.rest_v00();
    let volumes_api = rest_api.volumes_api();

    let volume = volumes_api
        .put_volume(
            &"22054b1f-cf32-46dc-90ff-d6a5c61429c2".parse().unwrap(),
            models::CreateVolumeBody::new(models::VolumePolicy::new(true), 2, 5242880u64, false),
        )
        .await
        .unwrap();

    let data_replicas_nodes = volume
        .state
        .replica_topology
        .values()
        .map(|r| r.node.clone().unwrap())
        .collect::<Vec<_>>();
    let nodes = rest_api.nodes_api().get_nodes(None).await.unwrap();
    let unused_node = nodes
        .iter()
        .find(|r| !data_replicas_nodes.contains(&r.id))
        .cloned()
        .unwrap();
    let nexus_node = nodes
        .iter()
        .find(|n| n.id != unused_node.id)
        .cloned()
        .unwrap();
    let replica_nexus = volume
        .state
        .replica_topology
        .into_iter()
        .find_map(|(i, r)| {
            if r.node.as_ref().unwrap() == &nexus_node.id {
                Some(i)
            } else {
                None
            }
        })
        .unwrap();

    let volume = volumes_api
        .put_volume_target(
            &volume.spec.uuid,
            PublishVolumeBody::new_all(
                HashMap::new(),
                None,
                nexus_node.id.clone(),
                models::VolumeShareProtocol::Nvmf,
                None,
                cluster.csi_node(0),
            ),
        )
        .await
        .unwrap();
    tracing::info!("Volume: {:?}\nUnused Node: {}", volume, unused_node.id);

    // 1. first we kill the node where the nexus is running
    cluster.composer().kill(&nexus_node.id).await.unwrap();
    // 2. now we force unpublish the volume
    volumes_api
        .del_volume_target(
            &volume.spec.uuid,
            Some(true),
            Some(cluster.csi_node(0).as_str()),
        )
        .await
        .unwrap();
    // 3. publish on the previously unused node
    let volume = volumes_api
        .put_volume_target(
            &volume.spec.uuid,
            PublishVolumeBody::new_all(
                HashMap::new(),
                None,
                unused_node.id.to_string(),
                models::VolumeShareProtocol::Nvmf,
                None,
                cluster.csi_node(0),
            ),
        )
        .await
        .unwrap();
    tracing::info!("Volume: {:?}", volume);

    // 4. now wait till the "broken" replica is disowned
    wait_till_replica_disowned(cluster, replica_nexus.parse().unwrap()).await;

    // 5. now wait till the volume becomes online again
    // (because we'll add a replica a rebuild)
    wait_till_volume_status(cluster, &volume.spec.uuid, models::VolumeStatus::Online).await;

    // 6. Bring back the io-engine and the original nexus and replica should be deleted
    cluster.composer().start(&nexus_node.id).await.unwrap();
    let timeout = Duration::from_secs(RECONCILE_TIMEOUT_SECS);
    let start = std::time::Instant::now();
    loop {
        let specs = cluster.rest_v00().specs_api().get_specs().await.unwrap();
        if specs.nexuses.len() == 1 && specs.replicas.len() == 2 {
            break;
        }

        if std::time::Instant::now() > (start + timeout) {
            panic!("Timeout waiting for the old nexus and replica to be removed");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    volumes_api.del_volume(&volume.spec.uuid).await.unwrap();
}

async fn wait_till_replica_disowned(cluster: &Cluster, replica_id: Uuid) {
    let timeout = Duration::from_secs(RECONCILE_TIMEOUT_SECS);
    let client = cluster.rest_v00();
    let specs_api = client.specs_api();
    let start = std::time::Instant::now();
    loop {
        let specs = specs_api.get_specs().await.unwrap();
        let replica_spec = specs
            .replicas
            .into_iter()
            .find(|r| r.uuid == replica_id)
            .unwrap();

        if replica_spec.owners.volume.is_none() {
            return;
        }

        if std::time::Instant::now() > (start + timeout) {
            panic!("Timeout waiting for the replica to be disowned. Actual: '{replica_spec:#?}'");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Creates a volume nexus on a node, which will have both spec and state.
/// Stop/Kill the io-engine container. At some point we will have no nexus state, because the node
/// is gone. We then restart the node and the volume nexus reconciler will then recreate the nexus!
/// At this point, we'll have a state again and the volume will be Online!
async fn missing_nexus_reconcile(cluster: &Cluster) {
    let volume_client = cluster.grpc_client().volume();
    let volume = volume_client
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

    let rest_api = cluster.rest_v00();
    let volumes_api = rest_api.volumes_api();

    let volume = volumes_api
        .put_volume_target(
            &volume.spec().uuid,
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

    tracing::info!("Volume: {:?}", volume);
    let volume_state = volume.state;
    let mut nexus = volume_state.target.unwrap();
    nexus.device_uri = strip_queries(nexus.device_uri, "hostnqn");

    cluster.composer().stop(nexus.node.as_str()).await.unwrap();
    let curr_nexus = wait_till_nexus_state(cluster, &nexus.uuid, None).await;
    assert_eq!(curr_nexus, None);

    cluster.composer().start(nexus.node.as_str()).await.unwrap();
    let curr_nexus = wait_till_nexus_state(cluster, &nexus.uuid, Some(&nexus)).await;
    assert_eq!(Some(nexus), curr_nexus);

    let volume_id = volume_state.uuid;
    let volume = volumes_api.get_volume(&volume_id).await.unwrap();
    assert_eq!(volume.state.status, models::VolumeStatus::Online);

    volumes_api.del_volume(&volume_id).await.unwrap();
}

/// Wait until the specified nexus state option matches the requested `state`
async fn wait_till_nexus_state(
    cluster: &Cluster,
    nexus_id: &Uuid,
    state: Option<&models::Nexus>,
) -> Option<models::Nexus> {
    let timeout = Duration::from_secs(RECONCILE_TIMEOUT_SECS);
    let client = cluster.rest_v00();
    let nexuses_api = client.nexuses_api();
    let start = std::time::Instant::now();
    loop {
        let nexus = nexuses_api.get_nexus(nexus_id).await;
        match &nexus {
            Ok(nexus) => {
                if let Some(state) = state {
                    if nexus.protocol != models::Protocol::None
                        && state.protocol != models::Protocol::None
                    {
                        return Some(nexus.clone());
                    }
                }
            }
            Err(Error::Response(response))
                if response.status() == StatusCode::NOT_FOUND && state.is_none() =>
            {
                return None;
            }
            _ => {}
        };

        if std::time::Instant::now() > (start + timeout) {
            panic!(
                "Timeout waiting for the nexus to have state: '{state:#?}'. Actual: '{nexus:#?}'"
            );
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Wait for a volume to reach the provided status
async fn wait_till_volume_status(cluster: &Cluster, volume: &Uuid, status: models::VolumeStatus) {
    let timeout = Duration::from_secs(RECONCILE_TIMEOUT_SECS);
    super::helpers::wait_till_volume_status(cluster, volume, status, timeout)
        .await
        .unwrap();
}

const POOL_SIZE_BYTES: u64 = 128 * 1024 * 1024;
#[tokio::test]
async fn volume_nexus_reconcile() {
    let cluster = ClusterBuilder::builder()
        .with_rest(true)
        .with_io_engines(2)
        .with_tmpfs_pool(POOL_SIZE_BYTES)
        .with_cache_period("1s")
        .with_reconcile_period(Duration::from_secs(1), Duration::from_secs(1))
        .build()
        .await
        .unwrap();

    let node_client = cluster.grpc_client().node();
    let nodes = node_client.get(Filter::None, false, None).await.unwrap();
    tracing::info!("Nodes: {:?}", nodes);

    missing_nexus_reconcile(&cluster).await;
}

/// When a second nexus with the same child is created for some reason, ensure that removing
/// a replica doesn't cause the replica to be disowned from the volume and destroyed.
/// This is something that shouldn't happen to begin with but this adds a safety net just in case.
#[tokio::test]
async fn duplicate_nexus_missing_children() {
    let reconcile_period = Duration::from_millis(100);
    let cluster = ClusterBuilder::builder()
        .with_rest(true)
        .with_io_engines(2)
        .with_pool(1, "malloc:///d?size_mb=100")
        .with_cache_period("100ms")
        .with_reconcile_period(reconcile_period, reconcile_period)
        .build()
        .await
        .unwrap();

    let volume_client = cluster.grpc_client().volume();
    let nexus_client = cluster.grpc_client().nexus();

    let volume = volume_client
        .create(
            &CreateVolume {
                uuid: VolumeId::try_from("1e3cf927-80c2-47a8-adf0-95c486bdd7b7").unwrap(),
                size: 5242880,
                replicas: 1,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    let pub_volume = PublishVolume {
        uuid: volume.spec().uuid,
        target_node: Some(cluster.node(0)),
        share: Some(VolumeShareProtocol::Nvmf),
        ..Default::default()
    };
    let volume = volume_client.publish(&pub_volume, None).await.unwrap();

    let fake_volume = CreateVolume {
        uuid: "2e3cf927-80c2-47a8-adf0-95c486bdd7b7".try_into().unwrap(),
        size: 5242880,
        replicas: 1,
        ..Default::default()
    };
    let fake_volume = volume_client.create(&fake_volume, None).await.unwrap();

    tracing::info!("Volume: {:?}", volume);
    let volume_state = volume.state();
    let nexus = volume_state.target.unwrap().clone();

    let child = nexus.children.first().cloned().unwrap();
    let child_uuid = ReplicaId::try_from(child.uri.uuid_str().unwrap()).unwrap();
    let child_uri = format!("bdev:///{child_uuid}?uuid={child_uuid}");
    let replica_uri = ReplicaUri::new(&child_uuid, &(child_uri.into()));

    let local = "malloc:///local?size_mb=12&uuid=4a7b0566-8ec6-49e0-a8b2-1d9a292cf59b".into();

    let bad_nexus_req = CreateNexus {
        node: cluster.node(1),
        uuid: NexusId::try_from("f086f12c-1728-449e-be32-9415051090d6").unwrap(),
        size: 5242880,
        children: vec![NexusChild::Replica(replica_uri.clone()), local],
        managed: true,
        // pretend this nexus is from another volume so it won't be deleted..
        owner: Some(fake_volume.uuid().clone()),
        ..Default::default()
    };
    let bad_nexus = nexus_client.create(&bad_nexus_req, None).await.unwrap();

    let nexuses = nexus_client.get(Filter::None, None).await.unwrap();
    tracing::info!("Nexuses: {:?}", nexuses);

    let mut rpc_handle = cluster.grpc_handle(cluster.node(1).as_str()).await.unwrap();

    let children_before_fault = volume_children(volume.uuid(), &volume_client).await;
    tracing::info!("volume children: {:?}", children_before_fault);

    rpc_handle
        .remove_child(bad_nexus.uuid.as_str(), replica_uri.uri().as_str())
        .await
        .unwrap();

    tracing::debug!("Nexus: {:?}", rpc_handle.list_nexuses().await.unwrap());

    // There no easy way to check for a negative here, just wait for 2 garbage reconcilers.
    super::helpers::wait_till_volume_status(
        &cluster,
        volume.uuid(),
        models::VolumeStatus::Faulted,
        reconcile_period * 10,
    )
    .await
    .expect_err("Should not get faulted!");
}
