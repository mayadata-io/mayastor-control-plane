use deployer_cluster::{Cluster, ClusterBuilder};
use grpc::operations::{
    nexus::traits::NexusOperations, pool::traits::PoolOperations,
    replica::traits::ReplicaOperations,
};
use openapi::clients::tower::hyper::http::uri;
use std::{
    convert::{From, TryFrom, TryInto},
    str::FromStr,
};
use stor_port::types::v0::{
    store::{nexus::ReplicaUri, nexus_child::NexusChild},
    transport as v0,
    transport::{Filter, NexusNvmePreemption, NvmeNqn, NvmeReservation, ShareReplica},
};

#[tokio::test]
async fn preservation() {
    let size = 20 * 1024 * 1024;
    let cluster = ClusterBuilder::builder()
        .with_io_engines(3)
        .with_tmpfs_pool(size * 2)
        .with_replicas(1, size, v0::Protocol::None)
        .with_options(|o| {
            o.with_io_engine_env("NEXUS_NVMF_RESV_ENABLE", "1")
                .with_fio_spdk(true)
        })
        .with_reconcile_period(
            std::time::Duration::from_millis(250),
            std::time::Duration::from_millis(250),
        )
        .build()
        .await
        .unwrap();
    let nexus_client = cluster.grpc_client().nexus();
    let replica_client = cluster.grpc_client().replica();
    let replica = replica_client
        .get(Filter::Replica(Cluster::replica(0, 0, 0)), None)
        .await;
    let replica = replica.unwrap().0.first().cloned().unwrap();

    let replica_uri = replica_client
        .share(&ShareReplica::from(&replica), None)
        .await
        .unwrap();

    let nexus1 = nexus_client
        .create(
            &v0::CreateNexus {
                node: cluster.node(1),
                uuid: "d40567ef-f373-4472-bd23-178c450f26db".try_into().unwrap(),
                size,
                children: vec![NexusChild::Replica(ReplicaUri::new(
                    &replica.uuid,
                    &replica_uri.clone().into(),
                ))],
                config: Some(v0::NexusNvmfConfig::new(
                    v0::NvmfControllerIdRange::default(),
                    0x12345678911,
                    NvmeReservation::ExclusiveAccess,
                    NexusNvmePreemption::Holder,
                )),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    let share = v0::ShareNexus::new(&nexus1, v0::NexusShareProtocol::Nvmf, vec![]);
    let _nexus1_uri = nexus_client.share(&share, None).await.unwrap();

    nexus_client
        .destroy(&v0::DestroyNexus::from(nexus1), None)
        .await
        .unwrap();

    // restart replica node
    let rep_node = replica.node.as_str();
    cluster.composer().stop(rep_node).await.unwrap();
    assert!(wait_pool_state(&cluster, cluster.pool(0, 0), false, 10).await);
    cluster.composer().start(rep_node).await.unwrap();
    // wait until pool is loaded
    assert!(wait_pool_state(&cluster, cluster.pool(0, 0), true, 20).await);

    let error = nexus_client
        .create(
            &v0::CreateNexus {
                node: cluster.node(2),
                uuid: "d40567ef-f373-4472-bd23-178c450f26da".try_into().unwrap(),
                size,
                children: vec![NexusChild::Replica(ReplicaUri::new(
                    &replica.uuid,
                    &replica_uri.into(),
                ))],
                config: Some(v0::NexusNvmfConfig::new(
                    v0::NvmfControllerIdRange::default(),
                    0x12345678912,
                    NvmeReservation::ExclusiveAccess,
                    // wrong preempt key to check if the previous holder is still there
                    NexusNvmePreemption::ArgKey(Some(0x12345678910)),
                )),
                ..Default::default()
            },
            None,
        )
        .await
        .expect_err("Old nexus still holds reservation even after replica power loss!");

    // Sadly io-engine returns error internal so we cannot type check to make sure the error
    // is caused by trying to preempt the wrong reservation key :(
    assert!(error
        .to_string()
        .contains("Failed to acquire write exclusive reservation"));
}

#[tokio::test]
async fn reservation() {
    let size = 20 * 1024 * 1024;
    let cluster = ClusterBuilder::builder()
        .with_io_engines(3)
        .with_tmpfs_pool(size * 2)
        .with_replicas(1, size, v0::Protocol::Nvmf)
        .with_options(|o| {
            o.with_io_engine_env("NEXUS_NVMF_RESV_ENABLE", "1")
                .with_fio_spdk(true)
        })
        .build()
        .await
        .unwrap();
    let nexus_client = cluster.grpc_client().nexus();

    let replica = cluster
        .rest_v00()
        .replicas_api()
        .get_replica(&Cluster::replica(2, 0, 0))
        .await
        .unwrap();
    let replica_uri = replica.uri.clone();

    let nexus1 = nexus_client
        .create(
            &v0::CreateNexus {
                node: cluster.node(0),
                uuid: v0::NexusId::new(),
                size,
                children: vec![replica_uri.clone().into()],
                config: Some(v0::NexusNvmfConfig::new(
                    v0::NvmfControllerIdRange::default(),
                    0x12345678911,
                    NvmeReservation::ExclusiveAccess,
                    NexusNvmePreemption::ArgKey(None),
                )),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    let share = v0::ShareNexus::new(&nexus1, v0::NexusShareProtocol::Nvmf, vec![]);
    let nexus1_uri = nexus_client.share(&share, None).await.unwrap();
    let nexus1_url = uri::Uri::from_str(&nexus1_uri).unwrap();
    let nqn = NvmeNqn::try_from(nexus1_url).unwrap().nqn_prefix().unwrap();
    let nexus_1_ip = cluster.composer().container_ip(cluster.node(0).as_str());
    let fio_builder = |nqn: &str, ip: &str, uuid: &str| {
        let nqn = format!("{nqn}\\:{uuid}");
        let filename =
            format!("--filename=trtype=tcp adrfam=IPv4 traddr={ip} trsvcid=8420 subnqn={nqn} ns=1");
        tracing::debug!("Filename: {filename}");
        vec![
            "fio",
            "--name=benchtest",
            filename.as_str(),
            "--direct=1",
            "--rw=randwrite",
            "--ioengine=spdk",
            "--bs=64k",
            "--iodepth=64",
            "--numjobs=1",
            "--thread=1",
            "--size=8M",
            "--norandommap=1",
        ]
        .into_iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
    };
    let fio_cmd = fio_builder(&nqn, &nexus_1_ip, nexus1.uuid.as_str());
    let (code, out) = cluster.composer().exec("fio-spdk", fio_cmd).await.unwrap();
    tracing::info!("{}", out);
    assert_eq!(code, Some(0));

    let nexus2_uuid = v0::NexusId::new();
    let nexus2 = nexus_client
        .create(
            &v0::CreateNexus {
                node: cluster.node(1),
                uuid: nexus2_uuid.clone(),
                size,
                children: vec![replica_uri.into()],
                config: Some(v0::NexusNvmfConfig::new(
                    v0::NvmfControllerIdRange::default(),
                    nexus2_uuid.as_u128() as u64,
                    NvmeReservation::ExclusiveAccess,
                    NexusNvmePreemption::Holder,
                )),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    let share = v0::ShareNexus::new(&nexus2, v0::NexusShareProtocol::Nvmf, vec![]);
    let _nexus2_uri = nexus_client.share(&share, None).await.unwrap();

    let nexus_2_ip = cluster.composer().container_ip(cluster.node(1).as_str());
    let fio_cmd = fio_builder(&nqn, &nexus_2_ip, nexus2.uuid.as_str());
    let (code, out) = cluster.composer().exec("fio-spdk", fio_cmd).await.unwrap();
    tracing::info!("{}", out);
    assert_eq!(code, Some(0));

    let fio_cmd = fio_builder(&nqn, &nexus_1_ip, nexus1.uuid.as_str());
    let (code, out) = cluster.composer().exec("fio-spdk", fio_cmd).await.unwrap();
    assert_eq!(code, Some(1), "{out}");
}

async fn wait_pool_state(
    cluster: &Cluster,
    pool: v0::PoolId,
    present: bool,
    max_tries: i32,
) -> bool {
    for _ in 1..max_tries {
        let filter = Filter::Pool(pool.clone());
        if let Ok(pools) = cluster.grpc_client().pool().get(filter, None).await {
            if pools
                .into_inner()
                .into_iter()
                .any(|p| p.state().map(|s| s.id == pool).unwrap_or_default())
                == present
            {
                return true;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
    false
}
