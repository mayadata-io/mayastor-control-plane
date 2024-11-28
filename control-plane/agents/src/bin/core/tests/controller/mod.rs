use deployer_cluster::{etcd_client::Client, *};
use stor_port::{
    pstor::{etcd::Etcd, key_prefix_obj, ApiVersion, StorableObjectType, StoreKv, StoreObj},
    types::v0::{
        openapi::models,
        store::registry::{ControlPlaneService, StoreLeaseOwner, StoreLeaseOwnerKey},
        transport,
    },
};

use serde_json::Value;
use std::{str::FromStr, time::Duration};
use uuid::Uuid;

/// Test that the content of the registry is correctly loaded from the persistent store on start up.
#[tokio::test]
async fn bootstrap_registry() {
    let size = 15 * 1024 * 1024;
    let cluster = ClusterBuilder::builder()
        .with_rest(true)
        .with_pools(1)
        .with_replicas(1, size, transport::Protocol::None)
        .build()
        .await
        .unwrap();

    let client = cluster.rest_v00();

    let replica = client
        .replicas_api()
        .get_replica(&Cluster::replica(0, 0, 0))
        .await
        .unwrap();
    client
        .nexuses_api()
        .put_node_nexus(
            cluster.node(0).as_str(),
            &transport::NexusId::new(),
            models::CreateNexusBody::new(vec![replica.uri], size),
        )
        .await
        .expect("Failed to create nexus");

    // Get all resource specs.
    let specs = client
        .specs_api()
        .get_specs()
        .await
        .expect("Failed to get resource specs");

    // Restart the core agent with the expectation that the registry will have all its resource
    // specs loaded from the persistent store.
    cluster.restart_core().await;

    // Wait for core service to restart.
    cluster
        .node_service_liveness(None)
        .await
        .expect("Should have restarted by now");

    // Get the specs after the core agent has restarted and check that they match what was there
    // before.
    let restart_specs = client
        .specs_api()
        .get_specs()
        .await
        .expect("Failed to get resource specs after restart");
    assert_eq!(specs, restart_specs);
}

/// Test that store lease lock in the core agent works as expected
#[tokio::test]
async fn store_lease_lock() {
    // deploy etcd only...
    let _cluster = ClusterBuilder::builder()
        .with_rest(false)
        .with_jaeger(false)
        .with_io_engines(0)
        .with_agents(vec![])
        .build()
        .await
        .unwrap();

    let lease_ttl = std::time::Duration::from_secs(2);
    let _core_agent = Etcd::new_leased(
        ["0.0.0.0:2379"],
        ControlPlaneService::CoreAgent.to_string(),
        lease_ttl,
    )
    .await
    .unwrap();

    let mut store = Client::connect(["0.0.0.0:2379"], None)
        .await
        .expect("Failed to connect to etcd.");

    let leases = store.leases().await.unwrap();
    let lease_id = leases.leases().first().unwrap().id();
    tracing::info!("lease_id: {:?}", lease_id);

    tokio::time::sleep(lease_ttl).await;

    let mut etcd = Etcd::new("0.0.0.0:2379").await.unwrap();
    let svc = ControlPlaneService::CoreAgent;
    let obj: StoreLeaseOwner = etcd
        .get_obj(&StoreLeaseOwnerKey::new(&svc))
        .await
        .expect("Should exist!");
    tracing::info!("EtcdLeaseOwnerKey: {:?}", obj);
    assert_eq!(
        obj.lease_id(),
        format!("{lease_id:x}"),
        "Lease should be the same!"
    );

    let _core_agent2 = Etcd::new_leased(
        ["0.0.0.0:2379"],
        ControlPlaneService::CoreAgent.to_string(),
        lease_ttl,
    )
    .await
    .expect_err("One core-agent is already running!");
}

/// Test that store lease lock works as expected
#[tokio::test]
async fn core_agent_lease_lock() {
    let lease_ttl = std::time::Duration::from_secs(2);
    let lease_ttl_wait = lease_ttl + std::time::Duration::from_secs(1);
    let cluster = ClusterBuilder::builder()
        .with_io_engines(1)
        .with_store_lease_ttl(lease_ttl)
        .build()
        .await
        .unwrap();

    let mut store = Client::connect(["0.0.0.0:2379"], None)
        .await
        .expect("Failed to connect to etcd.");

    let leases = store.leases().await.unwrap();
    let lease_id = leases.leases().first().unwrap().id();
    tracing::info!("lease_id: {:?}", lease_id);

    tokio::time::sleep(lease_ttl).await;

    let mut etcd = Etcd::new("0.0.0.0:2379").await.unwrap();
    let svc = ControlPlaneService::CoreAgent;
    let obj: StoreLeaseOwner = etcd
        .get_obj(&StoreLeaseOwnerKey::new(&svc))
        .await
        .expect("Should exist!");
    tracing::info!("EtcdLeaseOwnerKey: {:?}", obj);
    assert_eq!(
        obj.lease_id(),
        format!("{lease_id:x}"),
        "Lease should be the same!"
    );

    let _core_agent2 = Etcd::new_leased(
        ["0.0.0.0:2379"],
        ControlPlaneService::CoreAgent.to_string(),
        lease_ttl,
    )
    .await
    .expect_err("One core-agent is already running!");

    // pause the core agent
    cluster.composer().pause("core").await.unwrap();
    // let its lease expire
    tokio::time::sleep(lease_ttl_wait).await;

    let leases = store.leases().await.unwrap();
    tracing::info!("Leases: {:?}", leases);
    assert!(leases.leases().is_empty());

    // bring back the core-agent which should be able to reestablish the lease since it hasn't lost
    // it to another core-agent instance
    cluster.composer().thaw("core").await.unwrap();

    tokio::time::sleep(lease_ttl_wait).await;

    let leases = store.leases().await.unwrap();
    let current_lease_id = leases.leases().first().unwrap().id();
    tracing::info!("lease_id: {:?}", current_lease_id);
    // it's the same lease as it's the same core-agent instance...
    assert_eq!(lease_id, current_lease_id);

    // pause the core agent
    cluster.composer().pause("core").await.unwrap();
    // let its lease expire
    tokio::time::sleep(lease_ttl_wait).await;

    let _core_agent2 = Etcd::new_leased(
        ["0.0.0.0:2379"],
        ControlPlaneService::CoreAgent.to_string(),
        lease_ttl,
    )
    .await
    .expect("First core-agent expired, the second one can now run!");

    let leases = store.leases().await.unwrap();
    let current_lease_id = leases.leases().first().unwrap().id();
    tracing::info!("lease_id: {:?}", current_lease_id);
    // it's a new lease, from the new core-agent instance
    assert_ne!(lease_id, current_lease_id);

    // unpause the core agent
    cluster.composer().thaw("core").await.unwrap();
    // it should not be able to regain the lock and will panic!

    tokio::time::sleep(lease_ttl_wait).await;
    let core = cluster.composer().inspect("core").await.unwrap();
    tracing::info!("core: {:?}", core.state);
    assert_eq!(Some(false), core.state.unwrap().running);
}

const OLD_VOLUME_PREFIX: &str = "/namespace/default/control-plane/VolumeSpec";

#[tokio::test]
async fn etcd_pagination() {
    let lease_ttl = std::time::Duration::from_secs(2);
    let cluster = ClusterBuilder::builder()
        .with_io_engines(0)
        .with_rest(false)
        .with_jaeger(false)
        .with_store_lease_ttl(lease_ttl)
        .with_req_timeouts(Duration::from_millis(200), Duration::from_millis(200))
        .build()
        .await
        .unwrap();

    let mut etcd = Etcd::new("0.0.0.0:2379").await.unwrap();

    let node_prefix = key_prefix_obj(StorableObjectType::NodeSpec, ApiVersion::V0);
    let volume_prefix = key_prefix_obj(StorableObjectType::VolumeSpec, ApiVersion::V0);

    // Persist some nodes in etcd.
    for i in 1..11 {
        let key = format!("{}/node{}", node_prefix, i);
        let json_str = format!(
            r#"{{"id":"mayastor-node{}","endpoint":"136.144.51.107:10124","labels":{{}}}}"#,
            i
        );
        let value = Value::from_str(&json_str).unwrap();
        etcd.put_kv(&key, &value).await.unwrap();
    }

    // Persist some volumes in new keyspace in etcd.
    for _i in 1..4 {
        let uuid = Uuid::new_v4();
        let key = format!("{}/{}", volume_prefix, uuid);
        let json_str = r#"{"uuid":"456122b1-7e19-4148-a890-579ca785a119","size":2147483648,"labels":{"local":"true"},"num_replicas":3,"status":{"Created":"Online"},"target":{"node":"mayastor-node4","nexus":"d6ccbb97-d13e-4ffb-91a0-c7607bb01f8f","protocol":"nvmf"},"policy":{"self_heal":true},"topology":{"node":{"Explicit":{"allowed_nodes":["mayastor-node2","mayastor-master","mayastor-node3","mayastor-node1","mayastor-node4"],"preferred_nodes":["mayastor-node2","mayastor-node3","mayastor-node4","mayastor-master","mayastor-node1"]}},"pool":{"Labelled":{"exclusion":{},"inclusion":{"org.com/created-by":"msp-operator"}}}},"last_nexus_id":"d6ccbb97-d13e-4ffb-91a0-c7607bb01f8f","operation":null}"#;
        let value = Value::from_str(json_str).unwrap();
        etcd.put_kv(&key, &value).await.unwrap();
    }

    // Persist some volumes in old key space in etcd.
    for _i in 1..6 {
        let uuid = Uuid::new_v4();
        let key = format!("{}/{}", OLD_VOLUME_PREFIX, uuid);
        let json_str = r#"{"uuid":"456122b1-7e19-4148-a890-579ca785a119","size":2147483648,"labels":{"local":"true"},"num_replicas":3,"status":{"Created":"Online"},"target":{"node":"mayastor-node4","nexus":"d6ccbb97-d13e-4ffb-91a0-c7607bb01f8f","protocol":"nvmf"},"policy":{"self_heal":true},"topology":{"node":{"Explicit":{"allowed_nodes":["mayastor-node2","mayastor-master","mayastor-node3","mayastor-node1","mayastor-node4"],"preferred_nodes":["mayastor-node2","mayastor-node3","mayastor-node4","mayastor-master","mayastor-node1"]}},"pool":{"Labelled":{"exclusion":{},"inclusion":{"org.com/created-by":"msp-operator"}}}},"last_nexus_id":"d6ccbb97-d13e-4ffb-91a0-c7607bb01f8f","operation":null}"#;
        let value = Value::from_str(json_str).unwrap();
        etcd.put_kv(&key, &value).await.unwrap();
    }

    // Persist some nexus info in old key space in etcd.
    for _i in 1..6 {
        let uuid = Uuid::new_v4();
        let key = format!("{}", uuid);
        let json_str = r#"{"children":[{"healthy":true,"uuid":"82779efa-a0c7-4652-a37b-83eefd894714"},{"healthy":true,"uuid":"2d98fa96-ac12-40be-acdc-e3559c0b1530"},{"healthy":true,"uuid":"620ff519-419a-48d6-97a8-c1ba3260d87e"}],"clean_shutdown":false}"#;
        let value = Value::from_str(json_str).unwrap();
        etcd.put_kv(&key, &value).await.unwrap();
    }

    // There Should be exactly 10 Nodes.
    let node_kvs = etcd.get_values_paged_all(&node_prefix, 3).await.unwrap();
    assert_eq!(node_kvs.len(), 10);

    // There Should be exactly 5 New Volumes.
    let volume_kvs = etcd.get_values_paged_all(&volume_prefix, 3).await.unwrap();
    assert_eq!(volume_kvs.len(), 3);

    // There Should be exactly 5 Old Volumes.
    let volume_kvs = etcd
        .get_values_paged_all(OLD_VOLUME_PREFIX, 3)
        .await
        .unwrap();
    assert_eq!(volume_kvs.len(), 5);

    cluster.restart_core().await;
    cluster
        .volume_service_liveness(None)
        .await
        .expect("Should have restarted by now");

    // There Should be exactly 10 New Volumes, after the migration.
    let volume_kvs_all = etcd.get_values_paged_all(&volume_prefix, 3).await.unwrap();
    assert_eq!(volume_kvs_all.len(), 8);

    let all = etcd.get_values_paged_all("", 3).await.unwrap();
    assert_eq!(all.len(), 26);
}
