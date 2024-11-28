#[cfg(test)]
use crate::resources::utils::{print_table, CreateRows, GetHeaderRow, OutputFormat};
use deployer_cluster::{Cluster, ClusterBuilder};
use gag::BufferRedirect;
use once_cell::sync::OnceCell;
use openapi::{
    apis::Uuid,
    models::{CreateVolumeBody, NodeState, PoolState, VolumeSpec, VolumeState},
};
use serde::ser;
use std::io::Read;

static CLUSTER: OnceCell<std::sync::Mutex<Option<std::sync::Arc<Cluster>>>> = OnceCell::new();
const VOLUME_UUID: &str = "1e3cf927-80c2-47a8-adf0-95c486bdd7b7";

async fn setup() {
    let cluster = ClusterBuilder::builder()
        .with_rest(true)
        .with_io_engines(1)
        .with_pools(1)
        .build()
        .await
        .unwrap();

    cluster
        .rest_v00()
        .volumes_api()
        .put_volume(
            &Uuid::parse_str(VOLUME_UUID).unwrap(),
            CreateVolumeBody {
                policy: Default::default(),
                replicas: 1,
                size: 5242880,
                topology: None,
                labels: None,
                thin: false,
                affinity_group: None,
                max_snapshots: None,
            },
        )
        .await
        .unwrap();

    CLUSTER
        .set(std::sync::Mutex::new(Some(std::sync::Arc::new(cluster))))
        .ok()
        .expect("Expect to be initialised only once");
}

async fn cluster() -> std::sync::Arc<Cluster> {
    if CLUSTER.get().is_none() {
        setup().await;
        extern "C" fn cleanup_cluster() {
            if let Some(initialised) = CLUSTER.get() {
                let mut cluster = initialised.lock().expect("poisoned").take();
                // the cluster is cleaned up on Drop
                let _ = cluster.take();
            }
        }
        shutdown_hooks::add_shutdown_hook(cleanup_cluster);
    }
    CLUSTER
        .get()
        .expect("Initialised")
        .lock()
        .expect("Not poisoned")
        .as_ref()
        .expect("not None")
        .clone()
}

#[tokio::test]
async fn get_volumes() {
    let volumes = cluster()
        .await
        .rest_v00()
        .volumes_api()
        .get_volumes(0, None, None)
        .await
        .unwrap();
    let volume_state = volumes.entries[0].state.clone();
    let volume_spec = volumes.entries[0].spec.clone();
    compare(volume_output(volume_spec, volume_state), volumes.entries);
}

#[tokio::test]
async fn get_volumes_paginated() {
    let volume_uuids = [
        VOLUME_UUID,
        "81fb45d2-23e8-430d-9bbd-0bbce5b0c040",
        "ed10e8bd-bcfc-48cc-987c-5e5a5ebeb1ff",
    ];

    // Create an additional 2 volumes. We do not need to create the first volume because this is
    // already created by the setup code.
    for uuid in volume_uuids[1..=2].iter() {
        cluster()
            .await
            .rest_v00()
            .volumes_api()
            .put_volume(
                &Uuid::parse_str(uuid).unwrap(),
                CreateVolumeBody {
                    policy: Default::default(),
                    replicas: 1,
                    size: 5242880,
                    thin: false,
                    topology: None,
                    labels: None,
                    affinity_group: None,
                    max_snapshots: None,
                },
            )
            .await
            .unwrap();
    }

    let num_volumes = cluster()
        .await
        .rest_v00()
        .volumes_api()
        .get_volumes(0, None, None)
        .await
        .unwrap()
        .entries
        .len();

    assert_eq!(num_volumes, volume_uuids.len());

    // Get a single entry at a time.
    let max_entries = 1;
    let mut starting_token = Some(0);

    for uuid in volume_uuids {
        let volumes = cluster()
            .await
            .rest_v00()
            .volumes_api()
            .get_volumes(max_entries, None, starting_token)
            .await
            .unwrap();
        // The number of returned volumes should be equal to the number of specified max entries.
        assert_eq!(volumes.entries.len(), max_entries as usize);
        assert_eq!(volumes.entries[0].spec.uuid.to_string(), uuid);
        starting_token = volumes.next_token;
    }
}

#[tokio::test]
async fn get_volume() {
    let volume = cluster()
        .await
        .rest_v00()
        .volumes_api()
        .get_volume(&Uuid::parse_str(VOLUME_UUID).unwrap())
        .await
        .unwrap();
    let volume_state = volume.state.clone();
    let volume_spec = volume.spec.clone();
    compare(volume_output(volume_spec, volume_state), volume);
}

#[tokio::test]
async fn get_snapshots() {
    let snap_uuids = [
        "12fb45d2-23e8-430d-9bbd-0bbce5b0c040",
        "22fb45d2-23e8-430d-9bbd-0bbce5b0c040",
        "32fb45d2-23e8-430d-9bbd-0bbce5b0c040",
        "42fb45d2-23e8-430d-9bbd-0bbce5b0c040",
    ];

    let test_cluster = cluster().await;
    let vol = test_cluster
        .rest_v00()
        .volumes_api()
        .get_volume(&Uuid::parse_str(VOLUME_UUID).unwrap())
        .await
        .expect("Volume not found");

    // Create four snapshots
    for snapid in snap_uuids[0..=3].iter() {
        // Create four snapshots.
        test_cluster
            .rest_v00()
            .snapshots_api()
            .put_volume_snapshot(&vol.spec.uuid, &Uuid::parse_str(snapid).unwrap())
            .await
            .expect("Snapshot creation failed");
    }

    // Get all snapshots - non paginated.
    let snapshots = test_cluster
        .rest_v00()
        .snapshots_api()
        .get_volumes_snapshots(0, None, None, Some(0))
        .await
        .expect("Listing the snapshots failed");
    assert_eq!(snapshots.entries.len(), snap_uuids.len());

    // Get all snapshots by volume id - non paginated.
    let snapshots = test_cluster
        .rest_v00()
        .snapshots_api()
        .get_volumes_snapshots(0, None, Some(&vol.spec.uuid), Some(0))
        .await
        .expect("Listing the snapshots failed");
    assert_eq!(snapshots.entries.len(), snap_uuids.len());

    // Get one snapshot by snapshot id.
    let snapid = Uuid::parse_str(snap_uuids[0]).ok().unwrap();
    let snapshot = test_cluster
        .rest_v00()
        .snapshots_api()
        .get_volumes_snapshots(0, Some(&snapid), None, Some(0))
        .await
        .expect("Listing the snapshots failed");
    assert_eq!(snapshot.entries.len(), 1);
    assert_eq!(
        snapshot.entries.first().unwrap().definition.spec.uuid,
        snapid
    );

    // Get snapshots - paginated.
    // Get max_entries at a time, and num_iter times.
    // At the end, total_listed must be equal to total number of
    // snapshots created earlier.
    let max_entries = 1;
    let mut starting_token = Some(0);
    let mut total_listed = 0;
    while starting_token.is_some() {
        let snapshots = test_cluster
            .rest_v00()
            .snapshots_api()
            .get_volumes_snapshots(max_entries, None, None, starting_token)
            .await
            .expect("Listing the snapshots failed");
        assert_eq!(snapshots.entries.len(), max_entries as usize);
        total_listed += snapshots.entries.len();
        starting_token = snapshots.next_token;
    }
    assert_eq!(total_listed, snap_uuids.len());
}

#[tokio::test]
async fn get_pools() {
    let pools = cluster()
        .await
        .rest_v00()
        .pools_api()
        .get_pools(None)
        .await
        .unwrap();
    let pool_state = pools[0].state.as_ref().unwrap().clone();
    compare(pool_output(pool_state), pools);
}

#[tokio::test]
async fn get_pool() {
    let pool = cluster()
        .await
        .rest_v00()
        .pools_api()
        .get_pool(cluster().await.pool(0, 0).as_str())
        .await
        .unwrap();
    let pool_state = pool.state.as_ref().unwrap().clone();
    compare(pool_output(pool_state), pool);
}

#[tokio::test]
async fn get_nodes() {
    let nodes = cluster()
        .await
        .rest_v00()
        .nodes_api()
        .get_nodes(None)
        .await
        .unwrap();
    let node_state = nodes[0].state.as_ref().unwrap().clone();
    compare(node_output(node_state), nodes);
}

#[tokio::test]
async fn get_node() {
    let node = cluster()
        .await
        .rest_v00()
        .nodes_api()
        .get_node(cluster().await.node(0).as_str())
        .await
        .unwrap();
    let node_state = node.state.as_ref().unwrap().clone();
    compare(node_output(node_state), node);
}

#[tokio::test]
async fn get_replica_topology() {
    let replica_topo = cluster()
        .await
        .rest_v00()
        .volumes_api()
        .get_volume(&Uuid::parse_str(VOLUME_UUID).unwrap())
        .await
        .unwrap()
        .state
        .replica_topology;
    let replica_ids: Vec<String> = replica_topo.clone().into_keys().collect();
    let replica = replica_topo.get(&*replica_ids[0]).unwrap();
    compare(
        format!(
            " {:id_width$}{:node_width$}{:pool_width$}STATUS \n",
            "ID",
            "NODE",
            "POOL",
            id_width = 38,
            node_width = replica.node.as_ref().unwrap().len() + 2,
            pool_width = replica.pool.as_ref().unwrap().len() + 2
        ) + &*format!(
            " {}  {}  {}  {} \n",
            replica_ids[0],
            replica.node.as_ref().unwrap(),
            replica.pool.as_ref().unwrap(),
            replica.state.to_string()
        ),
        replica_topo,
    );
}

// Compares the print_table output redirected to buffer with the expected string
fn compare<T>(expected_output: String, obj: T)
where
    T: ser::Serialize,
    T: CreateRows,
    T: GetHeaderRow,
{
    let mut buf = BufferRedirect::stdout().unwrap();
    print_table(&OutputFormat::None, obj);
    let mut actual_output = String::new();
    buf.read_to_string(&mut actual_output).unwrap();
    assert_eq!(&actual_output[..], expected_output);
}

fn pool_output(pool_state: PoolState) -> String {
    let disks: String = pool_state.disks.join(", ");
    format!(
        " {:id_width$}TOTAL CAPACITY  USED CAPACITY  {:disk_width$}{:node_width$}STATUS  MANAGED \n",
        "ID",
        "DISKS",
        "NODE",
        id_width = pool_state.id.len() + 2,
        disk_width = disks.len() + 2,
        node_width = pool_state.node.len() + 2
    ) + &*format!(
        " {}  {}       {}        {}  {}  {}  {} \n",
        pool_state.id,
        pool_state.capacity,
        pool_state.used,
        disks,
        pool_state.node,
        pool_state.status.to_string(),
        true
    )
}

fn volume_output(volume_spec: VolumeSpec, volume_state: VolumeState) -> String {
    format!(
        " {:width$}REPLICAS  TARGET-NODE  ACCESSIBILITY  STATUS  SIZE \n",
        "ID",
        width = 38
    ) + &*format!(
        " {}  {}         {}       {}         {}  {} \n",
        volume_state.uuid,
        volume_spec.num_replicas,
        "<none>",
        "<none>",
        volume_state.status.to_string(),
        volume_state.size
    )
}

fn node_output(node_state: NodeState) -> String {
    format!(
        " {:width_id$}{:width_grpc$}STATUS \n",
        "ID",
        "GRPC ENDPOINT",
        width_id = node_state.id.len() + 2,
        width_grpc = node_state.grpc_endpoint.len() + 2
    ) + &*format!(
        " {}  {}  {} \n",
        node_state.id,
        node_state.grpc_endpoint,
        node_state.status.to_string()
    )
}
