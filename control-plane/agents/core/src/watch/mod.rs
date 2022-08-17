mod service;
mod watches;

use super::controller::registry::Registry;
use grpc::operations::watch::server::WatchServer;
use std::sync::Arc;

/// Configure the Service and return the builder.
pub(crate) fn configure(builder: common::Service) -> common::Service {
    let registry = builder.shared_state::<Registry>().clone();
    let new_service = Arc::new(service::Service::new(registry));
    let watch_service = WatchServer::new(new_service);
    builder.with_shared_state(watch_service)
}

#[cfg(test)]
mod tests {
    use common_lib::{
        store::etcd::Etcd,
        types::v0::{
            openapi::models,
            store::definitions::{ObjectKey, Store},
            transport::{CreateVolume, Volume, VolumeId, WatchResourceId},
        },
    };
    use deployer_cluster::*;
    use grpc::operations::volume::traits::VolumeOperations;
    use once_cell::sync::OnceCell;
    use std::{net::SocketAddr, str::FromStr, time::Duration};
    use tokio::net::TcpStream;

    static CALLBACK: OnceCell<tokio::sync::mpsc::Sender<()>> = OnceCell::new();

    async fn setup_watch(
        client: &dyn VolumeOperations,
    ) -> (Volume, tokio::sync::mpsc::Receiver<()>) {
        let volume = client
            .create(
                &CreateVolume {
                    uuid: VolumeId::new(),
                    size: 10 * 1024 * 1024,
                    replicas: 1,
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();

        let (s, r) = tokio::sync::mpsc::channel(1);
        CALLBACK.set(s).unwrap();

        async fn notify() -> actix_web::HttpResponse {
            CALLBACK.get().cloned().unwrap().send(()).await.unwrap();
            actix_web::HttpResponse::Ok().finish()
        }

        actix_rt::spawn(async move {
            let _ = actix_web::HttpServer::new(|| {
                actix_web::App::new().service(
                    actix_web::web::resource("/test").route(actix_web::web::put().to(notify)),
                )
            })
            .bind("10.1.0.1:8082")
            .unwrap()
            .workers(1)
            .run()
            .await;
        });

        // wait until the "callback" server is running
        callback_server_liveness("10.1.0.1:8082").await;

        (volume, r)
    }

    async fn callback_server_liveness(uri: &str) {
        let sa = SocketAddr::from_str(uri).unwrap();
        for _ in 0 .. 25 {
            if TcpStream::connect(&sa).await.is_ok() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        TcpStream::connect(&sa).await.unwrap();
    }

    #[actix_rt::test]
    async fn watch() {
        let cluster = ClusterBuilder::builder().with_pools(1).build().await;
        let cluster = cluster.unwrap();
        let client = cluster.rest_v00();
        let client = client.watches_api();
        let volume_client = cluster.grpc_client().volume();

        let (volume, mut callback_ch) = setup_watch(&volume_client).await;

        let watch_volume = WatchResourceId::Volume(volume.spec().uuid);
        let callback = url::Url::parse("http://10.1.0.1:8082/test").unwrap();

        let watches = client.get_watch_volume(&volume.spec().uuid).await.unwrap();
        assert!(watches.is_empty());

        let mut store = Etcd::new("0.0.0.0:2379")
            .await
            .expect("Failed to connect to etcd.");

        client
            .put_watch_volume(&volume.spec().uuid, callback.as_str())
            .await
            .expect_err("volume does not exist in the store");

        store
            .put_kv(&watch_volume.key(), &serde_json::json!("aaa"))
            .await
            .unwrap();

        client
            .put_watch_volume(&volume.spec().uuid, callback.as_str())
            .await
            .unwrap();

        let watches = client.get_watch_volume(&volume.spec().uuid).await.unwrap();
        assert_eq!(
            watches.first(),
            Some(&models::RestWatch {
                resource: watch_volume.to_string(),
                callback: callback.to_string(),
            })
        );
        assert_eq!(watches.len(), 1);

        store
            .put_kv(&watch_volume.key(), &serde_json::json!("aaa"))
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_millis(250), callback_ch.recv())
            .await
            .unwrap();

        client
            .del_watch_volume(&volume.spec().uuid, callback.as_str())
            .await
            .unwrap();

        store
            .put_kv(&watch_volume.key(), &serde_json::json!("bbb"))
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_millis(250), callback_ch.recv())
            .await
            .expect_err("should have been deleted so no callback");

        let watches = client.get_watch_volume(&volume.spec().uuid).await.unwrap();
        assert!(watches.is_empty());
    }
}
