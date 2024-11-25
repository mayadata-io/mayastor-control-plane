use deployer_cluster::{Cluster, ClusterBuilder};
use events_api::{
    event::{EventAction, EventCategory, EventMessage},
    mbus_nats::{message_bus_init, BusSubscription},
    Bus,
};
use grpc::operations::{
    nexus::traits::NexusOperations, pool::traits::PoolOperations, volume::traits::VolumeOperations,
};
use std::{collections::HashMap, time::Duration};
use stor_port::types::v0::{
    openapi::{
        models,
        models::{CreateVolumeBody, PublishVolumeBody, VolumePolicy},
    },
    transport::{DestroyNexus, DestroyPool, DestroyVolume, NexusId, UnpublishVolume, VolumeId},
};
use tokio::time::timeout;

const CHILD_WAIT: Duration = Duration::from_millis(1);
const EVENT_CATEGORIES: &[EventCategory] = &[
    EventCategory::Volume,
    EventCategory::Pool,
    EventCategory::Nexus,
    EventCategory::Replica,
];
const EVENT_ACTIONS: &[EventAction] = &[
    EventAction::Create,
    EventAction::Delete,
    EventAction::RebuildBegin,
    EventAction::RebuildEnd,
    EventAction::SwitchOver,
    EventAction::AddChild,
    EventAction::RemoveChild,
    EventAction::OnlineChild,
];

async fn build_cluster(num_ioe: u32, pool_size: u64) -> Cluster {
    let reconcile_period = Duration::from_millis(100);
    ClusterBuilder::builder()
        .with_rest(true)
        .with_io_engines(num_ioe)
        .with_faulted_child_wait_period(CHILD_WAIT)
        .with_reconcile_period(reconcile_period, reconcile_period)
        .with_cache_period("250ms")
        .with_tmpfs_pool(pool_size)
        .with_options(|b| b.with_isolated_io_engine(true))
        .with_eventing(true)
        .build()
        .await
        .unwrap()
}

#[tokio::test]
async fn events() {
    let cluster = build_cluster(3, 52428800).await;

    let vol_target = cluster.node(0).to_string();
    let api_client = cluster.rest_v00();
    let volume_api = api_client.volumes_api();
    let nexus_client = cluster.grpc_client().nexus();

    let mut events_mbus_subscription =
        timeout(Duration::from_millis(20), mbus_init("localhost:4222"))
            .await
            .unwrap();

    for _ in 0..3 {
        pool_creation_event_test(&mut events_mbus_subscription).await;
    }

    let body = CreateVolumeBody::new(VolumePolicy::new(true), 2, 10485760u64, false);
    let volid = VolumeId::new();
    let volume = volume_api.put_volume(&volid, body).await.unwrap();

    volume_creation_event_test(&mut events_mbus_subscription).await;

    // publish volume
    let volume = volume_api
        .put_volume_target(
            &volume.spec.uuid,
            PublishVolumeBody::new_all(
                HashMap::new(),
                None,
                vol_target.clone().to_string(),
                models::VolumeShareProtocol::Nvmf,
                None,
                cluster.csi_node(0),
            ),
        )
        .await
        .expect("Failed to publish volume");

    // nexus is created
    nexus_creation_event_test(&mut events_mbus_subscription).await;

    let volume_state = volume.state.clone();
    let nexus = volume_state.target.unwrap();

    let replicas = api_client.replicas_api().get_replicas().await.unwrap();
    let testrep = replicas.iter().find(|r| r.node != vol_target).unwrap();
    let testrep_node = &testrep.node;
    tracing::info!(
        "Stopping node {testrep_node} having replica {}",
        testrep.uri
    );

    // Stop the container and don't restart.
    cluster
        .composer()
        .stop(&testrep_node.to_string())
        .await
        .expect("container stop failure");

    // nexus child removal event
    nexus_child_removal_event_test(&mut events_mbus_subscription).await;

    // replica is created and rebuild starts
    rebuild_begin_event_test(&mut events_mbus_subscription).await;

    // nexus child transitions to online state
    nexus_child_online_event_test(&mut events_mbus_subscription).await;

    // nexus child addition event
    nexus_child_addition_event_test(&mut events_mbus_subscription).await;

    // rebuild complete event
    rebuild_end_event_test(&mut events_mbus_subscription).await;

    let vol_client = cluster.grpc_client().volume();

    vol_client
        .unpublish(
            &UnpublishVolume::new(&volid, false, vec![cluster.csi_node(0).into()]),
            None,
        )
        .await
        .unwrap();

    nexus_client
        .destroy(
            &DestroyNexus::new(vol_target.into(), NexusId::from(nexus.uuid)),
            None,
        )
        .await
        .unwrap();

    nexus_deletion_event_test(&mut events_mbus_subscription).await;
    vol_client
        .destroy(&DestroyVolume { uuid: volid }, None)
        .await
        .expect("Should be able to destroy the volume");

    volume_deletion_event_test(&mut events_mbus_subscription).await;

    let pool_client = cluster.grpc_client().pool();

    pool_client
        .destroy(
            &DestroyPool {
                node: "io-engine-1".into(),
                id: "io-engine-1-pool-1".into(),
            },
            None,
        )
        .await
        .unwrap();

    pool_deletion_event_test(&mut events_mbus_subscription).await;
}
async fn pool_creation_event_test(sub: &mut BusSubscription<EventMessage>) {
    let pool_creation_message = timeout(Duration::from_millis(50), get_next_event(sub))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pool_creation_message.category(), EventCategory::Pool);
    assert_eq!(pool_creation_message.action(), EventAction::Create);
}

async fn pool_deletion_event_test(sub: &mut BusSubscription<EventMessage>) {
    let pool_deletion_message = timeout(Duration::from_millis(50), get_next_event(sub))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pool_deletion_message.category(), EventCategory::Pool);
    assert_eq!(pool_deletion_message.action(), EventAction::Delete);
}

async fn volume_creation_event_test(sub: &mut BusSubscription<EventMessage>) {
    let replica_creation_message = timeout(Duration::from_millis(50), get_next_event(sub))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replica_creation_message.category(), EventCategory::Replica);
    assert_eq!(replica_creation_message.action(), EventAction::Create);
    let replica_creation_message = timeout(Duration::from_millis(50), get_next_event(sub))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replica_creation_message.category(), EventCategory::Replica);
    assert_eq!(replica_creation_message.action(), EventAction::Create);
    let vol_creation_message = timeout(Duration::from_millis(50), get_next_event(sub))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(vol_creation_message.category(), EventCategory::Volume);
    assert_eq!(vol_creation_message.action(), EventAction::Create);
}

async fn volume_deletion_event_test(sub: &mut BusSubscription<EventMessage>) {
    let replica_deletion_message = timeout(Duration::from_millis(50), get_next_event(sub))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replica_deletion_message.category(), EventCategory::Replica);
    assert_eq!(replica_deletion_message.action(), EventAction::Delete);
    let replica_deletion_message = timeout(Duration::from_millis(50), get_next_event(sub))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replica_deletion_message.category(), EventCategory::Replica);
    assert_eq!(replica_deletion_message.action(), EventAction::Delete);
    let vol_deletion_message = timeout(Duration::from_millis(50), get_next_event(sub))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(vol_deletion_message.category(), EventCategory::Volume);
    assert_eq!(vol_deletion_message.action(), EventAction::Delete);
}

async fn nexus_creation_event_test(sub: &mut BusSubscription<EventMessage>) {
    let nexus_creation_message = timeout(Duration::from_millis(50), get_next_event(sub))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(nexus_creation_message.category(), EventCategory::Nexus);
    assert_eq!(nexus_creation_message.action(), EventAction::Create);
}

async fn nexus_deletion_event_test(sub: &mut BusSubscription<EventMessage>) {
    let nexus_deletion_message = timeout(Duration::from_millis(50), get_next_event(sub))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(nexus_deletion_message.category(), EventCategory::Nexus);
    assert_eq!(nexus_deletion_message.action(), EventAction::Delete);
}

async fn nexus_child_addition_event_test(sub: &mut BusSubscription<EventMessage>) {
    let nexus_add_child_event = timeout(Duration::from_millis(50), get_next_event(sub))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(nexus_add_child_event.category(), EventCategory::Nexus);
    assert_eq!(nexus_add_child_event.action(), EventAction::AddChild);
}

async fn nexus_child_removal_event_test(sub: &mut BusSubscription<EventMessage>) {
    let nexus_remove_child_event = timeout(Duration::from_millis(500), get_next_event(sub))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(nexus_remove_child_event.category(), EventCategory::Nexus);
    assert_eq!(nexus_remove_child_event.action(), EventAction::RemoveChild);
}

async fn nexus_child_online_event_test(sub: &mut BusSubscription<EventMessage>) {
    let nexus_online_child_event = timeout(Duration::from_millis(50), get_next_event(sub))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(nexus_online_child_event.category(), EventCategory::Nexus);
    assert_eq!(nexus_online_child_event.action(), EventAction::OnlineChild);
}

async fn rebuild_begin_event_test(sub: &mut BusSubscription<EventMessage>) {
    let replica_creation_message = timeout(Duration::from_millis(300), get_next_event(sub))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replica_creation_message.category(), EventCategory::Replica);
    assert_eq!(replica_creation_message.action(), EventAction::Create);
    let rebuid_start_event_message = timeout(Duration::from_millis(300), get_next_event(sub))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rebuid_start_event_message.category(), EventCategory::Nexus);
    assert_eq!(
        rebuid_start_event_message.action(),
        EventAction::RebuildBegin
    );
}

async fn rebuild_end_event_test(sub: &mut BusSubscription<EventMessage>) {
    let rebuid_end_event_message = timeout(Duration::from_millis(500), get_next_event(sub))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rebuid_end_event_message.category(), EventCategory::Nexus);
    assert_eq!(rebuid_end_event_message.action(), EventAction::RebuildEnd);
}

async fn mbus_init(mbus_url: &str) -> BusSubscription<EventMessage> {
    let mut bus = message_bus_init(mbus_url, Some(1)).await;
    bus.subscribe::<EventMessage>().await.unwrap()
}

// Filter the events to obtain one with component test.
async fn get_next_event(sub: &mut BusSubscription<EventMessage>) -> Option<EventMessage> {
    while let Some(event) = sub.next().await {
        if EVENT_CATEGORIES.contains(&event.category()) && EVENT_ACTIONS.contains(&event.action()) {
            return Some(event);
        }
    }
    None
}
