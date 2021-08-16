use super::*;
use common_lib::store::etcd::Etcd as EtcdStore;

#[async_trait]
impl ComponentAction for Etcd {
    fn configure(&self, options: &StartOptions, cfg: Builder) -> Result<Builder, Error> {
        Ok(if !options.no_etcd {
            cfg.add_container_spec(
                ContainerSpec::from_binary(
                    "etcd",
                    Binary::from_path("etcd").with_args(vec![
                        "--data-dir",
                        "/tmp/etcd-data",
                        "--advertise-client-urls",
                        "http://0.0.0.0:2379",
                        "--listen-client-urls",
                        "http://0.0.0.0:2379",
                    ]),
                )
                .with_portmap("2379", "2379")
                .with_portmap("2380", "2380"),
            )
        } else {
            cfg
        })
    }
    async fn start(&self, options: &StartOptions, cfg: &ComposeTest) -> Result<(), Error> {
        if !options.no_etcd {
            cfg.start("etcd").await?;
        }
        Ok(())
    }
    async fn wait_on(&self, options: &StartOptions, _cfg: &ComposeTest) -> Result<(), Error> {
        if !options.no_etcd {
            let _store = EtcdStore::new("0.0.0.0:2379")
                .await
                .expect("Failed to connect to etcd.");

            // TODO: Remove if CI/CD passes
            // if !store.online().await {
            //     // we seem to get in this situation on CI, let's log the result of a get key
            //     // in case the result will be helpful
            //     let result = store.get_kv(&"a".to_string()).await;
            //     panic!("etcd get_kv result: {:#?}", result);
            // }
            // assert!(store.online().await);
        }
        Ok(())
    }
}
