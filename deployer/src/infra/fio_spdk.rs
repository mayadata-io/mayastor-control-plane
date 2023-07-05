use crate::infra::{
    async_trait, Builder, ComponentAction, ComposeTest, Error, FioSpdk, StartOptions,
};
use composer::ContainerSpec;

#[async_trait]
impl ComponentAction for FioSpdk {
    fn configure(&self, options: &StartOptions, cfg: Builder) -> Result<Builder, Error> {
        Ok(if options.fio_spdk {
            cfg.add_container_spec(
                ContainerSpec::from_image("fio-spdk", &utils::fio_spdk_image())
                    .with_entrypoint("tini")
                    .with_bind("/var/run/dpdk", "/var/run/dpdk")
                    .with_bind("/dev/vfio/vfio", "/dev/vfio/vfio")
                    .with_privileged(Some(true))
                    .with_args(vec!["--", "sleep", "infinity"]),
            )
        } else {
            cfg
        })
    }
    async fn start(&self, options: &StartOptions, cfg: &ComposeTest) -> Result<(), Error> {
        if options.fio_spdk {
            cfg.start("fio-spdk").await?;
        }
        Ok(())
    }

    async fn restart(&self, options: &StartOptions, cfg: &ComposeTest) -> Result<(), Error> {
        if options.fio_spdk {
            cfg.restart("fio-spdk").await?;
        }
        Ok(())
    }

    async fn wait_on(&self, _options: &StartOptions, _cfg: &ComposeTest) -> Result<(), Error> {
        // this is fine 🔥
        Ok(())
    }
}
