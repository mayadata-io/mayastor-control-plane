//! IoEngine CSI plugin.
//!
//! Implementation of gRPC methods from the CSI spec. This includes mounting
//! of volumes using iscsi/nvmf protocols on the node.

use crate::{
    client::AppNodesClientWrapper,
    error::CsiDriverError,
    fsfreeze::{bin::fsfreeze, FsFreezeOpt},
    identity::Identity,
    k8s::patch_k8s_node,
    mount::probe_filesystems,
    mount_utils::bin::{
        flag::{parse_mount_flags, parse_unmount_flags},
        mount_utils::{mount, unmount},
    },
    node::{Node, RDMA_CONNECT_CHECK},
    nodeplugin_grpc::NodePluginGrpcServer,
    nodeplugin_nvme::NvmeOperationsSvc,
    registration::run_registration_loop,
    shutdown_event::Shutdown,
};
use csi_driver::csi::{identity_server::IdentityServer, node_server::NodeServer};
use grpc::csi_node_nvme::nvme_operations_server::NvmeOperationsServer;
use stor_port::platform;
use utils::tracing_telemetry::{FmtLayer, FmtStyle};

use clap::Arg;
use serde_json::json;
use std::{
    collections::HashMap,
    env, fs,
    future::Future,
    io::ErrorKind,
    net::{IpAddr, SocketAddr},
    str::FromStr,
};
use tokio::net::UnixListener;
use tonic::{codegen::tokio_stream::wrappers::UnixListenerStream, transport::Server};
use tracing::{debug, error, info};

const GRPC_PORT: u16 = 50051;

pub(super) async fn main() -> anyhow::Result<()> {
    // Parse command line arguments.
    let matches = clap::Command::new(utils::package_description!())
        .about("k8s sidecar for IoEngine implementing CSI among others")
        .version(utils::version_info_str!())
        .subcommand_negates_reqs(true)
        .arg(
            Arg::new("rest-endpoint")
                .long("rest-endpoint")
                .env("ENDPOINT")
                .help("A URL endpoint to the control plane's rest endpoint")
        )
        .arg(
            Arg::new("enable-registration")
                .long("enable-registration")
                .action(clap::ArgAction::SetTrue)
                .value_name("BOOLEAN")
                .requires("rest-endpoint")
                .help("Enable registration of the csi node with the control plane")
        )
        .arg(
            Arg::new("csi-socket")
                .short('c')
                .long("csi-socket")
                .value_name("PATH")
                .help("CSI gRPC listen socket (default /var/tmp/csi.sock)")
        )
        .arg(
            Arg::new("node-name")
                .short('n')
                .long("node-name")
                .value_name("NAME")
                .help("Unique node name where this instance runs")
                .required(true)
        )
        .arg(
            Arg::new("grpc-endpoint")
                .short('g')
                .long("grpc-endpoint")
                .value_name("ENDPOINT")
                .conflicts_with_all(["grpc-ip", "grpc-port"])
                .help("ip address where this instance runs, and optionally the gRPC port")
                .required(false)
        )
        .arg(
            Arg::new("grpc-ip")
                .long("grpc-ip")
                .value_parser(clap::value_parser!(IpAddr))
                .value_name("GRPC_IP")
                .help("ip address this instance listens on")
                .default_value("::")
                .required(false)
        )
        .arg(
            Arg::new("grpc-port")
                .long("grpc-port")
                .value_parser(clap::value_parser!(u16))
                .value_name("GRPC_PORT")
                .help("port this instance listens on")
                .default_value(GRPC_PORT.to_string())
                .required(false)
        )
        .arg(
            Arg::new("v")
                .short('v')
                .action(clap::ArgAction::Count)
                .help("Sets the verbosity level")
        )
        .arg(
            Arg::new("nvme-core-io-timeout")
                .long("nvme-core-io-timeout")
                .value_name("TIMEOUT")
                .required(false)
                .help("Sets the global nvme_core module io_timeout, in seconds or humantime")
        )
        .arg(
            Arg::new("nvme-io-timeout")
                .long("nvme-io-timeout")
                .required(false)
                .help("Sets io_timeout for nvme block devices")
        )
        .arg(
            Arg::new(crate::config::nvme_nr_io_queues())
                .long(crate::config::nvme_nr_io_queues())
                .value_name("NUMBER")
                .required(false)
                .help("Sets the nvme-nr-io-queues parameter when connecting to a volume target")
        )
        .arg(
            Arg::new(crate::config::nvme_ctrl_loss_tmo())
                .long(crate::config::nvme_ctrl_loss_tmo())
                .value_name("NUMBER")
                .required(false)
                .help("Sets the nvme-ctrl-loss-tmo parameter when connecting to a volume target. (May be overridden through the storage class)")
        )
        .arg(
            Arg::new(crate::config::nvme_keep_alive_tmo())
                .long(crate::config::nvme_keep_alive_tmo())
                .value_name("NUMBER")
                .required(false)
                .help("Sets the nvme-keep-alive-tmo parameter when connecting to a volume target")
        )
        .arg(
            Arg::new("node-selector")
                .long("node-selector")
                .action(clap::ArgAction::Append)
                .num_args(1)
                .allow_hyphen_values(true)
                .default_value(csi_driver::csi_node_selector())
                .help(
                    "The node selector label which this plugin will report as part of its topology.\n\
                    Example:\n --node-selector key=value --node-selector key2=value2",
                )
            ,
        )
        .arg(
            Arg::new("fmt-style")
                .long("fmt-style")
                .default_value(FmtStyle::Pretty.as_ref())
                .value_parser(clap::value_parser!(FmtStyle))
                .help("Formatting style to be used while logging")
        )
        .arg(
            Arg::new("ansi-colors")
                .long("ansi-colors")
                .default_value("true")
                .value_parser(clap::value_parser!(bool))
                .help("Enable ansi color for logs")
        )
        .arg(
            Arg::new("nvme-connect-fallback")
                .long("nvme-connect-fallback")
                .default_value("false")
                .value_parser(clap::value_parser!(bool))
                .help(
                    "Enable falling back to nvme connect over tcp if initiator node is not rdma capable, \n\
                    even though volume target is rdma capable."
            )
        )
        .subcommand(
            clap::Command::new("fs-freeze")
                .arg(
                    Arg::new("volume-id")
                        .short('v')
                        .long("volume-id")
                        .value_name("UUID")
                        .required(true)
                        .help("Uuid of the volume to freeze")
                )
        )
        .subcommand(
            clap::Command::new("fs-unfreeze")
                .arg(
                    Arg::new("volume-id")
                        .short('v')
                        .long("volume-id")
                        .value_name("UUID")
                        .required(true)
                        .help("Uuid of the volume to unfreeze")
                )
        )
        .subcommand(
            clap::Command::new("mount")
                .arg(
                    Arg::new("source")
                        .long("source")
                        .value_name("PATH")
                        .required(true)
                        .help("Mount source path")
                )
                .arg(
                    Arg::new("target")
                        .long("target")
                        .value_name("PATH")
                        .required(true)
                        .help("Mount target path")
                )
                .arg(
                    Arg::new("fstype")
                        .long("fstype")
                        .value_name("STRING")
                        .help("Filesystem type for filesystem volume")
                )
                .arg(
                    Arg::new("data")
                        .long("data")
                        .value_name("STRING")
                        .help("Options to apply for the filesystem on mount")
                )
                .arg(
                    Arg::new("mount-flags")
                        .long("mount-flags")
                        .value_name("STRING")
                        .value_parser(parse_mount_flags)
                        .required(true)
                        .help("Mount flags")
                )
        )
        .subcommand(
            clap::Command::new("unmount")
                .arg(
                    Arg::new("target")
                        .long("target")
                        .value_name("PATH")
                        .required(true)
                        .help("Unmount target path")
                )
                .arg(
                    Arg::new("unmount-flags")
                        .long("unmount-flags")
                        .value_name("STRING")
                        .value_parser(parse_unmount_flags)
                        .required(true)
                        .help("Unmount flags")
                )
        )
        .get_matches();
    let tags = utils::tracing_telemetry::default_tracing_tags(
        utils::raw_version_str(),
        env!("CARGO_PKG_VERSION"),
    );

    // Handle fs-freeze and fs-unfreeze commands.
    if let Some(cmd) = matches.subcommand() {
        utils::tracing_telemetry::TracingTelemetry::builder()
            .with_writer(FmtLayer::Stderr)
            .with_style(FmtStyle::Compact)
            .with_colours(false)
            .with_tracing_tags(tags)
            .init("csi-node");
        match cmd {
            ("fs-freeze", arg_matches) => {
                let volume_id = arg_matches.get_one::<String>("volume-id").unwrap();
                fsfreeze(volume_id, FsFreezeOpt::Freeze)
                    .await
                    .map_err(|error| CsiDriverError::Fsfreeze { source: error })
            }
            ("fs-unfreeze", arg_matches) => {
                let volume_id = arg_matches.get_one::<String>("volume-id").unwrap();
                fsfreeze(volume_id, FsFreezeOpt::Unfreeze)
                    .await
                    .map_err(|error| CsiDriverError::Fsfreeze { source: error })
            }
            ("mount", arg_matches) => {
                let src_path = arg_matches.get_one::<String>("source").unwrap();
                let dsc_path = arg_matches.get_one::<String>("target").unwrap();
                let fstype = arg_matches.get_one::<String>("fstype");
                let data = arg_matches.get_one::<String>("data");
                let mnt_flags = arg_matches
                    .get_one::<sys_mount::MountFlags>("mount-flags")
                    .unwrap();
                mount(
                    src_path.to_string(),
                    dsc_path.to_string(),
                    data.cloned(),
                    *mnt_flags,
                    fstype.cloned(),
                )
                .await
                .map_err(|error| CsiDriverError::Mount { source: error })
            }
            ("unmount", arg_matches) => {
                let target_path = arg_matches.get_one::<String>("target").unwrap();
                let unmnt_flags = arg_matches
                    .get_one::<sys_mount::UnmountFlags>("unmount-flags")
                    .unwrap();
                unmount(target_path.to_string(), *unmnt_flags)
                    .await
                    .map_err(|error| CsiDriverError::Mount { source: error })
            }
            _ => Err(CsiDriverError::InvalidCsiDriverCommand),
        }?;
        return Ok(());
    }

    // Print package info and command line arguments.
    utils::print_package_info!();
    println!("{:?}", env::args().collect::<Vec<String>>());

    // Initialize tracing telemetry.
    let tags = utils::tracing_telemetry::default_tracing_tags(
        utils::raw_version_str(),
        env!("CARGO_PKG_VERSION"),
    );

    let fmt_style = matches.get_one::<FmtStyle>("fmt-style").unwrap();
    let colors = matches.get_flag("ansi-colors");
    utils::tracing_telemetry::TracingTelemetry::builder()
        .with_writer(FmtLayer::Stdout)
        .with_style(*fmt_style)
        .with_colours(colors)
        .with_tracing_tags(tags.clone())
        .init("csi-node");

    // Validate presence of nvme_tcp kernel module and set nvme_core parameters.
    if let Err(error) = crate::dev::nvmf::check_nvme_tcp_module() {
        anyhow::bail!("Failed to detect nvme_tcp kernel module. Run `modprobe nvme_tcp` to load the kernel module. {}", error);
    }
    if let Some(nvme_io_timeout) = matches.get_one::<String>("nvme-io-timeout") {
        let _ = humantime::Duration::from_str(nvme_io_timeout)
            .map_err(|error| anyhow::format_err!("Failed to parse 'nvme-io-timeout': {error}"))?;
    };
    if let Some(nvme_io_timeout) = matches.get_one::<String>("nvme-core-io-timeout") {
        let io_timeout_secs = match humantime::Duration::from_str(nvme_io_timeout) {
            Ok(human_time) => {
                human_time.as_secs() as u32
            }
            Err(_) => {
                nvme_io_timeout.parse().expect(
                    "nvme_core io_timeout should be in humantime or an integer number, representing the timeout in seconds",
                )
            }
        };

        if let Err(error) = crate::dev::nvmf::set_nvmecore_iotimeout(io_timeout_secs) {
            anyhow::bail!("Failed to set nvme_core io_timeout: {}", error);
        }
    }

    // Check for nvme ana multipath support and generate labels.
    let mut csi_labels = HashMap::new();
    let nvme_enabled = utils::check_nvme_core_ana().unwrap_or_default().to_string();
    csi_labels.insert(utils::csi_node_nvme_ana(), nvme_enabled.clone());

    // If running in k8s, label the nodes with generated labels.
    let node_name = matches.get_one::<String>("node-name").unwrap();
    if platform::current_platform_type() == platform::PlatformType::K8s {
        let kube_client = kube::Client::try_default().await?;
        check_ana_and_label_node(&kube_client, node_name, nvme_enabled).await?;
    }

    let conn_fallback = matches.get_flag("nvme-connect-fallback");
    // Check and store if this initiator node has an RDMA device so that nvme connect over RDMA
    // can be done. If this node is also an io-engine node with rdma enabled and working,
    // then this check will always set capability true.
    let ibv_output = tokio::process::Command::new("ibv_devinfo")
        .arg("-l")
        .output()
        .await;

    let cap = match ibv_output {
        Ok(okstdout) if okstdout.status.success() => {
            // In an unlikely case string from utf8 fails, err towards incapability.
            let outstr = String::from_utf8(okstdout.stdout).unwrap_or("0 HCA".to_string());
            if !outstr.starts_with("0 HCA") {
                info!("host node rdma capable. conn_fallback({conn_fallback:?}), {outstr:?}");
                // todo: check for the presence of nvme_rdma too. But don't bail out as we do for
                // nvme_tcp.
                true
            } else {
                info!(
                    "host node is not rdma capable. conn_fallback({conn_fallback:?}), {outstr:?}"
                );
                false
            }
        }
        Ok(_) | Err(_) => {
            error!("Error executing ibv_devinfo, or invalid command output. conn_fallback({conn_fallback:?}), {ibv_output:?}");
            false
        }
    };
    let _ = RDMA_CONNECT_CHECK.set((conn_fallback, cap));

    // Parse the CSI socket file name from the command line arguments.
    let csi_socket = matches
        .get_one::<String>("csi-socket")
        .map(|s| s.as_str())
        .unwrap_or("/var/tmp/csi.sock");
    // Remove stale CSI socket from previous instance if there is any.
    match fs::remove_file(csi_socket) {
        Ok(_) => info!("Removed stale CSI socket {}", csi_socket),
        Err(err) => {
            if err.kind() != ErrorKind::NotFound {
                anyhow::bail!("Error removing stale CSI socket {}: {}", csi_socket, err);
            }
        }
    }

    // Initialize the rest api client.
    let client = AppNodesClientWrapper::initialize(matches.get_one::<String>("rest-endpoint"))?;

    let registration_enabled = matches.get_flag("enable-registration");

    // Parse instance and grpc endpoints from the command line arguments and validate.
    let grpc_sock_addr = validate_endpoints(&matches, registration_enabled)?;

    // Start the CSI server, node plugin grpc server and registration loop if registration is
    // enabled.
    *crate::config::config().nvme_as_mut() = TryFrom::try_from(&matches)?;
    let (csi, grpc, registration) = tokio::join!(
        CsiServer::run(csi_socket, &matches)?,
        NodePluginGrpcServer::run(grpc_sock_addr),
        run_registration_loop(
            node_name.clone(),
            grpc_sock_addr.to_string(),
            Some(csi_labels),
            &client,
            registration_enabled
        )
    );
    info!("Main process terminated!");
    vec![csi, grpc, registration].into_iter().collect()
}

struct CsiServer {}

impl CsiServer {
    fn run(
        csi_socket: &str,
        cli_args: &clap::ArgMatches,
    ) -> anyhow::Result<impl Future<Output = anyhow::Result<()>>> {
        let node_name = cli_args.get_one::<String>("node-name").expect("required");
        let node_selector = csi_driver::csi_node_selector_parse(
            cli_args
                .get_many::<String>("node-selector")
                .map(|s| s.map(|s| s.as_str())),
        )?;

        let incoming = {
            let uds = UnixListener::bind(csi_socket)?;
            info!("CSI plugin bound to {}", csi_socket);

            // Change permissions on CSI socket to allow non-privileged clients to access it
            // to simplify testing.
            if let Err(e) = fs::set_permissions(
                csi_socket,
                std::os::unix::fs::PermissionsExt::from_mode(0o777),
            ) {
                error!("Failed to change permissions for CSI socket: {:?}", e);
            } else {
                debug!("Successfully changed file permissions for CSI socket");
            }

            UnixListenerStream::new(uds)
        };

        let node = Node::new(node_name.into(), node_selector, probe_filesystems());
        Ok(async move {
            Server::builder()
                .add_service(NodeServer::new(node))
                .add_service(IdentityServer::new(Identity {}))
                .add_service(NvmeOperationsServer::new(NvmeOperationsSvc {}))
                .serve_with_incoming_shutdown(incoming, Shutdown::wait())
                .await
                .map_err(|error| {
                    use stor_port::transport_api::ErrorChain;
                    error!(error = error.full_string(), "CsiServer failed");
                    error.into()
                })
        })
    }
}

/// Gets the nvme ana multipath parameter and sets on the node as a label.
async fn check_ana_and_label_node(
    client: &kube::client::Client,
    node_name: &str,
    nvme_enabled: String,
) -> anyhow::Result<()> {
    let node_patch = json!({
        "apiVersion": "v1",
        "kind": "Node",
        "metadata": {
            "labels": {
                utils::csi_node_nvme_ana(): nvme_enabled,
            },
        },
    });
    patch_k8s_node(client, node_name, &node_patch).await?;
    Ok(())
}

/// Validate that the grpc endpoint is valid.
fn validate_endpoints(
    matches: &clap::ArgMatches,
    registration_enabled: bool,
) -> anyhow::Result<SocketAddr> {
    let grpc_endpoint = matches.get_one::<String>("grpc-endpoint");
    let grpc_ip = matches.get_one::<IpAddr>("grpc-ip");
    let grpc_port = matches.get_one::<u16>("grpc-port");
    let grpc_endpoint_socket_addr = match grpc_endpoint {
        None => SocketAddr::new(
            *grpc_ip.expect("grpc-ip must be provided if grpc-endpoint is missing"),
            *grpc_port.expect("grpc-port must be provided if grpc-endpoint is missing"),
        ),
        Some(grpc_endpoint) => {
            let grpc_endpoint = if grpc_endpoint.contains(':') {
                grpc_endpoint.to_string()
            } else {
                format!("{grpc_endpoint}:{GRPC_PORT}")
            };
            SocketAddr::from_str(&grpc_endpoint)?
        }
    };
    // Should not allow using an unspecified ip if registration is enabled as grpc endpoint gets
    // sent in registration request.
    if registration_enabled && grpc_endpoint_socket_addr.ip().is_unspecified() {
        return Err(anyhow::format_err!(
            "gRPC endpoint: `[::]`/`0.0.0.0` is not allowed if registration is enabled"
        ));
    }
    Ok(grpc_endpoint_socket_addr)
}
