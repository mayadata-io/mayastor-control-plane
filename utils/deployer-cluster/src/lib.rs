pub mod lvm;
pub mod rest_client;

use composer::{Builder, ComposeTest};
use deployer_lib::{
    default_agents,
    infra::{Components, Error, IoEngine},
    StartOptions,
};
use opentelemetry::{global, KeyValue};
use opentelemetry_sdk::{propagation::TraceContextPropagator, trace as sdktrace};

use stor_port::{transport_api::TimeoutOptions, types::v0::transport};

use clap::Parser;
pub use composer::ImagePullPolicy;
pub use csi_driver::node::internal::*;
use deployer_lib::infra::CsiNode;
pub use etcd_client;
use etcd_client::DeleteOptions;
use grpc::{
    client::CoreClient,
    context::Context,
    operations::{
        node::traits::NodeOperations, pool::traits::PoolOperations,
        replica::traits::ReplicaOperations, volume::traits::VolumeOperations,
    },
};
use openapi::{apis::Url, models::Volume};
use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use rpc::{
    csi::{CreateSnapshotResponse, NodeStageVolumeResponse, NodeUnstageVolumeResponse},
    io_engine::RpcHandle,
};
use std::{
    collections::HashMap,
    convert::{TryFrom, TryInto},
    net::SocketAddr,
    ops::Deref,
    str::FromStr,
    sync::Arc,
    time::Duration,
};
use stor_port::{
    transport_api::ReplyError,
    types::v0::{
        store::{
            definitions::ObjectKey,
            registry::{ControlPlaneService, StoreLeaseLockKey},
        },
        transport::{CreatePool, Filter, NodeId, NodeStatus, PoolId, PoolStatus},
    },
};
use tokio::{net::UnixStream, time::sleep};
use tonic::transport::Uri;
use tower::service_fn;
use tracing::dispatcher::DefaultGuard;
use tracing_subscriber::{filter::Directive, layer::SubscriberExt, EnvFilter, Registry};
use utils::tracing_telemetry::default_tracing_tags;

const RUST_LOG_SILENCE_DEFAULTS: &str =
    "h2=info,hyper=info,tower_buffer=info,tower=info,rustls=info,reqwest=info,tokio_util=info,async_io=info,polling=info,tonic=info,want=info,mio=info,bollard=info,composer=info";

#[tokio::test]
async fn smoke_test() {
    // make sure the cluster can bootstrap properly
    let _cluster = ClusterBuilder::builder()
        .with_pull_policy(composer::ImagePullPolicy::Always)
        .build()
        .await
        .expect("Should bootstrap the cluster!");
}

/// Default options to create a cluster
pub fn default_options() -> StartOptions {
    // using from_iter as Default::default would not set the default_value from structopt
    let options: StartOptions = StartOptions::parse_from([""]);
    options
        .with_agents(default_agents().split(',').collect())
        .with_jaeger(true)
        .with_io_engines(1)
        .with_show_info(true)
        .with_build_all(!matches!(std::env::var("CI").as_deref(), Ok("1")))
        .with_env_tags(vec!["CARGO_PKG_NAME"])
}

/// A wrapper over the composer utility meant to ensure termination in the
/// correct order.
/// todo: I suspect this is not working because composer itself is being created
///  with cleaning enabled, so this won't actually work as expected!
pub struct ComposeTestNt {
    logs_on_panic: bool,
    clean: bool,
    allow_clean_on_panic: bool,
    composer: ComposeTest,
    shutdown_order: Vec<Vec<String>>,
}
impl ComposeTestNt {
    async fn new(composer: Builder) -> Result<Self, Error> {
        let composer = composer.build().await?;
        Ok(Self {
            logs_on_panic: composer.logs_on_panic(),
            clean: composer.clean(),
            allow_clean_on_panic: composer.clean_on_panic(),
            composer,
            shutdown_order: vec![],
        })
    }
}
impl Deref for ComposeTestNt {
    type Target = ComposeTest;
    fn deref(&self) -> &Self::Target {
        &self.composer
    }
}
impl Drop for ComposeTestNt {
    fn drop(&mut self) {
        if std::thread::panicking() && self.logs_on_panic {
            self.print_all_logs();
        }

        if self.clean && (!std::thread::panicking() || self.allow_clean_on_panic) {
            let sh = self.shutdown_order.drain(..);
            sh.into_iter().for_each(|c| {
                c.into_iter()
                    .map(|c| {
                        std::thread::spawn(move || {
                            std::process::Command::new("docker")
                                .args(["kill", "-s", "term", c.as_str()])
                                .output()
                                .unwrap();
                        })
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .for_each(|h| {
                        h.join().ok();
                    });
            });
            self.composer
                .containers()
                .keys()
                .map(|k| {
                    let name = k.clone();
                    std::thread::spawn(move || {
                        std::process::Command::new("docker")
                            .args(["kill", "-s", "term", name.as_str()])
                            .output()
                            .unwrap();
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .for_each(|h| {
                    h.join().ok();
                });
        }
        self.composer.clear_logs_on_panic();
    }
}

/// Cluster with the composer, the rest client and the jaeger pipeline
#[allow(unused)]
pub struct Cluster {
    composer: Arc<ComposeTestNt>,
    rest_client: rest_client::RestClient,
    grpc_client: Option<CoreClient>,
    trace_guard: Arc<DefaultGuard>,
    builder: ClusterBuilder,
}

impl Cluster {
    /// A reference to the compose test utility.
    pub fn composer(&self) -> &ComposeTest {
        &self.composer
    }
    /// A reference to our wrapper over the compose utility.
    /// This can be safely sent across threads.
    pub fn composer_nt(&self) -> &Arc<ComposeTestNt> {
        &self.composer
    }

    /// grpc client for connection
    pub fn grpc_client(&self) -> &CoreClient {
        self.grpc_client.as_ref().unwrap()
    }

    pub async fn new_grpc_client(&self, grpc_timeout: TimeoutOptions) -> CoreClient {
        let core_ip = self.composer.container_ip("core");
        CoreClient::new(Uri::try_from(grpc_addr(core_ip)).unwrap(), grpc_timeout).await
    }

    /// volume service liveness checks whether the volume service responds to the
    /// liveliness probe(generally after restart of core agent), with the timeout and
    /// retry options specified
    pub async fn volume_service_liveness(
        &self,
        timeout_opts: Option<TimeoutOptions>,
    ) -> Result<bool, ReplyError> {
        let client = self.grpc_client().volume();
        let timeout_opts = match timeout_opts {
            Some(opts) => opts,
            None => TimeoutOptions::new()
                .with_req_timeout(Duration::from_millis(500))
                .with_max_retries(20),
        };
        for x in 1..timeout_opts.max_retries().unwrap_or_default() {
            match client
                .probe(Some(Context::new(Some(timeout_opts.clone()))))
                .await
            {
                Ok(resp) => return Ok(resp),
                Err(_) => {
                    tracing::debug!("Volume Service not available, Retrying ....{}", x);
                    tokio::time::sleep(timeout_opts.base_timeout()).await;
                }
            }
        }
        Err(ReplyError::invalid_reply_error(
            "Max tries exceeded, volume service not up".to_string(),
        ))
    }

    /// node service liveness checks whether the node service responds to the
    /// liveliness probe(generally after restart of core agent), with the timeout and
    /// retry options specified
    pub async fn node_service_liveness(
        &self,
        timeout_opts: Option<TimeoutOptions>,
    ) -> Result<bool, ReplyError> {
        let client = self.grpc_client().node();
        let timeout_opts = match timeout_opts {
            Some(opts) => opts,
            None => TimeoutOptions::new()
                .with_req_timeout(Duration::from_millis(100))
                .with_timeout_backoff(Duration::from_millis(25))
                .with_max_retries(100),
        };
        for x in 1..timeout_opts.max_retries().unwrap_or_default() {
            match client
                .probe(Some(Context::new(Some(timeout_opts.clone()))))
                .await
            {
                Ok(resp) => return Ok(resp),
                Err(_) => {
                    tracing::debug!("Node Service not available, Retrying ....{}", x);
                    tokio::time::sleep(timeout_opts.backoff_timeout()).await;
                }
            }
        }
        Err(ReplyError::invalid_reply_error(
            "Max tries exceeded, node service not up".to_string(),
        ))
    }

    /// Wait till the node is in the given status.
    pub async fn wait_node_status(&self, node_id: NodeId, status: NodeStatus) -> Result<(), ()> {
        let timeout = Duration::from_secs(2);
        let node_cli = self.grpc_client().node();
        let start = std::time::Instant::now();
        loop {
            let node = node_cli
                .get(Filter::Node(node_id.clone()), true, None)
                .await
                .expect("Cant get node object");
            if let Some(node) = node.0.first() {
                if node.state().map(|n| &n.status) == Some(&status) {
                    return Ok(());
                }
            }
            if std::time::Instant::now() > (start + timeout) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err(())
    }
    /// Wait till the pool is online.
    pub async fn wait_pool_online(&self, pool_id: PoolId) -> Result<(), ()> {
        let timeout = Duration::from_secs(2);
        let start = std::time::Instant::now();
        loop {
            let filter = Filter::Pool(pool_id.clone());
            if let Ok(pools) = self.grpc_client().pool().get(filter, None).await {
                if pools
                    .into_inner()
                    .first()
                    .and_then(|p| p.state().map(|s| s.status == PoolStatus::Online))
                    == Some(true)
                {
                    return Ok(());
                }
            }
            if std::time::Instant::now() > (start + timeout) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err(())
    }

    /// return grpc handle to the container
    pub async fn grpc_handle(&self, name: &str) -> Result<RpcHandle, String> {
        match self.composer.containers().iter().find(|&c| c.0 == name) {
            Some(container) => Ok(RpcHandle::connect(
                self.builder.opts.latest_io_api_version(),
                container.0,
                format!("{}:10124", container.1 .1)
                    .parse::<SocketAddr>()
                    .unwrap(),
                20,
                tokio::time::sleep,
            )
            .await?),
            None => Err(format!("Container {name} not found!")),
        }
    }

    /// Return a grpc handle to the csi-node plugin.
    pub async fn csi_node_client(&self, index: u32) -> Result<CsiNodeClient, Error> {
        let csi_socket = Arc::new(self.csi_socket(index));

        let endpoint = tonic::transport::Endpoint::try_from("http://[::]")?
            .connect_timeout(Duration::from_millis(100));
        let channel = loop {
            let csi_socket = csi_socket.clone();
            match endpoint
                .connect_with_connector(tower::service_fn(move |_: Uri| {
                    let socket = csi_socket.clone();
                    async move {
                        Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(
                            UnixStream::connect(socket.deref()).await?,
                        ))
                    }
                }))
                .await
            {
                Ok(channel) => break channel,
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        };

        let csi = rpc::csi::node_client::NodeClient::new(channel);
        let csi_endpoint = self
            .composer()
            .container_ip(&CsiNode::container_name(index));
        let internal = csi_driver::node::internal::node_plugin_client::NodePluginClient::connect(
            format!("http://{csi_endpoint}:50055"),
        )
        .await?;

        Ok(CsiNodeClient { csi, internal })
    }

    /// Return a grpc handle to the csi-controller.
    pub async fn csi_controller_client(&self) -> Result<CsiControllerClient, Error> {
        let endpoint = tonic::transport::Endpoint::try_from("http://[::]")?
            .connect_timeout(Duration::from_millis(100));
        let channel = loop {
            match endpoint
                .connect_with_connector(service_fn(|_: Uri| async {
                    Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(
                        UnixStream::connect(CSI_SOCKET).await?,
                    ))
                }))
                .await
            {
                Ok(channel) => break channel,
                Err(_) => sleep(Duration::from_millis(25)).await,
            }
        };

        let csi = rpc::csi::controller_client::ControllerClient::new(channel);

        Ok(CsiControllerClient { csi })
    }

    /// Restart the core agent.
    pub async fn restart_core(&self) {
        self.remove_store_lock(ControlPlaneService::CoreAgent).await;
        self.composer.restart("core").await.unwrap();
    }

    /// Restart the core agent and wait for services to be live.
    pub async fn restart_core_with_liveness(
        &self,
        timeout_opts: Option<TimeoutOptions>,
    ) -> Result<bool, ReplyError> {
        self.restart_core().await;
        self.volume_service_liveness(timeout_opts).await
    }

    /// Replace the given old node with a new one from the idles.
    pub async fn replace_node(&self, old: NodeId, new: NodeId) -> Result<(), ()> {
        self.composer().stop(&old).await.unwrap();
        self.wait_node_status(old, NodeStatus::Unknown)
            .await
            .unwrap();
        self.composer().start(&new).await.unwrap();
        Ok(())
    }

    /// remove etcd store lock for `name` instance
    pub async fn remove_store_lock(&self, name: ControlPlaneService) {
        let mut store = etcd_client::Client::connect(["[::]:2379"], None)
            .await
            .expect("Failed to connect to etcd.");
        store
            .delete(
                StoreLeaseLockKey::new(&name).key(),
                Some(DeleteOptions::new().with_prefix()),
            )
            .await
            .unwrap();
    }

    /// The node id for `index`.
    pub fn node(&self, index: u32) -> transport::NodeId {
        IoEngine::name(index, &self.builder.opts).into()
    }

    /// The io-engine node nqn for `index`.
    pub fn node_nqn(&self, index: u32) -> transport::HostNqn {
        IoEngine::nqn(index, &self.builder.opts).try_into().unwrap()
    }

    /// The node id for `index`.
    pub fn csi_node(&self, index: u32) -> transport::NodeId {
        CsiNode::name(index).into()
    }

    /// The container name for `index`.
    pub fn csi_container(&self, index: u32) -> String {
        CsiNode::container_name(index)
    }

    /// node id for `index`
    pub fn csi_socket(&self, index: u32) -> String {
        CsiNode::socket(self.csi_node(index).as_str())
    }

    /// node ip for `index`
    pub fn node_ip(&self, index: u32) -> String {
        let name = self.node(index);
        self.composer.container_ip(name.as_str())
    }

    /// Get the node socket.
    pub fn node_socket(&self, index: u32) -> SocketAddr {
        let name = self.node(index);
        let ip = self.composer.container_ip(name.as_str());
        format!("{ip}:10124").parse::<SocketAddr>().unwrap()
    }

    /// pool id for `pool` index on `node` index
    pub fn pool(&self, node: u32, pool: u32) -> transport::PoolId {
        format!("{}-pool-{}", self.node(node), pool + 1).into()
    }

    /// replica id with index for `pool` index and `replica` index
    pub fn replica(node: u32, pool: usize, replica: u32) -> transport::ReplicaId {
        if replica > 254 || pool > 254 || node > 254 {
            panic!("too large");
        }
        let mut uuid = transport::ReplicaId::default().to_string();
        // we can't use a uuid with all zeroes, as spdk seems to ignore it and generate new one
        let replica = replica + 1;
        let _ = uuid.drain(24..uuid.len());
        format!(
            "{}{:02x}{:02x}{:08x}",
            uuid, node as u8, pool as u8, replica
        )
        .try_into()
        .unwrap()
    }

    /// openapi rest client v0
    pub fn rest_v00(&self) -> openapi::tower::client::direct::ApiClient {
        self.rest_client.v0()
    }

    /// Get the openapi rest url.
    pub fn rest_url(&self) -> &Url {
        self.rest_client.url()
    }

    /// New cluster
    async fn new(
        trace: bool,
        trace_guard: Arc<DefaultGuard>,
        timeout_rest: std::time::Duration,
        grpc_timeout: TimeoutOptions,
        bearer_token: Option<String>,
        components: Components,
        composer: ComposeTestNt,
    ) -> Result<Cluster, Error> {
        let rest_client = rest_client::RestClient::new_timeout(
            "http://localhost:8081",
            trace,
            bearer_token,
            timeout_rest,
        )
        .unwrap();

        components
            .start_wait(&composer, std::time::Duration::from_secs(30))
            .await?;

        let unknown_module = "unknown".to_string();
        let mut test_module = None;
        if let Ok(mcp_root) = std::env::var("WORKSPACE_ROOT") {
            backtrace::trace(|frame| {
                backtrace::resolve_frame(frame, |symbol| {
                    if let Some(name) = symbol.name() {
                        if let Some(filename) = symbol.filename() {
                            if filename.starts_with(&mcp_root) && !filename.ends_with(file!()) {
                                let name = name.to_string();
                                let name = match name.split('{').collect::<Vec<_>>().first() {
                                    Some(name) => {
                                        let name = name.to_string();
                                        name.trim_end_matches("::").to_string()
                                    }
                                    None => unknown_module.clone(),
                                };
                                test_module = Some(name);
                            }
                        }
                    }
                });
                test_module.is_none()
            });
        }

        let grpc_client = if components.core_enabled() {
            Some(
                CoreClient::new(
                    Uri::try_from(grpc_addr(composer.container_ip("core"))).unwrap(),
                    grpc_timeout.clone(),
                )
                .await,
            )
        } else {
            None
        };

        let cluster = Cluster {
            composer: Arc::new(composer),
            rest_client,
            grpc_client,
            trace_guard,
            builder: ClusterBuilder::builder(),
        };

        Ok(cluster)
    }
}

fn option_str<F: ToString>(input: Option<F>) -> String {
    match input {
        Some(input) => input.to_string(),
        None => "?".into(),
    }
}

/// Run future and compare result with what's expected.
/// Expected result should be in the form Result<TestValue,TestValue>
/// where TestValue is a useful value which will be added to the returned error
/// string Eg, testing the replica share protocol:
/// test_result(Ok(Nvmf), async move { ... })
/// test_result(Err(NBD), async move { ... })
pub async fn test_result_grpc<F, O, E, T>(
    expected: &Result<O, E>,
    future: F,
) -> Result<(), ReplyError>
where
    F: std::future::Future<Output = Result<T, ReplyError>>,
    E: std::fmt::Debug,
    O: std::fmt::Debug,
    T: std::fmt::Debug,
{
    match future.await {
        Ok(_) if expected.is_ok() => Ok(()),
        Err(_) if expected.is_err() => Ok(()),
        Err(error) => Err(ReplyError::invalid_reply_error(format!(
            "Expected '{expected:#?}' but failed with '{error:?}'!"
        ))),
        Ok(r) => Err(ReplyError::invalid_reply_error(format!(
            "Expected '{expected:#?} {r:#?}' but succeeded!"
        ))),
    }
}

#[macro_export]
macro_rules! result_either {
    ($test:expr) => {
        match $test {
            Ok(v) => v,
            Err(v) => v,
        }
    };
}

#[derive(Clone)]
enum PoolDisk {
    Malloc(u64),
    Uri(String),
    Tmp(TmpDiskFile),
}

/// Wrapper over a temporary "disk" file, which gets deleted on drop.
#[derive(Clone)]
pub struct TmpDiskFile {
    inner: Arc<TmpDiskFileInner>,
}

/// Temporary "disk" file, which gets deleted on drop.
#[derive(Debug)]
pub struct TmpDiskFileInner {
    path: String,
    uri: String,
    cleanup: bool,
}

impl TmpDiskFile {
    /// Creates a new file on `path` with `size`.
    /// The file is deleted on drop.
    pub fn new(name: &str, size: u64) -> Self {
        Self {
            inner: Arc::new(TmpDiskFileInner::new(name, size)),
        }
    }
    /// Disk URI to be used by the dataplane.
    pub fn uri(&self) -> &str {
        self.inner.uri()
    }
    /// Disk path on the host.
    pub fn path(&self) -> &str {
        &self.inner.path
    }

    /// Get the inner disk if there are no other references to it.
    pub fn into_inner(self) -> Result<TmpDiskFileInner, Arc<TmpDiskFileInner>> {
        Arc::try_unwrap(self.inner)
    }
}
impl TmpDiskFileInner {
    fn new(name: &str, size: u64) -> Self {
        let disk = Self::make_new(name);
        let file = std::fs::File::create(&disk.path).expect("to create the tmp file");
        file.set_len(size).expect("to truncate the tmp file");
        disk
    }
    fn make_new(name: &str) -> Self {
        let (path, container) = Self::make_path(name);
        let pool_id = PoolId::new();
        Self {
            // the io-engine is setup with a bind mount from /workspace/tmp to /host/tmp
            uri: format!("aio://{container}?blk_size=512&uuid={pool_id}"),
            path,
            cleanup: true,
        }
    }
    fn make_path(name: &str) -> (String, String) {
        let file = format!("io-engine-disk-{name}");
        let host_tmp = deployer_lib::host_tmp().expect("workspace error");
        (format!("{host_tmp}/{file}"), format!("/host/tmp/{file}"))
    }
    fn uri(&self) -> &str {
        &self.uri
    }
    /// Move the disk to another location.
    pub fn rename(&mut self, new_name: &str) -> std::io::Result<()> {
        let new_disk = Self::make_new(new_name);
        std::fs::rename(&self.path, &new_disk.path)?;
        self.cleanup = false;
        *self = new_disk;
        Ok(())
    }
}

impl Drop for TmpDiskFileInner {
    fn drop(&mut self) {
        if self.cleanup {
            std::fs::remove_file(&self.path).expect("to unlink the tmp file");
        }
    }
}

/// Builder for the Cluster
pub struct ClusterBuilder {
    opts: StartOptions,
    pools: HashMap<u32, Vec<PoolDisk>>,
    replicas: Replica,
    trace: bool,
    env_filter: Option<EnvFilter>,
    bearer_token: Option<String>,
    rest_timeout: std::time::Duration,
    grpc_timeout: TimeoutOptions,
}

#[derive(Default)]
struct Replica {
    count: u32,
    size: u64,
    share: transport::Protocol,
}

/// The default timeout options for every grpc request.
fn grpc_timeout_opts() -> TimeoutOptions {
    TimeoutOptions::default()
        .with_req_timeout(Duration::from_secs(5))
        .with_timeout_backoff(Duration::from_millis(500))
        .with_max_retries(2)
}

impl ClusterBuilder {
    /// Cluster Builder with default options.
    #[must_use]
    pub fn builder() -> Self {
        ClusterBuilder {
            opts: default_options(),
            pools: Default::default(),
            replicas: Default::default(),
            trace: true,
            env_filter: None,
            bearer_token: None,
            rest_timeout: Duration::from_secs(5),
            grpc_timeout: grpc_timeout_opts(),
        }
        .with_default_tracing()
    }
    /// Update the start options.
    #[must_use]
    pub fn with_options<F>(mut self, set: F) -> Self
    where
        F: Fn(StartOptions) -> StartOptions,
    {
        self.opts = set(self.opts);
        self
    }
    /// Update the start options, if enabled.
    #[must_use]
    pub fn with_options_en<F>(mut self, enabled: bool, set: F) -> Self
    where
        F: Fn(StartOptions) -> StartOptions,
    {
        if enabled {
            self.opts = set(self.opts);
        }
        self
    }
    /// Enable/Disable the default tokio tracing setup.
    #[must_use]
    pub fn with_default_tracing(self) -> Self {
        self.with_tracing_filter(
            Self::rust_log_add_quiet_defaults(
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug")),
            )
            .to_string()
            .as_str(),
        )
    }
    /// Silence stor_port and deployer_cluster traces by setting them to WARN.
    #[must_use]
    pub fn with_silence_test_traces(mut self) -> Self {
        self.env_filter = self.env_filter.map(|f| {
            f.add_directive(Directive::from_str("stor_port=warn").unwrap())
                .add_directive(Directive::from_str("deployer_cluster=warn").unwrap())
                .add_directive(Directive::from_str("h2=off").unwrap())
        });
        self
    }
    fn rust_log_add_quiet_defaults(
        current: tracing_subscriber::EnvFilter,
    ) -> tracing_subscriber::EnvFilter {
        let main = match current.to_string().as_str() {
            "debug" => "debug",
            "trace" => "trace",
            _ => return current,
        };
        let logs = format!("{main},{RUST_LOG_SILENCE_DEFAULTS}");
        tracing_subscriber::EnvFilter::new(logs)
    }
    /// Enable/Disable jaeger tracing.
    #[must_use]
    pub fn with_jaeger_tracing(mut self, enabled: bool) -> Self {
        self.trace = enabled;
        self
    }
    /// Use the provided filter for tracing.
    #[must_use]
    pub fn with_tracing_filter<'a>(mut self, filter: impl Into<Option<&'a str>>) -> Self {
        self.env_filter = filter.into().map(tracing_subscriber::EnvFilter::new);
        self
    }
    /// Rest request timeout.
    #[must_use]
    pub fn with_rest_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.rest_timeout = timeout;
        self
    }
    /// Add `count` malloc pools (100MiB size) to each node.
    #[must_use]
    pub fn with_pools(mut self, count: u32) -> Self {
        for _ in 0..count {
            for node in 0..self.opts.io_engines {
                if let Some(pools) = self.pools.get_mut(&node) {
                    pools.push(PoolDisk::Malloc(100 * 1024 * 1024));
                } else {
                    self.pools
                        .insert(node, vec![PoolDisk::Malloc(100 * 1024 * 1024)]);
                }
            }
        }
        self
    }
    /// Add pool URI with `disk` to the node `index`.
    #[must_use]
    pub fn with_pool(mut self, index: u32, disk: &str) -> Self {
        if let Some(pools) = self.pools.get_mut(&index) {
            pools.push(PoolDisk::Uri(disk.to_string()));
        } else {
            self.pools
                .insert(index, vec![PoolDisk::Uri(disk.to_string())]);
        }
        self
    }
    /// Add a tmpfs img pool with `disk` to each io-engine node with the specified `size`.
    #[must_use]
    pub fn with_tmpfs_pool(mut self, size: u64) -> Self {
        for node in 0..self.opts.io_engines {
            self = self.with_tmpfs_pool_ix(node, size);
        }
        self
    }
    /// Add a tmpfs img pool with `disk` to the indexed io-engine node with the specified `size`.
    #[must_use]
    pub fn with_tmpfs_pool_ix(mut self, node: u32, size: u64) -> Self {
        // If we use a new file everytime it can also pollute the workspace when
        // we don't clean up properly...
        // Let's try to reuse the same disk for now and see how it goes..
        let name = format!("index-{index}-node-{node}", index = self.pools.len());
        let disk = TmpDiskFile::new(&name, size);
        if let Some(pools) = self.pools.get_mut(&node) {
            pools.push(PoolDisk::Tmp(disk));
        } else {
            self.pools.insert(node, vec![PoolDisk::Tmp(disk)]);
        }
        self
    }
    /// Specify `count` replicas to add to each node per pool.
    #[must_use]
    pub fn with_replicas(mut self, count: u32, size: u64, share: transport::Protocol) -> Self {
        self.replicas = Replica { count, size, share };
        self
    }
    /// Specify `count` io_engines for the cluster.
    #[must_use]
    pub fn with_io_engines(mut self, count: u32) -> Self {
        self.opts = self.opts.with_io_engines(count);
        self
    }
    /// Specify the image pull policy.
    #[must_use]
    pub fn with_pull_policy(mut self, policy: ImagePullPolicy) -> Self {
        self.opts = self.opts.with_pull_policy(policy);
        self
    }
    /// Specify which agents to use.
    #[must_use]
    pub fn with_agents(mut self, agents: Vec<&str>) -> Self {
        self.opts = self.opts.with_agents(agents);
        self
    }
    /// Specify the node deadline for the node agent.
    /// eg: 2s.
    #[must_use]
    pub fn with_node_deadline(mut self, deadline: &str) -> Self {
        self.opts = self.opts.with_node_deadline(deadline);
        self
    }
    /// The period at which the registry updates its cache of all
    /// resources from all nodes.
    #[must_use]
    pub fn with_cache_period(mut self, period: &str) -> Self {
        self.opts = self.opts.with_cache_period(period);
        self
    }

    /// With reconcile periods:
    /// `busy` for when there's work that needs to be retried on the next poll
    /// `idle` when there's no work pending.
    #[must_use]
    pub fn with_reconcile_period(mut self, busy: Duration, idle: Duration) -> Self {
        self.opts = self.opts.with_reconcile_period(busy, idle);
        self
    }
    #[must_use]
    pub fn with_faulted_child_wait_period(mut self, twait: Duration) -> Self {
        self.opts = self.opts.with_faulted_child_wait_period(twait);
        self
    }

    /// With store operation timeout.
    #[must_use]
    pub fn with_store_timeout(mut self, timeout: Duration) -> Self {
        self.opts = self.opts.with_store_timeout(timeout);
        self
    }
    /// With store lease ttl.
    #[must_use]
    pub fn with_store_lease_ttl(mut self, ttl: Duration) -> Self {
        self.opts = self.opts.with_store_lease_ttl(ttl);
        self
    }
    /// Specify the node connect and request timeouts.
    #[must_use]
    pub fn with_req_timeouts(mut self, connect: Duration, request: Duration) -> Self {
        self.opts = self.opts.with_req_timeouts(true, connect, request);
        self
    }
    /// Specify the node connect and request timeouts and whether to use minimum timeouts or not.
    #[must_use]
    pub fn with_req_timeouts_min(
        mut self,
        no_min: bool,
        connect: Duration,
        request: Duration,
    ) -> Self {
        self.opts = self.opts.with_req_timeouts(no_min, connect, request);
        self
    }
    /// Specify the message grpc timeout options.
    #[must_use]
    pub fn with_grpc_timeouts(mut self, timeout: TimeoutOptions) -> Self {
        self.grpc_timeout = timeout;
        self
    }
    /// Specify whether rest is enabled or not.
    #[must_use]
    pub fn with_rest(mut self, enabled: bool) -> Self {
        self.opts = self.opts.with_rest(enabled, None);
        self
    }
    /// Specify which csi components should be enabled.
    #[must_use]
    pub fn with_csi(mut self, controller: bool, node: bool) -> Self {
        self.opts = self.opts.with_csi(controller, node);
        self
    }
    /// Specify whether csi node registration should be enabled.
    #[must_use]
    pub fn with_csi_registration(mut self, opt: bool) -> Self {
        self.opts = self.opts.with_csi_registration(opt);
        self
    }
    /// Specify whether jaeger is enabled or not.
    #[must_use]
    pub fn with_jaeger(mut self, enabled: bool) -> Self {
        self.opts = self.opts.with_jaeger(enabled);
        if !enabled {
            self.trace = false;
        }
        self
    }
    /// Specify whether rest is enabled or not and whether to use authentication or not.
    #[must_use]
    pub fn with_rest_auth(mut self, enabled: bool, jwk: Option<String>) -> Self {
        self.opts = self.opts.with_rest(enabled, jwk);
        self
    }
    /// Specify whether the components should be cargo built or not.
    #[must_use]
    pub fn with_build(mut self, enabled: bool) -> Self {
        self.opts = self.opts.with_build(enabled);
        self
    }
    /// Specify whether the workspace binaries should be cargo built or not.
    #[must_use]
    pub fn with_build_all(mut self, enabled: bool) -> Self {
        self.opts = self.opts.with_build_all(enabled);
        self
    }
    /// Specify whether eventing is enabled or not.
    pub fn with_eventing(mut self, enabled: bool) -> Self {
        self.opts = self.opts.with_eventing(enabled);
        self
    }
    /// Build into the resulting Cluster using a composer closure, eg:
    /// .compose_build(|c| c.with_logs(false)).
    pub async fn compose_build<F>(mut self, set: F) -> Result<Cluster, Error>
    where
        F: Fn(Builder) -> Builder,
    {
        let (components, composer) = self.build_prepare()?;
        let composer = set(composer);
        let mut cluster = self.new_cluster(components, composer).await?;
        cluster.builder = self;
        Ok(cluster)
    }
    /// Build into the resulting Cluster.
    pub async fn build(mut self) -> Result<Cluster, Error> {
        let (components, composer) = self.build_prepare()?;
        let mut cluster = self.new_cluster(components, composer).await?;
        cluster.builder = self;
        Ok(cluster)
    }
    fn build_prepare(&self) -> Result<(Components, Builder), Error> {
        // Ensure that the composer is initialised with the correct root path.
        composer::initialize(
            std::path::Path::new(std::env!("WORKSPACE_ROOT"))
                .to_str()
                .unwrap(),
        );
        let components = Components::new(self.opts.clone());
        let composer = Builder::new()
            .name(&self.opts.cluster_label.name())
            .configure(components.clone())?
            .with_base_image(self.opts.base_image.clone())
            .autorun(false)
            .with_clean(true)
            // test script will clean up containers if ran on CI/CD
            .with_clean_on_panic(false)
            .with_logs(true);
        Ok((components, composer))
    }

    async fn new_cluster(
        &mut self,
        components: Components,
        compose_builder: Builder,
    ) -> Result<Cluster, Error> {
        let subscriber = Registry::default()
            // todo: add env filter as an optional layer
            .with(self.env_filter.take().unwrap())
            .with(tracing_subscriber::fmt::layer());

        let mut tracing_tags = vec![];
        let trace_guard = Arc::new(match self.trace {
            true => {
                tracing_tags.append(&mut default_tracing_tags(
                    utils::raw_version_str(),
                    env!("CARGO_PKG_VERSION"),
                ));
                tracing_tags.dedup();
                tracing_tags.push(KeyValue::new(
                    opentelemetry_semantic_conventions::resource::SERVICE_NAME,
                    "cluster-client".to_owned(),
                ));

                global::set_text_map_propagator(TraceContextPropagator::new());
                let provider = opentelemetry_otlp::new_pipeline()
                    .tracing()
                    .with_exporter(
                        opentelemetry_otlp::new_exporter()
                            .tonic()
                            .with_endpoint("http://127.0.0.1:4317"),
                    )
                    .with_trace_config(
                        sdktrace::Config::default().with_resource(Resource::new(tracing_tags)),
                    )
                    // TODO: there's currently a few bugs on opentelemetry
                    // 1. We can't use simple exporter on a tokio environment
                    // 2. Even wit the tokio batch exporter, we can't shutdown properly,
                    // meaning that we might not flush traces to jaeger :(
                    .install_batch(opentelemetry_sdk::runtime::TokioCurrentThread)
                    .expect("Should be able to initialise the exporter");
                global::set_tracer_provider(provider.clone());
                let tracer = provider.tracer("tracing-otel-subscriber");
                let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
                tracing::subscriber::set_default(subscriber.with(telemetry))
            }
            false => tracing::subscriber::set_default(subscriber),
        });

        let composer = ComposeTestNt::new(compose_builder).await?;

        let cluster = Cluster::new(
            self.trace,
            trace_guard,
            self.rest_timeout,
            self.grpc_timeout.clone(),
            self.bearer_token.clone(),
            components,
            composer,
        )
        .await?;

        if self.opts.show_info {
            for container in cluster.composer.list_cluster_containers().await? {
                let networks = container.network_settings.unwrap().networks.unwrap();
                let ip = networks
                    .get(&self.opts.cluster_label.name())
                    .unwrap()
                    .ip_address
                    .clone();
                tracing::debug!(
                    "{:?} [{}] {}",
                    container.names.clone().unwrap_or_default(),
                    ip.clone().unwrap_or_default(),
                    option_str(container.command.clone())
                );
            }
        }

        for pool in &self.pools() {
            let pool_client = cluster.grpc_client().pool();
            let replica_client = cluster.grpc_client().replica();
            pool_client
                .create(
                    &CreatePool {
                        node: pool.node.clone().into(),
                        id: pool.id(),
                        disks: vec![pool.disk()],
                        labels: None,
                        encryption: None,
                    },
                    None,
                )
                .await
                .unwrap();

            for replica in &pool.replicas {
                replica_client.create(replica, None).await.unwrap();
            }
        }

        Ok(cluster)
    }
    fn pools(&self) -> Vec<Pool> {
        let mut pools = vec![];

        for (node, i_pools) in &self.pools {
            for (pool_index, pool) in i_pools.iter().enumerate() {
                let mut pool = Pool {
                    node: IoEngine::name(*node, &self.opts),
                    disk: pool.clone(),
                    index: (pool_index + 1) as u32,
                    replicas: vec![],
                };
                for replica_index in 0..self.replicas.count {
                    let rep_id = Cluster::replica(*node, pool_index, replica_index);
                    pool.replicas.push(transport::CreateReplica {
                        node: pool.node.clone().into(),
                        name: None,
                        uuid: rep_id,
                        pool_id: pool.id(),
                        pool_uuid: None,
                        size: self.replicas.size,
                        thin: false,
                        share: self.replicas.share,
                        managed: false,
                        ..Default::default()
                    });
                }
                pools.push(pool);
            }
        }
        pools
    }
}

struct Pool {
    node: String,
    disk: PoolDisk,
    index: u32,
    replicas: Vec<transport::CreateReplica>,
}

impl Pool {
    fn id(&self) -> transport::PoolId {
        format!("{}-pool-{}", self.node, self.index).into()
    }
    fn disk(&self) -> transport::PoolDeviceUri {
        match &self.disk {
            PoolDisk::Malloc(size) => {
                let size = size / (1024 * 1024);
                format!(
                    "malloc:///disk{}?size_mb={}&uuid={}",
                    self.index,
                    size,
                    transport::PoolId::new()
                )
                .into()
            }
            PoolDisk::Uri(uri) => uri.into(),
            PoolDisk::Tmp(disk) => disk.uri().into(),
        }
    }
}

fn grpc_addr(ip: String) -> String {
    format!("https://{ip}:50051")
}

/// Bundles both the csi and the internal node service.
pub struct CsiNodeClient {
    csi: csi_driver::csi::node_client::NodeClient<tonic::transport::Channel>,
    internal:
        csi_driver::node::internal::node_plugin_client::NodePluginClient<tonic::transport::Channel>,
}
impl CsiNodeClient {
    /// Get a mutable reference to the node-plugin csi client.
    pub fn csi(
        &mut self,
    ) -> &mut csi_driver::csi::node_client::NodeClient<tonic::transport::Channel> {
        &mut self.csi
    }
    /// Get a mutable reference to the node-plugin internal node client.
    pub fn internal(
        &mut self,
    ) -> &mut csi_driver::node::internal::node_plugin_client::NodePluginClient<
        tonic::transport::Channel,
    > {
        &mut self.internal
    }
    /// Stage the given volume.
    pub async fn node_stage_volume(
        &mut self,
        volume: &Volume,
        publish_context: HashMap<String, String>,
    ) -> Result<NodeStageVolumeResponse, Error> {
        let mut context = std::collections::HashMap::new();
        context.insert(
            "uri".into(),
            volume.state.target.as_ref().unwrap().device_uri.to_string(),
        );
        context.extend(publish_context);
        let request = rpc::csi::NodeStageVolumeRequest {
            volume_id: volume.spec.uuid.to_string(),
            publish_context: context,
            staging_target_path: "unused".to_string(),
            volume_capability: Some(rpc::csi::VolumeCapability {
                access_mode: Some(rpc::csi::volume_capability::AccessMode {
                    mode: rpc::csi::volume_capability::access_mode::Mode::SingleNodeWriter as i32,
                }),
                access_type: Some(rpc::csi::volume_capability::AccessType::Block(
                    rpc::csi::volume_capability::BlockVolume {},
                )),
            }),
            secrets: Default::default(),
            volume_context: Default::default(),
        };
        let response = self.csi.node_stage_volume(request).await?;
        Ok(response.into_inner())
    }
    /// Stage the given filesystem volume.
    pub async fn node_stage_volume_fs(
        &mut self,
        volume: &Volume,
        fs_type: &str,
        publish_context: HashMap<String, String>,
    ) -> Result<NodeStageVolumeResponse, Error> {
        let mut context = std::collections::HashMap::new();
        context.insert(
            "uri".into(),
            volume.state.target.as_ref().unwrap().device_uri.to_string(),
        );
        context.extend(publish_context);
        let request = rpc::csi::NodeStageVolumeRequest {
            volume_id: volume.spec.uuid.to_string(),
            publish_context: context,
            staging_target_path: format!("/var/tmp/staging/mount/{}", volume.spec.uuid),
            volume_capability: Some(rpc::csi::VolumeCapability {
                access_mode: Some(rpc::csi::volume_capability::AccessMode {
                    mode: rpc::csi::volume_capability::access_mode::Mode::SingleNodeWriter as i32,
                }),
                access_type: Some(rpc::csi::volume_capability::AccessType::Mount(
                    rpc::csi::volume_capability::MountVolume {
                        fs_type: fs_type.to_string(),
                        mount_flags: vec![],
                        volume_mount_group: "".to_string(),
                    },
                )),
            }),
            secrets: Default::default(),
            volume_context: Default::default(),
        };
        let response = self.csi.node_stage_volume(request).await?;
        Ok(response.into_inner())
    }
    /// Unstage the given volume.
    pub async fn node_unstage_volume(
        &mut self,
        volume: &Volume,
    ) -> Result<NodeUnstageVolumeResponse, Error> {
        let request = rpc::csi::NodeUnstageVolumeRequest {
            volume_id: volume.spec.uuid.to_string(),
            staging_target_path: format!("/var/tmp/staging/mount/{}", volume.spec.uuid),
        };
        let response = self.csi.node_unstage_volume(request).await?;
        Ok(response.into_inner())
    }
    /// Stage the given volume.
    pub async fn node_publish_volume(
        &mut self,
        volume: &Volume,
        publish_context: HashMap<String, String>,
    ) -> Result<rpc::csi::NodePublishVolumeResponse, Error> {
        std::fs::create_dir_all("/var/tmp/target/mount")?;

        let request = rpc::csi::NodePublishVolumeRequest {
            volume_id: volume.spec.uuid.to_string(),
            publish_context,
            staging_target_path: format!("/var/tmp/staging/mount/{}", volume.spec.uuid),
            target_path: format!("/var/tmp/target/mount/{}", volume.spec.uuid),
            volume_capability: Some(rpc::csi::VolumeCapability {
                access_mode: Some(rpc::csi::volume_capability::AccessMode {
                    mode: rpc::csi::volume_capability::access_mode::Mode::SingleNodeWriter as i32,
                }),
                access_type: Some(rpc::csi::volume_capability::AccessType::Block(
                    rpc::csi::volume_capability::BlockVolume {},
                )),
            }),
            readonly: false,
            secrets: Default::default(),
            volume_context: Default::default(),
        };
        let response = self.csi.node_publish_volume(request).await?;
        Ok(response.into_inner())
    }
    /// Publish the given volume.
    pub async fn node_publish_volume_fs(
        &mut self,
        volume: &Volume,
        fs_type: &str,
        publish_context: HashMap<String, String>,
    ) -> Result<rpc::csi::NodePublishVolumeResponse, Error> {
        std::fs::create_dir_all("/var/tmp/target/mount")?;

        let request = rpc::csi::NodePublishVolumeRequest {
            volume_id: volume.spec.uuid.to_string(),
            publish_context,
            staging_target_path: format!("/var/tmp/staging/mount/{}", volume.spec.uuid),
            target_path: format!("/var/tmp/target/mount/{}", volume.spec.uuid),
            volume_capability: Some(rpc::csi::VolumeCapability {
                access_mode: Some(rpc::csi::volume_capability::AccessMode {
                    mode: rpc::csi::volume_capability::access_mode::Mode::SingleNodeWriter as i32,
                }),
                access_type: Some(rpc::csi::volume_capability::AccessType::Mount(
                    rpc::csi::volume_capability::MountVolume {
                        fs_type: fs_type.to_string(),
                        mount_flags: vec![],
                        volume_mount_group: "".to_string(),
                    },
                )),
            }),
            readonly: false,
            secrets: Default::default(),
            volume_context: Default::default(),
        };
        let response = self.csi.node_publish_volume(request).await?;
        Ok(response.into_inner())
    }
    /// Unpublish the given volume.
    pub async fn node_unpublish_volume(
        &mut self,
        volume: &Volume,
    ) -> Result<rpc::csi::NodeUnpublishVolumeResponse, Error> {
        let request = rpc::csi::NodeUnpublishVolumeRequest {
            volume_id: volume.spec.uuid.to_string(),
            target_path: format!("/var/tmp/target/mount/{}", volume.spec.uuid),
        };
        let response = self.csi.node_unpublish_volume(request).await?;

        std::fs::remove_dir_all("/var/tmp/target/mount")?;

        Ok(response.into_inner())
    }
}

const CSI_SOCKET: &str = "/var/tmp/csi-controller.sock";

/// Bundles the csi controller client.
pub struct CsiControllerClient {
    csi: csi_driver::csi::controller_client::ControllerClient<tonic::transport::Channel>,
}

impl CsiControllerClient {
    /// Get a mutable reference to the csi controller client.
    pub fn csi(
        &mut self,
    ) -> &mut csi_driver::csi::controller_client::ControllerClient<tonic::transport::Channel> {
        &mut self.csi
    }

    /// Create the given snapshot.
    pub async fn create_snapshot(
        &mut self,
        volume: &Volume,
        snap_uuid: &str,
        enable_fs_quiesce: bool,
    ) -> Result<CreateSnapshotResponse, Error> {
        let mut map = HashMap::new();
        if enable_fs_quiesce {
            map.insert("quiesceFs".to_string(), "freeze".to_string());
        } else {
            map.insert("quiesceFs".to_string(), "none".to_string());
        }
        let request = rpc::csi::CreateSnapshotRequest {
            source_volume_id: volume.spec.uuid.to_string(),
            name: snap_uuid.to_string(),
            secrets: Default::default(),
            parameters: map,
        };
        let response = self.csi().create_snapshot(request).await?;
        Ok(response.into_inner())
    }

    /// Delete the given snapshot.
    pub async fn delete_snapshot(
        &mut self,
        snap_uuid: &str,
    ) -> Result<rpc::csi::DeleteSnapshotResponse, Error> {
        let request = rpc::csi::DeleteSnapshotRequest {
            snapshot_id: snap_uuid.to_string(),
            secrets: Default::default(),
        };
        let response = self.csi().delete_snapshot(request).await?;
        Ok(response.into_inner())
    }

    /// Controller Publish the given fs volume.
    pub async fn controller_publish_volume_fs(
        &mut self,
        volume: &Volume,
        fs_type: &str,
        node_id: &str,
    ) -> Result<rpc::csi::ControllerPublishVolumeResponse, Error> {
        let request = rpc::csi::ControllerPublishVolumeRequest {
            volume_id: volume.spec.uuid.to_string(),
            volume_capability: Some(rpc::csi::VolumeCapability {
                access_mode: Some(rpc::csi::volume_capability::AccessMode {
                    mode: rpc::csi::volume_capability::access_mode::Mode::SingleNodeWriter as i32,
                }),
                access_type: Some(rpc::csi::volume_capability::AccessType::Mount(
                    rpc::csi::volume_capability::MountVolume {
                        fs_type: fs_type.to_string(),
                        mount_flags: vec![],
                        volume_mount_group: "".to_string(),
                    },
                )),
            }),
            readonly: false,
            secrets: Default::default(),
            volume_context: Default::default(),
            node_id: node_id.to_string(),
        };
        let response = self.csi().controller_publish_volume(request).await?;
        Ok(response.into_inner())
    }

    /// Controller Publish the given volume.
    pub async fn controller_publish_volume(
        &mut self,
        volume: &Volume,
        node_id: &str,
    ) -> Result<rpc::csi::ControllerPublishVolumeResponse, Error> {
        let request = rpc::csi::ControllerPublishVolumeRequest {
            volume_id: volume.spec.uuid.to_string(),
            volume_capability: Some(rpc::csi::VolumeCapability {
                access_mode: Some(rpc::csi::volume_capability::AccessMode {
                    mode: rpc::csi::volume_capability::access_mode::Mode::SingleNodeWriter as i32,
                }),
                access_type: Some(rpc::csi::volume_capability::AccessType::Block(
                    rpc::csi::volume_capability::BlockVolume {},
                )),
            }),
            readonly: false,
            secrets: Default::default(),
            volume_context: Default::default(),
            node_id: node_id.to_string(),
        };
        let response = self.csi().controller_publish_volume(request).await?;
        Ok(response.into_inner())
    }

    /// Controller Unpublish the given volume.
    pub async fn controller_unpublish_volume(
        &mut self,
        volume: &Volume,
        node_id: &str,
    ) -> Result<rpc::csi::ControllerUnpublishVolumeResponse, Error> {
        let request = rpc::csi::ControllerUnpublishVolumeRequest {
            volume_id: volume.spec.uuid.to_string(),
            node_id: node_id.to_string(),
            secrets: Default::default(),
        };
        let response = self.csi().controller_unpublish_volume(request).await?;
        Ok(response.into_inner())
    }
}
