use crate::infra::{async_trait, Builder, ComponentAction, ComposeTest, Error, Etcd, StartOptions};
use composer::{Binary, ContainerSpec};
use stor_port::pstor::etcd::Etcd as EtcdStore;

#[async_trait]
impl ComponentAction for Etcd {
    fn configure(&self, options: &StartOptions, cfg: Builder) -> Result<Builder, Error> {
        Ok(if !options.no_etcd {
            let container_spec = ContainerSpec::from_binary(
                "etcd",
                Binary::from_path("etcd").with_args(vec![
                    "--data-dir",
                    "/tmp/etcd-data",
                    "--advertise-client-urls",
                    "http://[::]:2379",
                    "--listen-client-urls",
                    "http://[::]:2379",
                    // these ensure fast startup since it's not a cluster anyway
                    "--heartbeat-interval=1",
                    "--election-timeout=5",
                ]),
            )
            .with_portmap("2379", "2379")
            .with_portmap("2380", "2380");

            #[cfg(target_arch = "aarch64")]
            let container_spec = container_spec.with_env("ETCD_UNSUPPORTED_ARCH", "arm64");
            cfg.add_container_spec(container_spec)
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
            let _store = EtcdStore::new("[::]:2379")
                .await
                .expect("Failed to connect to etcd.");
        }
        Ok(())
    }
}
