//! The Core Agent.
//! todo: document.
/// The app node related operations.
pub(crate) mod app_node;
/// The controller logic for all resources.
pub(crate) mod controller;
/// The nexus related operations.
pub(crate) mod nexus;
/// The node related operations.
pub(crate) mod node;
/// The pool related operations.
pub(crate) mod pool;
/// The registry which contains all the resources.
pub(crate) mod registry;
/// The volume related operations.
pub(crate) mod volume;
/// The watch related operations.
pub(crate) mod watch;

use clap::Parser;
use controller::registry::NumRebuilds;
use std::{net::SocketAddr, num::ParseIntError};
use utils::{version_info_str, DEFAULT_GRPC_SERVER_ADDR, ETCD_MAX_PAGE_LIMIT};

use stor_port::HostAccessControl;
use utils::tracing_telemetry::{trace::TracerProvider, FmtLayer, FmtStyle, KeyValue};

/// The Cli arguments for this binary.
#[derive(Debug, Parser)]
#[structopt(name = utils::package_description!(), version = version_info_str!())]
pub(crate) struct CliArgs {
    /// The period at which the registry updates its cache of all
    /// resources from all nodes.
    #[clap(long, short, default_value = utils::CACHE_POLL_PERIOD)]
    pub(crate) cache_period: humantime::Duration,

    /// The period at which the reconcile loop checks for new work.
    #[clap(long, default_value = "30s")]
    pub(crate) reconcile_idle_period: humantime::Duration,

    /// The period at which the reconcile loop attempts to do work.
    #[clap(long, default_value = "10s")]
    pub(crate) reconcile_period: humantime::Duration,

    /// The duration for which the reconciler waits for the replica to
    /// to be healthy again before attempting to online the faulted child.
    #[clap(long)]
    pub(crate) faulted_child_wait_period: Option<humantime::Duration>,

    /// When the pool creation gRPC times out, the actual call in the io-engine
    /// may still progress.
    /// We wait up to this period before considering the operation a failure and
    /// GC'ing the pool.
    #[clap(long, default_value = "15m")]
    pub(crate) pool_async_creat_tmo: humantime::Duration,

    /// Disable partial rebuild for volume targets.
    #[clap(long, env = "DISABLE_PARTIAL_REBUILD")]
    pub(crate) disable_partial_rebuild: bool,

    /// Disable the access control which is used to gate access to the replicas and nexuses,
    /// namely the reservations and allow hosts.
    #[clap(long, env = "DISABLE_TARGET_ACC")]
    pub(crate) disable_target_acc: bool,

    /// Deadline for the io-engine instance keep alive registration.
    #[clap(long, short, default_value = "10s")]
    pub(crate) deadline: humantime::Duration,

    /// The Persistent Store URLs to connect to.
    /// (supports the http/https schema)
    #[clap(long, short, default_value = "http://localhost:2379")]
    pub(crate) store: String,

    /// The timeout for store operations.
    #[clap(long, default_value = utils::STORE_OP_TIMEOUT)]
    pub(crate) store_timeout: humantime::Duration,

    /// The lease lock ttl for the persistent store after which we'll lose the exclusive access.
    #[clap(long, default_value = utils::STORE_LEASE_LOCK_TTL)]
    pub(crate) store_lease_ttl: humantime::Duration,

    /// The timeout for every node connection (gRPC).
    #[clap(long, default_value = utils::DEFAULT_CONN_TIMEOUT)]
    pub(crate) connect_timeout: humantime::Duration,

    /// The default timeout for node request timeouts (gRPC).
    #[clap(long, short, default_value = utils::DEFAULT_REQ_TIMEOUT)]
    pub(crate) request_timeout: humantime::Duration,

    /// The maximum number of concurrent create volume requests.
    #[clap(long, default_value = "10")]
    create_volume_limit: usize,

    /// Control hosts access control via their NQN's.
    #[clap(long, use_value_delimiter = true, default_value = utils::DEFAULT_HOST_ACCESS_CONTROL)]
    pub(crate) hosts_acl: Vec<HostAccessControl>,

    /// Add process service tags to the traces.
    #[clap(short, long, env = "TRACING_TAGS", value_delimiter=',', value_parser = utils::tracing_telemetry::parse_key_value)]
    tracing_tags: Vec<KeyValue>,

    /// Don't use minimum timeouts for specific requests.
    #[clap(long)]
    no_min_timeouts: bool,
    /// Trace rest requests to the Jaeger endpoint agent.
    #[clap(long, short)]
    jaeger: Option<String>,
    /// The GRPC Server URLs to connect to.
    /// (supports the http/https schema)
    #[clap(long, short, default_value = DEFAULT_GRPC_SERVER_ADDR)]
    pub(crate) grpc_server_addr: SocketAddr,
    /// The maximum number of system-wide rebuilds permitted at any given time.
    /// If `None` do not limit the number of rebuilds.
    #[clap(long)]
    max_rebuilds: Option<NumRebuilds>,

    #[clap(flatten)]
    thin_args: ThinArgs,

    /// Events message-bus endpoint url.
    #[clap(long, short)]
    events_url: Option<url::Url>,

    /// Disable the HA/Failover feature.
    /// This is useful when the frontend nodes do not support the NVMe ANA feature.
    #[clap(long, env = "HA_DISABLED")]
    pub(crate) disable_ha: bool,

    /// Etcd Pagination Limit.
    #[clap(long, default_value = ETCD_MAX_PAGE_LIMIT)]
    pub(crate) etcd_page_limit: u32,

    /// Formatting style to be used while logging.
    #[clap(default_value = FmtStyle::Pretty.as_ref(), short, long)]
    fmt_style: FmtStyle,

    /// Use ANSI colors for the logs.
    #[clap(long, default_value_t = true, action = clap::ArgAction::Set)]
    ansi_colors: bool,
}
impl CliArgs {
    fn args() -> Self {
        CliArgs::parse()
    }
}

/// Cluster wide thin provisioning parameters.
#[derive(Debug, clap::Parser, Clone)]
pub(crate) struct ThinArgs {
    /// The allowed pool commitment limit when dealing with thin provisioned volumes.
    /// Example: If the commitment is 250 and the pool is 10GiB we can overcommit the pool
    /// up to 25GiB (create 2 10GiB and 1 5GiB volume) but no further.
    #[clap(long, env = "POOL_COMMITMENT_%", value_parser = value_parse_percent, default_value = "250%")]
    pool_commitment: u64,
    /// When creating replicas for an existing volume, each replica pool must have at least
    /// this much free space percentage of the volume size.
    /// Example: if this value is 40, the pool has 40GiB free, then the max volume size allowed
    /// to be created on the pool is 100GiB.
    #[clap(long, env = "VOLUME_COMMITMENT_%", value_parser = value_parse_percent, default_value = "40%")]
    volume_commitment: u64,
    /// When creating snapshots for an existing volume, each replica pool must have at least
    /// this much free space percentage of the volume size.
    /// Example: if this value is 40, the pool has 40GiB free, then the max volume size allowed
    /// to be snapped on the pool is 100GiB.
    #[clap(long, env = "SNAPSHOT_COMMITMENT_%", value_parser = value_parse_percent, default_value = "40%")]
    snapshot_commitment: u64,
    /// Same as the `volume_commitment` argument, but applicable only when creating replicas for
    /// a new volume.
    #[clap(long, env = "VOLUME_COMMITMENT_%_INITIAL", value_parser = value_parse_percent, default_value = "40%")]
    volume_commitment_initial: u64,
}

fn value_parse_percent(value: &str) -> Result<u64, ParseIntError> {
    value.replace('%', "").parse()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli_args = CliArgs::args();
    utils::print_package_info!();
    println!("Using options: {cli_args:?}");
    utils::tracing_telemetry::TracingTelemetry::builder()
        .with_writer(FmtLayer::Stdout)
        .with_style(cli_args.fmt_style)
        .with_colours(cli_args.ansi_colors)
        .with_jaeger(cli_args.jaeger.clone())
        .with_events_url(cli_args.events_url.clone())
        .with_tracing_tags(cli_args.tracing_tags.clone())
        .init("agent-core");
    server(cli_args).await
}

async fn server(cli_args: CliArgs) -> anyhow::Result<()> {
    stor_port::platform::init_cluster_info_or_panic().await;
    let registry = controller::registry::Registry::new(
        cli_args.cache_period.into(),
        cli_args.store.clone(),
        cli_args.store_timeout.into(),
        cli_args.store_lease_ttl.into(),
        cli_args.reconcile_period.into(),
        cli_args.reconcile_idle_period.into(),
        cli_args.faulted_child_wait_period.map(|t| t.into()),
        cli_args.pool_async_creat_tmo.into(),
        cli_args.disable_partial_rebuild,
        cli_args.disable_target_acc,
        cli_args.max_rebuilds,
        cli_args.create_volume_limit,
        if cli_args.hosts_acl.contains(&HostAccessControl::None) {
            vec![]
        } else {
            cli_args.hosts_acl.clone()
        },
        cli_args.thin_args,
        cli_args.disable_ha,
        cli_args.etcd_page_limit as i64,
    )
    .await?;

    let service = agents::Service::builder()
        .with_shared_state(
            utils::tracing_telemetry::global::tracer_provider().versioned_tracer(
                "core-agent",
                Some(env!("CARGO_PKG_VERSION")),
                None::<std::borrow::Cow<'static, str>>,
                None,
            ),
        )
        .with_shared_state(registry.clone())
        .with_shared_state(cli_args.grpc_server_addr)
        .configure_async(node::configure)
        .await
        .configure(pool::configure)
        .configure(nexus::configure)
        .configure(volume::configure)
        .configure(watch::configure)
        .configure(registry::configure)
        .configure(app_node::configure);

    registry.start().await;
    let result = service.run_err(cli_args.grpc_server_addr).await;
    registry.stop().await;
    utils::tracing_telemetry::flush_traces();
    result?;
    Ok(())
}
