pub mod infra;

use infra::*;

use clap::{builder::TypedValueParser, Parser};
use composer::Builder;
use rpc::io_engine::IoEngineApiVersion;
use std::{collections::HashMap, convert::TryInto, fmt::Write, str::FromStr, time::Duration};
pub(crate) use utils::tracing_telemetry::KeyValue;

const TEST_LABEL_PREFIX: &str = "io.composer.test";

#[derive(Debug, Parser)]
#[clap(name = utils::package_description!(), version = utils::version_info_str!())]
pub struct CliArgs {
    #[clap(subcommand)]
    action: Action,
}

#[derive(Debug, Parser)]
#[clap(about = "Deployment actions")]
pub(crate) enum Action {
    Start(Box<StartOptions>),
    Stop(StopOptions),
    List(ListOptions),
}

/// docker network label:
/// $prefix.name = $name
const DEFAULT_CLUSTER_LABEL: &str = ".cluster";

#[derive(Debug, Parser)]
#[clap(about = "Stop and delete all components")]
pub struct StopOptions {
    /// Label for the cluster
    /// In the form of "prefix.name"
    #[clap(short, long, default_value = DEFAULT_CLUSTER_LABEL)]
    pub cluster_label: ClusterLabel,
}

#[derive(Debug, Default, Parser)]
#[clap(about = "List all running components")]
pub struct ListOptions {
    /// Simple list without using the docker executable
    #[clap(short, long)]
    pub no_docker: bool,

    /// Format the docker output
    #[clap(short, long, conflicts_with = "no_docker")]
    pub format: Option<String>,

    /// Label for the cluster
    #[clap(short, long, default_value = DEFAULT_CLUSTER_LABEL)]
    pub cluster_label: ClusterLabel,
}

/// Label for a cluster: $filter.name = $name
#[derive(Default, Clone)]
pub struct ClusterLabel {
    prefix: String,
    name: String,
}

/// Cluster Uid
pub type ClusterUid = String;

/// default enabled agents
pub fn default_agents() -> &'static str {
    "Core"
}

#[derive(Debug, Default, Clone, Parser)]
#[clap(about = "Create and start all components")]
pub struct StartOptions {
    /// Use the following Control Plane Agents
    /// Specify one agent at a time or as a list.
    /// ( "" for no agents ).
    /// todo: specify start arguments, eg: Node="-v".
    #[clap(
        short,
        long,
        default_value = default_agents(),
        value_parser = clap::builder::PossibleValuesParser::new(ControlPlaneAgent::variants()).map(|s| s.parse::<ControlPlaneAgent>().unwrap()),
        value_delimiter = ','
    )]
    pub agents: Vec<ControlPlaneAgent>,

    /// Kubernetes Config file if using operators.
    /// [default: "~/.kube/config"]
    #[clap(long)]
    pub kube_config: Option<String>,

    /// Use a base image for the binary components (eg: alpine:latest).
    #[clap(long, env = "BASE_IMAGE")]
    pub base_image: Option<String>,

    /// Use the jaegertracing service.
    #[clap(short, long)]
    pub jaeger: bool,

    /// Use an external jaegertracing collector.
    #[clap(long, env = "EXTERNAL_JAEGER")]
    pub external_jaeger: Option<String>,

    /// Use the elasticsearch service.
    #[clap(short, long)]
    pub elastic: bool,

    /// Use the kibana service.
    /// Note: the index pattern is only created when used in conjunction with `wait_timeout`.
    #[clap(short, long)]
    pub kibana: bool,

    /// Disable the REST Server.
    #[clap(long)]
    pub no_rest: bool,

    /// Enable the CSI Controller plugin.
    #[clap(long)]
    pub csi_controller: bool,

    /// Enable the CSI Node plugin.
    #[clap(long)]
    pub csi_node: bool,

    /// Enable the rest client in the csi-node plugin.
    #[clap(long, requires = "csi_node")]
    pub csi_node_rest: bool,

    /// Use `N` csi-node instances
    #[clap(long, requires = "csi_node")]
    pub app_nodes: Option<u32>,

    /// Run csi-node instances on io-engine nodes.
    #[clap(short, long, requires = "csi_node")]
    pub local_nodes: bool,

    /// Rest Path to JSON Web KEY file used for authenticating REST requests.
    /// Otherwise, no authentication is used
    #[clap(long, conflicts_with = "no_rest")]
    pub rest_jwk: Option<String>,

    /// Set the rest-to-core health probe frequency on the rest.
    #[arg(long)]
    pub rest_core_health_freq: Option<String>,

    /// Use the following image pull policy when creating containers from images.
    #[clap(long, default_value = "ifnotpresent")]
    pub image_pull_policy: composer::ImagePullPolicy,

    /// Use `N` io_engine instances.
    /// Note: the io_engine containers have the host's /tmp directory mapped into the container
    /// as /host/tmp. This is useful to create pool's from file images.
    #[clap(short, long, default_value = "1")]
    pub io_engines: u32,

    /// Use `N` idle io_engine instances, to replace `N` `io_engines`, in order!
    /// These can be used to replace the `io_engines` which can be useful to simulate upgrades
    /// for example.
    /// # Warnings:
    /// These engines share a lot of configuration with the regular `io_engines` (as intended),
    /// and as such please ensure these are not active at the same time as its corresponding
    /// `io_engine` container.
    #[clap(long, default_value = "0")]
    pub idle_io_engines: u32,

    /// Use the following docker image for the io_engine instances.
    #[clap(long, env = "IO_ENGINE_IMAGE", default_value = utils::io_engine_image())]
    pub io_engine_image: String,

    /// Use the following docker image for the io_engine instances.
    #[clap(long, env = "IDLE_IO_ENGINE_IMAGE", default_value = utils::io_engine_image())]
    pub idle_io_engine_image: String,

    /// Use the following runnable binary for the io_engine instances.
    #[clap(long, env = "IO_ENGINE_BIN", conflicts_with = "io_engine_image")]
    pub io_engine_bin: Option<String>,

    /// Use the following runnable binary for the io_engine instances.
    #[clap(
        long,
        env = "IDLE_IO_ENGINE_BIN",
        conflicts_with = "idle_io_engine_image"
    )]
    pub idle_io_engine_bin: Option<String>,

    /// Add host block devices to the io_engine containers as a docker bind mount
    /// A raw block device: --io_engine-devices /dev/sda /dev/sdb
    /// An lvm volume group: --io_engine-devices /dev/sdavg
    /// Note: the io_engine containers will run as `privileged`!
    #[clap(long)]
    pub io_engine_devices: Vec<String>,

    /// Run each io_engine on separate cores.
    #[clap(long)]
    pub io_engine_isolate: bool,

    /// Run each io_engine with these many cores.
    /// A value of 0 means we'll rely on the default value set by the io-engine.
    #[clap(long, default_value = "0")]
    pub io_engine_cores: u32,

    /// Add the following environment variables to the io_engine containers.
    #[clap(long, env = "IO_ENGINE_ENV", value_delimiter=',', value_parser = utils::tracing_telemetry::parse_key_value)]
    pub io_engine_env: Option<Vec<KeyValue>>,

    /// The gRPC api versions to be passed to the io-engine.
    #[clap(
        long,
        env = "IO_ENGINE_API_VERSIONS",
        value_delimiter = ',',
        default_value = "v1"
    )]
    io_engine_api_versions: Vec<IoEngineApiVersion>,

    /// Don't configure the persistent store with the io-engine.
    #[clap(long)]
    io_engine_no_pstor: bool,

    /// Set the developer delayed env flag of the io_engine reactor.
    #[clap(short, long)]
    pub developer_delayed: bool,

    /// Add the following environment variables to the agent containers.
    #[clap(long, env = "AGENTS_ENV", value_delimiter=',', value_parser = utils::tracing_telemetry::parse_key_value)]
    pub agents_env: Option<Vec<KeyValue>>,

    /// Add the following environment variables to the rest container.
    #[clap(long, env = "REST_ENV", value_delimiter=',', value_parser = utils::tracing_telemetry::parse_key_value)]
    pub rest_env: Option<Vec<KeyValue>>,

    /// Cargo Build each component before deploying.
    #[clap(short, long)]
    pub build: bool,

    /// Cargo Build the workspace before deploying.
    #[clap(long)]
    pub build_all: bool,

    /// Use a dns resolver for the cluster: defreitas/dns-proxy-server.
    /// Note this messes with your /etc/resolv.conf so use at your own risk.
    #[clap(long)]
    pub dns: bool,

    /// Show information from the cluster after creation.
    #[clap(short, long)]
    pub show_info: bool,

    /// Name of the cluster - currently only one allowed at a time.
    /// Note: Does not quite work as intended, as we haven't figured out how to bridge between
    /// different networks.
    #[clap(short, long, default_value = DEFAULT_CLUSTER_LABEL)]
    pub cluster_label: ClusterLabel,

    /// Uid of the cluster.
    /// By default the agents will determine this value themselves.
    #[clap(long)]
    pub cluster_uid: Option<ClusterUid>,

    /// Disable the etcd service.
    #[clap(long)]
    pub no_etcd: bool,

    /// The period at which the registry updates its cache of all
    /// resources from all nodes.
    #[clap(long)]
    pub cache_period: Option<humantime::Duration>,

    /// Override the node's deadline for the Core Agent.
    #[clap(long)]
    pub node_deadline: Option<humantime::Duration>,

    /// Override the base request timeout for GRPC requests.
    #[clap(long)]
    pub request_timeout: Option<humantime::Duration>,

    /// Override the node's connection timeout.
    #[clap(long)]
    pub node_conn_timeout: Option<humantime::Duration>,

    /// If set, configures the agent-ha-cluster fast requeue period to this duration.
    #[clap(long)]
    cluster_fast_requeue: Option<humantime::Duration>,

    /// Don't use minimum timeouts for specific requests.
    #[clap(long)]
    no_min_timeouts: bool,

    /// Override the core agent's store operation timeout.
    #[clap(long)]
    pub store_timeout: Option<humantime::Duration>,

    /// Override the core agent's lease lock ttl for the persistent store after which it'll loose
    /// the exclusive access to the store.
    #[clap(long)]
    pub store_lease_ttl: Option<humantime::Duration>,

    /// Override the core agent's reconcile period.
    #[clap(long)]
    pub reconcile_period: Option<humantime::Duration>,

    /// Override the core agent's reconcile idle period.
    #[clap(long)]
    pub reconcile_idle_period: Option<humantime::Duration>,

    /// Override the core agent's rebuild policy timed-wait setting.
    #[clap(long)]
    pub faulted_child_wait_period: Option<humantime::Duration>,

    /// Override the opentel max exporter batch size.
    #[clap(long, env = "OTEL_BSP_MAX_EXPORT_BATCH_SIZE")]
    pub otel_max_batch_size: Option<String>,

    /// Amount of time to wait for all containers to start.
    #[clap(short, long)]
    pub wait_timeout: Option<humantime::Duration>,

    /// Don't stop/remove existing containers on the same cluster
    /// Allows us to start "different" stacks independently, eg:
    /// > deployer start -ejk -s -a "" --no-rest --no-etcd -m 0
    /// > deployer start -s -m 2
    #[clap(short, long)]
    pub reuse_cluster: bool,

    /// Add process service tags to the traces.
    #[clap(short, long, env = "TRACING_TAGS", value_delimiter=',', value_parser = utils::tracing_telemetry::parse_key_value)]
    tracing_tags: Vec<KeyValue>,

    /// Maximum number of concurrent rebuilds across the cluster.
    #[clap(long)]
    max_rebuilds: Option<u32>,

    /// Deploy a fio-spdk container.
    /// This can be used for userspace io against a volume's nvmf target using the spdk ioengine.
    #[clap(long)]
    pub(crate) fio_spdk: bool,

    #[clap(long, default_value = "false")]
    pub eventing: bool,

    #[clap(long, default_value = "false")]
    pub enable_app_node_registration: bool,
}

/// List of KeyValues
#[derive(Default, Debug)]
pub(crate) struct KeyValues {
    inner: HashMap<String, String>,
}
impl KeyValues {
    /// return new `Self` from `Vec<KeyValue>`
    pub(crate) fn new(src: Vec<KeyValue>) -> Self {
        src.into_iter().fold(Self::default(), |mut acc, key_val| {
            acc.add(key_val);
            acc
        })
    }
    /// add if not already there
    pub(crate) fn add(&mut self, key_val: KeyValue) {
        if let std::collections::hash_map::Entry::Vacant(e) =
            self.inner.entry(key_val.key.to_string())
        {
            e.insert(key_val.value.to_string());
        }
    }
    /// Convert to args
    pub(crate) fn into_args(self) -> Option<String> {
        if !self.inner.is_empty() {
            let mut arg_start = "".to_string();
            self.inner.into_iter().for_each(|(k, v)| {
                if !arg_start.is_empty() {
                    arg_start.push(',');
                }
                let _ = write!(arg_start, "{k}={v}");
            });
            Some(arg_start)
        } else {
            None
        }
    }
    /// Convert to otel resource attributes.
    pub(crate) fn into_otel_attrs(self) -> Option<String> {
        if !self.inner.is_empty() {
            let mut arg_start = "processors:\n  resource:\n    attributes:\n".to_string();
            self.inner.into_iter().for_each(|(k, v)| {
                let _ = write!(
                    arg_start,
                    "      - key: {k}\n        value: {v}\n        action: insert\n"
                );
            });
            Some(arg_start)
        } else {
            None
        }
    }
}

impl StartOptions {
    #[must_use]
    pub fn with_agents(mut self, agents: Vec<&str>) -> Self {
        let agents: ControlPlaneAgents = agents.try_into().unwrap();
        self.agents = agents.into_inner();
        self
    }
    #[must_use]
    pub fn with_cache_period(mut self, period: &str) -> Self {
        self.cache_period = Some(humantime::Duration::from_str(period).unwrap());
        self
    }
    #[must_use]
    pub fn with_node_deadline(mut self, deadline: &str) -> Self {
        self.node_deadline = Some(humantime::Duration::from_str(deadline).unwrap());
        self
    }
    #[must_use]
    pub fn with_store_timeout(mut self, timeout: Duration) -> Self {
        self.store_timeout = Some(timeout.into());
        self
    }
    #[must_use]
    pub fn with_store_lease_ttl(mut self, ttl: Duration) -> Self {
        self.store_lease_ttl = Some(ttl.into());
        self
    }
    #[must_use]
    pub fn with_reconcile_period(mut self, busy: Duration, idle: Duration) -> Self {
        self.reconcile_period = Some(busy.into());
        self.reconcile_idle_period = Some(idle.into());
        self
    }
    #[must_use]
    pub fn with_faulted_child_wait_period(mut self, twait: Duration) -> Self {
        self.faulted_child_wait_period = Some(twait.into());
        self
    }
    #[must_use]
    pub fn with_req_timeouts(mut self, no_min: bool, connect: Duration, request: Duration) -> Self {
        self.no_min_timeouts = no_min;
        self.node_conn_timeout = Some(connect.into());
        self.request_timeout = Some(request.into());
        self
    }
    #[must_use]
    pub fn with_rest(mut self, enabled: bool, jwk: Option<String>) -> Self {
        self.no_rest = !enabled;
        self.rest_jwk = jwk;
        self
    }
    #[must_use]
    pub fn with_csi(mut self, controller: bool, node: bool) -> Self {
        self.csi_controller = controller;
        self.csi_node = node;
        self
    }
    #[must_use]
    pub fn with_csi_registration(mut self, opt: bool) -> Self {
        self.enable_app_node_registration = opt;
        self
    }
    #[must_use]
    pub fn with_jaeger(mut self, jaeger: bool) -> Self {
        self.jaeger = jaeger;
        self
    }
    #[must_use]
    pub fn with_build(mut self, build: bool) -> Self {
        self.build = build;
        self
    }
    #[must_use]
    pub fn with_build_all(mut self, build: bool) -> Self {
        self.build_all = build;
        self
    }
    #[must_use]
    pub fn with_io_engines(mut self, io_engines: u32) -> Self {
        self.io_engines = io_engines;
        self
    }
    #[must_use]
    pub fn with_idle_io_engines(mut self, idle_io_engines: u32) -> Self {
        self.idle_io_engines = idle_io_engines;
        self
    }
    #[must_use]
    pub fn with_pull_policy(mut self, policy: composer::ImagePullPolicy) -> Self {
        self.image_pull_policy = policy;
        self
    }
    #[must_use]
    pub fn with_io_engine_env(mut self, key: &str, val: &str) -> Self {
        let mut env = self.io_engine_env.unwrap_or_default();
        env.push(KeyValue::new(key.to_string(), val.to_string()));
        self.io_engine_env = Some(env);
        self
    }
    #[must_use]
    pub fn with_agents_env(mut self, key: &str, val: &str) -> Self {
        let mut env = self.agents_env.unwrap_or_default();
        env.push(KeyValue::new(key.to_string(), val.to_string()));
        self.agents_env = Some(env);
        self
    }
    #[must_use]
    pub fn with_isolated_io_engine(mut self, isolate: bool) -> Self {
        self.io_engine_isolate = isolate;
        self
    }
    #[must_use]
    pub fn with_io_engine_no_pstor(mut self, no_pstor: bool) -> Self {
        self.io_engine_no_pstor = no_pstor;
        self
    }
    #[must_use]
    pub fn with_io_engine_cores(mut self, cores: u32) -> Self {
        self.io_engine_cores = cores;
        self
    }
    #[must_use]
    pub fn with_io_engine_bin(mut self, binary: &str) -> Self {
        self.io_engine_bin = Some(binary.to_string());
        self
    }
    #[must_use]
    pub fn with_io_engine_tag(mut self, tag: &str) -> Self {
        self.io_engine_image = utils::io_engine_image_tagged(tag.into());
        self
    }
    #[must_use]
    pub fn with_io_engine_img(mut self, image: &str) -> Self {
        self.io_engine_image = image.to_string();
        self
    }
    #[must_use]
    pub fn with_idle_io_engine_bin(mut self, bin: &str) -> Self {
        self.idle_io_engine_bin = Some(bin.to_string());
        self
    }
    #[must_use]
    pub fn with_io_engine_devices(mut self, devices: Vec<&str>) -> Self {
        self.io_engine_devices = devices.into_iter().map(Into::into).collect();
        self
    }
    #[must_use]
    pub fn with_show_info(mut self, show_info: bool) -> Self {
        self.show_info = show_info;
        self
    }
    #[must_use]
    pub fn with_cluster_name(mut self, cluster_name: &str) -> Self {
        self.cluster_label = format!(".{cluster_name}").parse().unwrap();
        self
    }
    #[must_use]
    pub fn with_base_image(mut self, base_image: impl Into<Option<String>>) -> Self {
        self.base_image = base_image.into();
        self
    }
    #[must_use]
    pub fn with_tags(mut self, tags: Vec<KeyValue>) -> Self {
        self.tracing_tags.extend(tags);
        self
    }
    #[must_use]
    pub fn with_env_tags(mut self, env: Vec<&str>) -> Self {
        env.iter().for_each(|env| {
            if let Ok(val) = std::env::var(env) {
                self.tracing_tags.push(KeyValue::new(env.to_string(), val));
            }
        });
        self
    }
    #[must_use]
    pub fn with_max_rebuilds(mut self, max: Option<u32>) -> Self {
        self.max_rebuilds = max;
        self
    }
    /// Enable/Disable the fio-spdk container.
    #[must_use]
    pub fn with_fio_spdk(mut self, fio_spdk: bool) -> Self {
        self.fio_spdk = fio_spdk;
        self
    }
    /// Specify whether eventing is enabled or not.
    pub fn with_eventing(mut self, enabled: bool) -> Self {
        self.eventing = enabled;
        self
    }

    pub(crate) fn app_nodes(&self) -> u32 {
        if self.csi_node {
            self.app_nodes.unwrap_or(1)
        } else {
            0
        }
    }

    /// Get the io-engine api versions.
    pub fn io_api_versions(&self) -> &Vec<IoEngineApiVersion> {
        &self.io_engine_api_versions
    }
    /// Get the latest io-engine api version.
    pub fn latest_io_api_version(&self) -> IoEngineApiVersion {
        let mut sorted = self.io_engine_api_versions.clone();
        sorted.sort();
        *sorted.last().unwrap_or(&IoEngineApiVersion::V1)
    }
}

impl CliArgs {
    /// Act upon the requested action
    pub async fn execute(&self) -> Result<(), Error> {
        self.action.execute().await
    }
}

impl Action {
    async fn execute(&self) -> Result<(), Error> {
        match self {
            Action::Start(options) => options.start(self).await,
            Action::Stop(options) => options.stop(self).await,
            Action::List(options) => options.list(self).await,
        }
    }
}

impl StartOptions {
    async fn start(&self, _action: &Action) -> Result<(), Error> {
        let components = Components::new(self.clone());
        let composer = Builder::new()
            .name(&self.cluster_label.name())
            .label_prefix(&self.cluster_label.prefix())
            .with_clean(false)
            .with_base_image(self.base_image.clone())
            .with_prune_reuse(!self.reuse_cluster, self.reuse_cluster, self.reuse_cluster)
            .autorun(false)
            .load_existing_containers()
            .await
            .configure(components.clone())?
            .build()
            .await?;

        match self.wait_timeout {
            Some(timeout) => {
                components.start_wait(&composer, timeout.into()).await?;
            }
            None => {
                components.start(&composer).await?;
            }
        }

        if self.show_info {
            let lister = ListOptions {
                cluster_label: self.cluster_label.clone(),
                ..Default::default()
            };
            lister.list_simple().await?;
        }
        Ok(())
    }
}
impl StopOptions {
    async fn stop(&self, _action: &Action) -> Result<(), Error> {
        let composer = Builder::new()
            .name(&self.cluster_label.name())
            .label_prefix(&self.cluster_label.prefix())
            .with_prune(false)
            .with_clean(true)
            .build()
            .await?;
        tracing::trace!("Stopping network containers");
        let _ = composer.stop_network_containers().await;
        tracing::trace!("Removing network containers");
        composer
            .remove_network_containers(&self.cluster_label.name())
            .await?;
        Ok(())
    }
}
impl ListOptions {
    fn list_docker(&self) -> Result<(), Error> {
        let label_filter = format!("label={}", self.cluster_label.filter());
        let mut args = vec!["ps", "-a", "--filter", &label_filter];
        if let Some(format) = &self.format {
            args.push("--format");
            args.push(format)
        }
        let status = std::process::Command::new("docker").args(args).status()?;
        build_error("docker", status.code())
    }
    /// Simple listing of all started components
    pub async fn list_simple(&self) -> Result<(), Error> {
        let cfg = Builder::new()
            .name(&self.cluster_label.name())
            .label_prefix(&self.cluster_label.prefix())
            .with_prune_reuse(false, false, false)
            .with_clean(false)
            .build()
            .await?;

        let containers = cfg.list_cluster_containers().await?;
        let mut components = Vec::with_capacity(containers.len());
        for component in containers {
            let ip = match component.network_settings.clone() {
                None => None,
                Some(networks) => match networks.networks {
                    None => None,
                    Some(network) => match network.get(&self.cluster_label.name()) {
                        None => None,
                        Some(endpoint) => endpoint.ip_address.clone(),
                    },
                },
            };
            let name = component
                .names
                .unwrap_or_default()
                .first()
                .cloned()
                .unwrap_or_else(|| "?".to_string());
            let ip = ip.unwrap_or_default();
            let command = option_str(component.command);
            components.push((name, ip, command));
        }
        components.sort_by_key(|a| a.0.clone());
        for (name, ip, command) in components {
            println!("[{name}] [{ip}] {command}",);
        }
        Ok(())
    }
    async fn list(&self, _action: &Action) -> Result<(), Error> {
        match self.no_docker {
            true => self.list_simple().await,
            false => self.list_docker(),
        }
    }
}

fn option_str<F: ToString>(input: Option<F>) -> String {
    match input {
        Some(input) => input.to_string(),
        None => "?".into(),
    }
}

impl ClusterLabel {
    /// Get the network prefix
    pub fn prefix(&self) -> String {
        if self.prefix.is_empty() {
            TEST_LABEL_PREFIX.to_string()
        } else {
            self.prefix.clone()
        }
    }
    /// Get the network name
    pub fn name(&self) -> String {
        self.name.clone()
    }
    /// Get the network as the filter
    pub fn filter(&self) -> String {
        format!("{}.name={}", self.prefix(), self.name())
    }
}

impl FromStr for ClusterLabel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut split = s.split('.');
        if split.clone().count() == 2 {
            return Ok(ClusterLabel {
                prefix: split.next().unwrap().to_string(),
                name: split.next().unwrap().to_string(),
            });
        }
        Err("Should be in the format 'prefix.name'".to_string())
    }
}
impl std::fmt::Display for ClusterLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "'{}'.'{}'", self.prefix(), self.name())
    }
}
impl std::fmt::Debug for ClusterLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

/// Get the host tmp folder for this workspace.
pub fn host_tmp() -> Result<String, std::io::Error> {
    let root_tmp = format!("{root}/.tmp", root = env!("WORKSPACE_ROOT"));
    if !std::path::Path::new(&root_tmp).exists() {
        std::fs::create_dir(&root_tmp)?;
    }
    Ok(root_tmp)
}
