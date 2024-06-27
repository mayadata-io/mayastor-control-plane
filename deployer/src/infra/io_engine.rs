use crate::infra::{
    async_trait, Builder, ComponentAction, ComposeTest, Error, IoEngine, StartOptions,
};
use composer::{Binary, ContainerSpec};
use rpc::io_engine::{IoEngineApiVersion, RpcHandle};
use std::net::{IpAddr, SocketAddr};
use stor_port::types::v0::openapi::apis::Uuid;
use utils::DEFAULT_GRPC_CLIENT_ADDR;

#[async_trait]
impl ComponentAction for IoEngine {
    fn configure(&self, options: &StartOptions, cfg: Builder) -> Result<Builder, Error> {
        let mut cfg = cfg;
        for i in 0 .. options.io_engines + options.idle_io_engines {
            let io_engine_socket =
                format!("{}:10124", cfg.next_ip_for_name(&Self::name(i, options))?);
            let name = Self::name(i, options);
            let reg_name = Self::idle_name(i, options);

            let ptpl_dir = format!("{}/{}", Self::ptpl().1, name);

            let bin = utils::DATA_PLANE_BINARY;
            let binary = if i < options.io_engines {
                options.io_engine_bin.clone()
            } else {
                options.idle_io_engine_bin.clone()
            }
            .or_else(|| Self::binary(bin));

            let mut spec = if let Some(binary) = binary {
                ContainerSpec::from_binary(&name, Binary::from_path(&binary))
                    .with_bind_binary_dir(true)
            } else {
                let image = if i < options.io_engines {
                    &options.io_engine_image
                } else {
                    &options.idle_io_engine_image
                };
                ContainerSpec::from_image(&name, image)
                    .with_pull_policy(options.image_pull_policy.clone())
            }
            .with_args(vec!["-N", &reg_name])
            .with_args(vec!["-g", &io_engine_socket])
            .with_args(vec!["-R", DEFAULT_GRPC_CLIENT_ADDR])
            .with_args(vec![
                "--api-versions".to_string(),
                IoEngineApiVersion::vec_to_str(options.io_engine_api_versions.clone()),
            ])
            .with_args(vec!["-r", format!("/host/tmp/{}.sock", reg_name).as_str()])
            .with_args(vec!["--ptpl-dir", &ptpl_dir])
            .with_env("MAYASTOR_NVMF_HOSTID", Uuid::new_v4().to_string().as_str())
            .with_env("NEXUS_NVMF_RESV_ENABLE", "1")
            .with_env("NEXUS_NVMF_ANA_ENABLE", "1")
            .with_env("NVMF_TGT_CRDT", "0")
            .with_env("ENABLE_SNAPSHOT_REBUILD", "true")
            .with_bind("/tmp", "/host/tmp")
            .with_bind("/var/run/dpdk", "/var/run/dpdk");

            let core_list = match options.io_engine_isolate {
                true => {
                    let cores = 1.max(options.io_engine_cores);
                    let initial = i * cores;
                    initial .. initial + cores
                }
                false => 0 .. options.io_engine_cores,
            }
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(",");
            if !core_list.is_empty() {
                spec = spec.with_args(vec!["-l", core_list.as_str()]);
            }

            if let Some(env) = &options.io_engine_env {
                for kv in env {
                    spec = spec.with_env(kv.key.as_str(), kv.value.as_str().as_ref());
                }
            }

            if !options.io_engine_devices.is_empty() {
                spec = spec.with_privileged(Some(true));
                for device in options.io_engine_devices.iter() {
                    spec = spec.with_bind(device, device);
                }
            }

            if options.developer_delayed {
                spec = spec.with_env("DEVELOPER_DELAYED", "1");
            }

            if !options.no_etcd && !options.io_engine_no_pstor {
                let etcd = format!("etcd.{}:2379", options.cluster_label.name());
                spec = spec.with_args(vec!["-p", &etcd]);
            }

            if options.eventing {
                let nats_server_url = "nats://nats:4222";
                spec = spec.with_args(vec!["--events-url", nats_server_url]);
            }

            cfg = cfg.add_container_spec(spec)
        }
        Ok(cfg)
    }
    async fn start(&self, options: &StartOptions, cfg: &ComposeTest) -> Result<(), Error> {
        let io_engines = (0 .. options.io_engines)
            .map(|i| async move { cfg.start(&Self::name(i, options)).await });
        futures::future::try_join_all(io_engines).await?;
        Ok(())
    }
    async fn wait_on(&self, options: &StartOptions, cfg: &ComposeTest) -> Result<(), Error> {
        for i in 0 .. options.io_engines {
            let name = Self::name(i, options);
            let container_ip = cfg.container_ip_as_ref(&name);
            let socket = SocketAddr::new(IpAddr::from(*container_ip), 10124);
            let mut hdl = RpcHandle::connect(
                options.latest_io_api_version(),
                &name,
                socket,
                100,
                tokio::time::sleep,
            )
            .await?;
            hdl.ping().await.unwrap();
        }
        for i in 0 .. options.io_engines {
            let name = Self::name(i, options);
            if i == 0 {
                let rm = match options.io_engine_bin.is_some() {
                    true => Binary::which("rm").unwrap(),
                    false => "rm".to_string(),
                };
                cfg.exec(&name, vec![rm.as_str(), "-rf", Self::ptpl().1])
                    .await?;
            }
            super::CoreAgent::wait_node_online(cfg, &name).await;
        }
        Ok(())
    }
}

impl IoEngine {
    /// Get the `IoEngine` container and node name.
    pub fn name(i: u32, _options: &StartOptions) -> String {
        format!("io-engine-{}", i + 1)
    }
    fn idle_name(i: u32, options: &StartOptions) -> String {
        let index = if options.idle_io_engines == 0 {
            i + 1
        } else {
            (i % options.idle_io_engines) + 1
        };
        format!("io-engine-{}", index)
    }
    pub fn nqn(i: u32, options: &StartOptions) -> String {
        format!(
            "{}:{}",
            utils::constants::nvme_initiator_nqn_prefix(),
            Self::name(i, options)
        )
    }
    /// Get the persistent reservation base path for host and container.
    pub fn ptpl() -> (&'static str, &'static str) {
        ("/tmp/ptpl", "/host/tmp/ptpl")
    }
    fn binary(path: &str) -> Option<String> {
        match std::env::var_os(path) {
            None => None,
            Some(val) if val.is_empty() => None,
            Some(val) => Some(val.to_string_lossy().to_string()),
        }
    }
}
