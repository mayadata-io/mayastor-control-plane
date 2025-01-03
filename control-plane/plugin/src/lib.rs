#[macro_use]
extern crate prettytable;
#[macro_use]
extern crate lazy_static;

use operations::{Label, SetProperty};
use resources::{error::OutputFormatAbsentSnafu, utils::OutputFormat, LabelResources};
use std::fmt::Debug;
use utils::tracing_telemetry::{FmtLayer, FmtStyle};

use crate::{
    operations::{
        Cordoning, Drain, Get, GetBlockDevices, GetSnapshotTopology, GetSnapshots, GetWithArgs,
        List, ListExt, ListWithArgs, Operations, PluginResult, RebuildHistory, ReplicaTopology,
        Scale,
    },
    resources::{
        blockdevice, cordon, drain, node, pool, snapshot, volume, CordonResources, DrainResources,
        GetCordonArgs, GetDrainArgs, GetResources, ScaleResources, SetPropertyResources,
        SetVolumeProperties, UnCordonResources,
    },
};

pub mod operations;
pub mod resources;
pub mod rest_wrapper;

/// Flush traces on `Drop`.
pub struct TracingFlusher {
    _span: Option<tracing::span::EnteredSpan>,
}
impl Drop for TracingFlusher {
    fn drop(&mut self) {
        {
            let _ = self._span.take();
        }
        utils::tracing_telemetry::flush_traces();
    }
}

/// Every plugin operation must implement this trait to become composable.
#[async_trait::async_trait(?Send)]
pub trait ExecuteOperation {
    type Args;
    type Error;
    async fn execute(
        &self,
        cli_args: impl clap::Parser,
        output: Option<&OutputFormat>,
    ) -> Result<(), Self::Error>;
}

#[derive(Clone, clap::Parser, Debug)]
pub struct CliArgs {
    /// The Output, viz yaml, json.
    #[clap(global = true, default_value = OutputFormat::None.as_ref(), short, long)]
    pub output: OutputFormat,

    /// Trace rest requests to the Jaeger endpoint agent.
    #[clap(global = true, long, short)]
    pub jaeger: Option<String>,

    /// Timeout for the REST operations.
    #[clap(long, short, default_value = "10s")]
    pub timeout: humantime::Duration,
}

impl CliArgs {
    /// Initialize tracing (including opentelemetry).
    pub fn init_tracing(&self) -> TracingFlusher {
        let git_version = option_env!("GIT_VERSION").unwrap_or_else(utils::raw_version_str);
        let tags =
            utils::tracing_telemetry::default_tracing_tags(git_version, env!("CARGO_PKG_VERSION"));

        let fmt_layer = match std::env::var("RUST_LOG") {
            Ok(_) => FmtLayer::Stderr,
            Err(_) => FmtLayer::None,
        };

        let jaeger = self.jaeger.as_ref();

        utils::tracing_telemetry::TracingTelemetry::builder()
            .with_writer(fmt_layer)
            .with_style(FmtStyle::Pretty)
            .with_colours(false)
            .with_jaeger(jaeger.cloned())
            .with_tracing_tags(tags)
            .init(env!("CARGO_PKG_NAME"));

        TracingFlusher {
            _span: jaeger.map(|_| tracing::info_span!(env!("CARGO_PKG_NAME")).entered()),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl ExecuteOperation for Operations {
    type Args = CliArgs;
    type Error = crate::resources::Error;
    async fn execute(
        &self,
        cli_args: impl clap::Parser,
        output: Option<&OutputFormat>,
    ) -> PluginResult {
        match self {
            Operations::Drain(resource) => resource.execute(cli_args, output).await,
            Operations::Get(resource) => resource.execute(cli_args, output).await,
            Operations::Scale(resource) => resource.execute(cli_args, output).await,
            Operations::Set(resource) => resource.execute(cli_args, output).await,
            Operations::Cordon(resource) => resource.execute(cli_args, output).await,
            Operations::Uncordon(resource) => resource.execute(cli_args, output).await,
            Operations::Label(resource) => resource.execute(cli_args, output).await,
        }
    }
}

#[async_trait::async_trait(?Send)]
impl ExecuteOperation for DrainResources {
    type Args = CliArgs;
    type Error = crate::resources::Error;
    async fn execute(
        &self,
        _cli_args: impl clap::Parser,
        output: Option<&OutputFormat>,
    ) -> PluginResult {
        let output = output.ok_or(OutputFormatAbsentSnafu.build())?;
        match self {
            DrainResources::Node(drain_node_args) => {
                node::Node::drain(
                    &drain_node_args.node_id(),
                    drain_node_args.label(),
                    drain_node_args.drain_timeout(),
                    output,
                )
                .await
            }
        }
    }
}

#[async_trait::async_trait(?Send)]
impl ExecuteOperation for GetResources {
    type Args = CliArgs;
    type Error = crate::resources::Error;
    async fn execute(
        &self,
        _cli_args: impl clap::Parser,
        output: Option<&OutputFormat>,
    ) -> PluginResult {
        let output = output.ok_or(OutputFormatAbsentSnafu.build())?;
        match self {
            GetResources::Cordon(get_cordon_resource) => match get_cordon_resource {
                GetCordonArgs::Node { id: node_id } => {
                    cordon::NodeCordon::get(node_id, output).await
                }
                GetCordonArgs::Nodes => cordon::NodeCordons::list(output).await,
            },
            GetResources::Drain(get_drain_resource) => match get_drain_resource {
                GetDrainArgs::Node { id: node_id } => drain::NodeDrain::get(node_id, output).await,
                GetDrainArgs::Nodes => drain::NodeDrains::list(output).await,
            },
            GetResources::Volumes(vol_args) => volume::Volumes::list(output, vol_args).await,
            GetResources::Volume { id } => volume::Volume::get(id, output).await,
            GetResources::RebuildHistory { id } => {
                volume::Volume::rebuild_history(id, output).await
            }
            GetResources::VolumeReplicaTopologies(vol_args) => {
                volume::Volume::topologies(output, vol_args).await
            }
            GetResources::VolumeReplicaTopology { id } => {
                volume::Volume::topology(id, output).await
            }
            GetResources::Pools(args) => pool::Pools::list(args, output).await,
            GetResources::Pool(args) => pool::Pool::get(&args.pool_id(), args, output).await,
            GetResources::Nodes(args) => node::Nodes::list(args, output).await,
            GetResources::Node(args) => node::Node::get(&args.node_id(), args, output).await,
            GetResources::BlockDevices(bdargs) => {
                blockdevice::BlockDevice::get_blockdevices(&bdargs.node_id(), &bdargs.all(), output)
                    .await
            }
            GetResources::VolumeSnapshots(snapargs) => {
                snapshot::VolumeSnapshots::get_snapshots(
                    &snapargs.volume(),
                    &snapargs.snapshot(),
                    output,
                )
                .await
            }
            GetResources::VolumeSnapshotTopology(snapargs) => {
                snapshot::VolumeSnapshots::get_snapshot_topology(
                    &snapargs.volume(),
                    &snapargs.snapshot(),
                    output,
                )
                .await
            }
        }
    }
}

#[async_trait::async_trait(?Send)]
impl ExecuteOperation for ScaleResources {
    type Args = CliArgs;
    type Error = crate::resources::Error;
    async fn execute(
        &self,
        _cli_args: impl clap::Parser,
        output: Option<&OutputFormat>,
    ) -> PluginResult {
        let output = output.ok_or(OutputFormatAbsentSnafu.build())?;
        match self {
            ScaleResources::Volume { id, replica_count } => {
                volume::Volume::scale(id, *replica_count, output).await
            }
        }
    }
}

#[async_trait::async_trait(?Send)]
impl ExecuteOperation for SetPropertyResources {
    type Args = CliArgs;
    type Error = crate::resources::Error;
    async fn execute(
        &self,
        _cli_args: impl clap::Parser,
        output: Option<&OutputFormat>,
    ) -> PluginResult {
        let output = output.ok_or(OutputFormatAbsentSnafu.build())?;
        match self {
            SetPropertyResources::Volume { id, properties } => {
                volume::Volume::set_property(id, properties, output).await
            }
        }
    }
}

#[async_trait::async_trait(?Send)]
impl ExecuteOperation for CordonResources {
    type Args = CliArgs;
    type Error = crate::resources::Error;
    async fn execute(
        &self,
        _cli_args: impl clap::Parser,
        output: Option<&OutputFormat>,
    ) -> PluginResult {
        let output = output.ok_or(OutputFormatAbsentSnafu.build())?;
        match self {
            CordonResources::Node { id, label } => node::Node::cordon(id, label, output).await,
        }
    }
}

#[async_trait::async_trait(?Send)]
impl ExecuteOperation for UnCordonResources {
    type Args = CliArgs;
    type Error = crate::resources::Error;
    async fn execute(
        &self,
        _cli_args: impl clap::Parser,
        output: Option<&OutputFormat>,
    ) -> PluginResult {
        let output = output.ok_or(OutputFormatAbsentSnafu.build())?;
        match self {
            UnCordonResources::Node { id, label } => node::Node::uncordon(id, label, output).await,
        }
    }
}

#[async_trait::async_trait(?Send)]
impl ExecuteOperation for LabelResources {
    type Args = CliArgs;
    type Error = crate::resources::Error;
    async fn execute(
        &self,
        _cli_args: impl clap::Parser,
        output: Option<&OutputFormat>,
    ) -> PluginResult {
        let output = output.ok_or(OutputFormatAbsentSnafu.build())?;
        match self {
            LabelResources::Node {
                id,
                label,
                overwrite,
            } => node::Node::label(id, label.to_string(), *overwrite, output).await,
            LabelResources::Pool {
                id,
                label,
                overwrite,
            } => pool::Pool::label(id, label.to_string(), *overwrite, output).await,
        }
    }
}
