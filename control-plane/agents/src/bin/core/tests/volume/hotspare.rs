#![cfg(test)]

use super::helpers::{volume_children, wait_till_volume, wait_till_volume_nexus};
use deployer_cluster::{Cluster, ClusterBuilder};
use grpc::operations::{
    node::traits::NodeOperations, registry::traits::RegistryOperations,
    replica::traits::ReplicaOperations, volume::traits::VolumeOperations,
};
use std::{collections::HashMap, convert::TryInto, time::Duration};
use stor_port::{
    pstor::{etcd::Etcd, StorableObject, StoreObj},
    transport_api::TimeoutOptions,
    types::v0::{
        store::volume::VolumeSpec,
        transport::{
            CreateReplica, CreateVolume, DestroyReplica, DestroyVolume, Filter, GetReplicas,
            GetSpecs, PublishVolume, ReplicaId, ReplicaOwners,
        },
    },
};

#[tokio::test]
async fn hotspare() {
    let cluster = ClusterBuilder::builder()
        .with_rest(true)
        .with_io_engines(3)
        .with_pools(2)
        .with_cache_period("1s")
        .with_reconcile_period(Duration::from_secs(1), Duration::from_secs(1))
        .build()
        .await
        .unwrap();

    let node_client = cluster.grpc_client().node();
    let nodes = node_client.get(Filter::None, false, None).await.unwrap();
    tracing::info!("Nodes: {:?}", nodes);

    hotspare_faulty_children(&cluster).await;
    hotspare_unknown_children(&cluster).await;
    hotspare_missing_children(&cluster).await;
    hotspare_replica_count(&cluster).await;
    hotspare_replica_count_spread(&cluster).await;
    hotspare_nexus_replica_count(&cluster).await;
}

/// Faults a volume nexus replica and waits for it to be replaced with a new one
async fn hotspare_faulty_children(cluster: &Cluster) {
    let volume_client = cluster.grpc_client().volume();
    let registry_client = cluster.grpc_client().registry();
    let volume = volume_client
        .create(
            &CreateVolume {
                uuid: "1e3cf927-80c2-47a8-adf0-95c486bdd7b7".try_into().unwrap(),
                size: 5242880,
                replicas: 2,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    let volume = volume_client
        .publish(
            &PublishVolume::new(
                volume.spec().uuid.clone(),
                Some(cluster.node(0)),
                None,
                HashMap::new(),
                vec![],
            ),
            None,
        )
        .await
        .unwrap();

    tracing::info!("Volume: {:?}", volume);
    let volume_state = volume.state();
    let nexus = volume_state.target.unwrap().clone();

    let mut rpc_handle = cluster.grpc_handle(cluster.node(0).as_str()).await.unwrap();

    let children_before_fault = volume_children(volume.uuid(), &volume_client).await;
    tracing::info!("volume children: {:?}", children_before_fault);

    let fault_child = nexus.children.first().unwrap().uri.to_string();
    rpc_handle
        .fault_child(nexus.uuid.as_str(), &fault_child)
        .await
        .unwrap();

    tracing::debug!("Nexus: {:?}", rpc_handle.list_nexuses().await.unwrap());

    let children = wait_till_volume_nexus(
        volume.uuid(),
        2,
        &fault_child,
        &volume_client,
        &registry_client,
    )
    .await;
    tracing::info!("volume children: {:?}", children);

    assert_eq!(children.len(), 2);
    // the faulted child should have been replaced!
    assert!(!children.iter().any(|c| c.uri == fault_child));

    volume_client
        .destroy(&DestroyVolume::new(volume.uuid()), None)
        .await
        .unwrap();
}

/// Adds a child to the volume nexus (under the control plane) and waits till it gets removed
async fn hotspare_unknown_children(cluster: &Cluster) {
    let volume_client = cluster.grpc_client().volume();
    let registry_client = cluster.grpc_client().registry();
    let volume = volume_client
        .create(
            &CreateVolume {
                uuid: "1e3cf927-80c2-47a8-adf0-95c486bdd7b7".try_into().unwrap(),
                size: 5242880,
                replicas: 2,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    let size_mb = volume.spec().size / 1024 / 1024 + 5;

    let volume = volume_client
        .publish(
            &PublishVolume::new(
                volume.spec().uuid.clone(),
                Some(cluster.node(0)),
                None,
                HashMap::new(),
                vec![],
            ),
            None,
        )
        .await
        .unwrap();

    tracing::info!("Volume: {:?}", volume);
    let volume_state = volume.state();
    let nexus = volume_state.target.unwrap().clone();

    let mut rpc_handle = cluster.grpc_handle(cluster.node(0).as_str()).await.unwrap();

    let children_before_fault = volume_children(volume.uuid(), &volume_client).await;
    tracing::info!("volume children: {:?}", children_before_fault);

    let unknown_replica = format!(
        "malloc:///xxxxx?size_mb={}&uuid={}",
        size_mb,
        ReplicaId::new()
    );

    // todo: this sometimes fails??
    // is the reconciler interleaving with the add_child_nexus?
    rpc_handle
        .add_child(nexus.uuid.as_str(), &unknown_replica, true)
        .await
        .ok();

    tracing::debug!("Nexus: {:?}", rpc_handle.list_nexuses().await.unwrap());
    let children = wait_till_volume_nexus(
        volume.uuid(),
        2,
        &unknown_replica,
        &volume_client,
        &registry_client,
    )
    .await;
    tracing::info!("volume children: {:?}", children);

    assert_eq!(children.len(), 2);
    // the unknown child should have been replaced!
    assert!(!children.iter().any(|c| c.uri == unknown_replica));

    volume_client
        .destroy(&DestroyVolume::new(volume.uuid()), None)
        .await
        .unwrap();
}

/// Remove a child from a volume nexus (under the control plane) and waits till it gets added back
async fn hotspare_missing_children(cluster: &Cluster) {
    let volume_client = cluster.grpc_client().volume();
    let registry_client = cluster.grpc_client().registry();
    let volume = volume_client
        .create(
            &CreateVolume {
                uuid: "1e3cf927-80c2-47a8-adf0-95c486bdd7b7".try_into().unwrap(),
                size: 5242880,
                replicas: 2,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    let volume = volume_client
        .publish(
            &PublishVolume::new(
                volume.spec().uuid.clone(),
                Some(cluster.node(0)),
                None,
                HashMap::new(),
                vec![],
            ),
            None,
        )
        .await
        .unwrap();

    tracing::info!("Volume: {:?}", volume);
    let volume_state = volume.state();
    let nexus = volume_state.target.unwrap().clone();

    let mut rpc_handle = cluster.grpc_handle(cluster.node(0).as_str()).await.unwrap();

    let children_before_fault = volume_children(volume.uuid(), &volume_client).await;
    tracing::info!("volume children: {:?}", children_before_fault);

    let missing_child = nexus.children.first().unwrap().uri.to_string();
    rpc_handle
        .remove_child(nexus.uuid.as_str(), &missing_child)
        .await
        .unwrap();

    tracing::debug!("Nexus: {:?}", rpc_handle.list_nexuses().await.unwrap());
    let children = wait_till_volume_nexus(
        volume.uuid(),
        2,
        &missing_child,
        &volume_client,
        &registry_client,
    )
    .await;
    tracing::info!("volume children: {:?}", children);

    assert_eq!(children.len(), 2);
    // the missing child should have been replaced!
    assert!(!children.iter().any(|c| c.uri == missing_child));

    volume_client
        .destroy(&DestroyVolume::new(volume.uuid()), None)
        .await
        .unwrap();
}

/// When more than 1 replicas are faulted at the same time, the new replicas should be spread
/// across the existing pools, and no pool nor any node should be reused
async fn hotspare_replica_count_spread(cluster: &Cluster) {
    let nodes = cluster
        .rest_v00()
        .nodes_api()
        .get_nodes(None)
        .await
        .unwrap();
    assert!(
        nodes.len() >= 3,
        "We need enough nodes to be able to add at least 2 replicas"
    );
    let pools = cluster
        .rest_v00()
        .pools_api()
        .get_pools(None)
        .await
        .unwrap();
    assert!(
        pools.len() >= nodes.len() * 2,
        "We need at least 2 pools per node to be able to test the failure case"
    );

    let replica_count = nodes.len();
    let volume_client = cluster.grpc_client().volume();
    let registry_client = cluster.grpc_client().registry();
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

    // stop the core agent, so we can simulate `replica_count-1` data replicas being faulted at once
    // by increasing the replica count from 1 to `replica_count` under the core agent
    cluster.composer().stop("core").await.unwrap();

    let mut store = Etcd::new("0.0.0.0:2379")
        .await
        .expect("Failed to connect to etcd.");

    let mut volume_spec: VolumeSpec = store.get_obj(&volume.spec().key()).await.unwrap();
    volume_spec.num_replicas = replica_count as u8;
    tracing::info!("VolumeSpec: {:?}", volume_spec);
    store.put_obj(&volume_spec).await.unwrap();

    cluster.restart_core().await;

    let timeout_opts = TimeoutOptions::default()
        .with_max_retries(10)
        .with_req_timeout(Duration::from_millis(200))
        .with_timeout_backoff(Duration::from_millis(50));

    cluster
        .volume_service_liveness(Some(timeout_opts.clone()))
        .await
        .expect("Should have restarted by now");

    // check we have the new replica_count
    wait_till_volume(
        volume.uuid(),
        replica_count,
        &volume_client,
        &registry_client,
    )
    .await;

    {
        let volume = cluster
            .rest_v00()
            .volumes_api()
            .get_volume(volume.uuid())
            .await
            .unwrap();

        tracing::info!("Replicas: {:?}", volume.state.replica_topology);

        assert_eq!(volume.spec.num_replicas, replica_count as u8);
        assert_eq!(volume.state.replica_topology.len(), replica_count);

        for node in 0..nodes.len() {
            let node = cluster.node(node as u32);
            let replicas = volume
                .state
                .replica_topology
                .values()
                .filter(|r| r.node == Some(node.to_string()));
            assert_eq!(replicas.count(), 1, "each node should have 1 replica");
        }
    }

    volume_client
        .destroy(&DestroyVolume::new(volume.uuid()), None)
        .await
        .unwrap();
}

/// Remove a replica that belongs to a volume. Another should be created.
async fn hotspare_replica_count(cluster: &Cluster) {
    let replica_client = cluster.grpc_client().replica();
    let volume_client = cluster.grpc_client().volume();
    let registry_client = cluster.grpc_client().registry();
    let volume = volume_client
        .create(
            &CreateVolume {
                uuid: "1e3cf927-80c2-47a8-adf0-95c486bdd7b7".try_into().unwrap(),
                size: 5242880,
                replicas: 2,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    let specs = registry_client.get_specs(&GetSpecs {}, None).await.unwrap();

    let replica_spec = specs.replicas.first().cloned().unwrap();
    let replicas = replica_client
        .get(GetReplicas::new(&replica_spec.uuid).filter, None)
        .await
        .unwrap();
    let replica = replicas.0.first().unwrap().clone();

    // forcefully destroy a volume replica
    let mut destroy = DestroyReplica::from(replica);
    destroy.disowners = ReplicaOwners::from_volume(volume.uuid());
    match replica_client.destroy(&destroy, None).await {
        Ok(_) => tracing::info!("replica destroyed forcefully"),
        Err(_) => tracing::error!("could not destroy replica forcefully"),
    }

    // check we have 2 replicas
    wait_till_volume(volume.uuid(), 2, &volume_client, &registry_client).await;

    // now add one extra replica (it should be removed)
    replica_client
        .create(
            &CreateReplica {
                node: cluster.node(1),
                name: Default::default(),
                uuid: ReplicaId::new(),
                entity_id: None,
                pool_id: cluster.pool(1, 0),
                pool_uuid: None,
                size: volume.spec().size / 1024 / 1024 + 5,
                thin: false,
                share: Default::default(),
                managed: true,
                owners: ReplicaOwners::from_volume(volume.uuid()),
                allowed_hosts: vec![],
                kind: None,
            },
            None,
        )
        .await
        .unwrap();

    wait_till_volume(volume.uuid(), 2, &volume_client, &registry_client).await;

    volume_client
        .destroy(&DestroyVolume::new(volume.uuid()), None)
        .await
        .unwrap();
}

/// Remove a replica that belongs to a volume. Another should be created.
async fn hotspare_nexus_replica_count(cluster: &Cluster) {
    let volume_client = cluster.grpc_client().volume();
    let registry_client = cluster.grpc_client().registry();
    let volume = volume_client
        .create(
            &CreateVolume {
                uuid: "1e3cf927-80c2-47a8-adf0-95c486bdd7b7".try_into().unwrap(),
                size: 5242880,
                replicas: 2,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    let volume = volume_client
        .publish(
            &PublishVolume::new(
                volume.spec().uuid.clone(),
                Some(cluster.node(0)),
                None,
                HashMap::new(),
                vec![],
            ),
            None,
        )
        .await
        .unwrap();

    tracing::info!("Volume: {:?}", volume);

    let timeout_opts = TimeoutOptions::default()
        .with_max_retries(10)
        .with_req_timeout(Duration::from_millis(500))
        .with_timeout_backoff(Duration::from_millis(50));
    let mut store = Etcd::new("0.0.0.0:2379")
        .await
        .expect("Failed to connect to etcd.");

    let mut volume_spec: VolumeSpec = store.get_obj(&volume.spec().key()).await.unwrap();
    volume_spec.num_replicas += 1;
    tracing::info!("VolumeSpec: {:?}", volume_spec);
    store.put_obj(&volume_spec).await.unwrap();

    cluster.restart_core().await;

    cluster
        .volume_service_liveness(Some(timeout_opts.clone()))
        .await
        .expect("Should have restarted by now");

    wait_till_volume_nexus(
        volume.uuid(),
        volume_spec.num_replicas as usize,
        "",
        &volume_client,
        &registry_client,
    )
    .await;

    let mut volume_spec: VolumeSpec = store.get_obj(&volume.spec().key()).await.unwrap();
    volume_spec.num_replicas -= 1;
    tracing::info!("VolumeSpec: {:?}", volume_spec);
    store.put_obj(&volume_spec).await.unwrap();

    cluster.restart_core().await;

    cluster
        .volume_service_liveness(Some(timeout_opts.clone()))
        .await
        .expect("Should have restarted by now");

    wait_till_volume_nexus(
        volume.uuid(),
        volume_spec.num_replicas as usize,
        "",
        &volume_client,
        &registry_client,
    )
    .await;

    volume_client
        .destroy(&DestroyVolume::new(volume.uuid()), None)
        .await
        .unwrap();
}
