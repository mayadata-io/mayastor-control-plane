use deployer_cluster::ClusterBuilder;
use grpc::operations::pool::traits::PoolOperations;
use openapi::models;
use stor_port::types::v0::transport as v0;

#[tokio::test]
async fn create_pool_malloc() {
    let cluster = ClusterBuilder::builder().build().await.unwrap();
    cluster
        .rest_v00()
        .pools_api()
        .put_node_pool(
            cluster.node(0).as_str(),
            cluster.pool(0, 0).as_str(),
            models::CreatePoolBody::new(vec!["malloc:///disk?size_mb=100"]),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn create_pool_with_missing_disk() {
    let cluster = ClusterBuilder::builder().build().await.unwrap();

    cluster
        .rest_v00()
        .pools_api()
        .put_node_pool(
            cluster.node(0).as_str(),
            cluster.pool(0, 0).as_str(),
            models::CreatePoolBody::new(vec!["/dev/c/3po"]),
        )
        .await
        .expect_err("Device should not exist");
}

#[tokio::test]
async fn create_pool_with_existing_disk() {
    let cluster = ClusterBuilder::builder().build().await.unwrap();

    cluster
        .rest_v00()
        .pools_api()
        .put_node_pool(
            cluster.node(0).as_str(),
            cluster.pool(0, 0).as_str(),
            models::CreatePoolBody::new(vec!["malloc:///disk?size_mb=100"]),
        )
        .await
        .unwrap();

    cluster
        .rest_v00()
        .pools_api()
        .put_node_pool(
            cluster.node(0).as_str(),
            cluster.pool(0, 0).as_str(),
            models::CreatePoolBody::new(vec!["malloc:///disk?size_mb=100"]),
        )
        .await
        .expect_err("Disk should be used by another pool");

    cluster
        .rest_v00()
        .pools_api()
        .del_pool(cluster.pool(0, 0).as_str())
        .await
        .unwrap();

    cluster
        .rest_v00()
        .pools_api()
        .put_node_pool(
            cluster.node(0).as_str(),
            cluster.pool(0, 0).as_str(),
            models::CreatePoolBody::new(vec!["malloc:///disk?size_mb=100"]),
        )
        .await
        .expect("Should now be able to create the new pool");
}

#[tokio::test]
async fn create_pool_idempotent() {
    let cluster = ClusterBuilder::builder().build().await.unwrap();
    let pool_client = cluster.grpc_client().pool();

    pool_client
        .create(
            &v0::CreatePool {
                node: cluster.node(0),
                id: cluster.pool(0, 0),
                disks: vec!["malloc:///disk?size_mb=100".into()],
                labels: None,
                encryption: None,
            },
            None,
        )
        .await
        .unwrap();

    pool_client
        .create(
            &v0::CreatePool {
                node: cluster.node(0),
                id: cluster.pool(0, 0),
                disks: vec!["malloc:///disk?size_mb=100".into()],
                labels: None,
                encryption: None,
            },
            None,
        )
        .await
        .expect_err("already exists");
}

/// FIXME: CAS-710
#[tokio::test]
async fn create_pool_idempotent_same_disk_different_query() {
    let cluster = ClusterBuilder::builder().build().await.unwrap();
    let pool_client = cluster.grpc_client().pool();
    pool_client
        .create(
            &v0::CreatePool {
                node: cluster.node(0),
                id: cluster.pool(0, 0),
                disks: vec!["malloc:///disk?size_mb=100&blk_size=512".into()],
                labels: None,
                encryption: None,
            },
            None,
        )
        .await
        .unwrap();

    pool_client
        .create(
            &v0::CreatePool {
                node: cluster.node(0),
                id: cluster.pool(0, 0),
                disks: vec!["malloc:///disk?size_mb=200&blk_size=4096".into()],
                labels: None,
                encryption: None,
            },
            None,
        )
        .await
        .expect_err("Different query not allowed!");
}

#[tokio::test]
async fn create_pool_idempotent_different_nvmf_host() {
    let cluster = ClusterBuilder::builder()
        .with_options(|opts| opts.with_io_engines(3))
        .build()
        .await
        .unwrap();
    let pool_client = cluster.grpc_client().pool();
    pool_client
        .create(
            &v0::CreatePool {
                node: cluster.node(1),
                id: cluster.pool(1, 0),
                disks: vec!["malloc:///disk?size_mb=100".into()],
                labels: None,
                encryption: None,
            },
            None,
        )
        .await
        .unwrap();

    pool_client
        .create(
            &v0::CreatePool {
                node: cluster.node(2),
                id: cluster.pool(2, 0),
                disks: vec!["malloc:///disk?size_mb=100".into()],
                labels: None,
                encryption: None,
            },
            None,
        )
        .await
        .unwrap();

    pool_client
        .create(
            &v0::CreatePool {
                node: cluster.node(2),
                id: cluster.pool(2, 0),
                disks: vec!["malloc:///disk?size_mb=100".into()],
                labels: None,
                encryption: None,
            },
            None,
        )
        .await
        .expect_err("Pool Already exists!");

    pool_client
        .create(
            &v0::CreatePool {
                node: cluster.node(2),
                id: cluster.pool(2, 0),
                disks: vec!["malloc:///disk?size_mb=100".into()],
                labels: None,
                encryption: None,
            },
            None,
        )
        .await
        .expect_err("Pool disk already used by another pool!");
}
