use crate::{
    csi_node_nvme_client,
    detector::{NvmeController, NvmePathCache},
    path_provider::get_nvme_path_entry,
    Cli,
};
use agents::errors::{SvcError, SvcError::SubsystemNotFound};
use events_api::event::{EventAction, EventCategory, EventMessage, EventMeta, EventSource};
use grpc::{
    common::MapWrapper,
    context::Context,
    csi_node_nvme::NvmeConnectRequest,
    operations::ha_node::{
        server::NodeAgentServer,
        traits::{NodeAgentOperations, ReplacePathInfo},
    },
};
use stor_port::transport_api::{ErrorChain, ReplyError, ResourceKind};

use grpc::operations::ha_node::traits::GetControllerInfo;
use stor_port::{
    transport_api::v0::NvmeSubsystems as NvmeSubsys,
    types::v0::transport::NvmeSubsystem as NvmeCtrller,
};

use http::Uri;
use nvmeadm::{nvmf_discovery::TrType, nvmf_subsystem::Subsystem};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Instant};
use tokio::time::{sleep, Duration};
use utils::nvme_target_nqn_prefix;

/// Common error source name for all gRPC errors in HA Node agent.
const HA_AGENT_ERR_SOURCE: &str = "HA Node agent gRPC server";

/// High-level object that represents HA Node agent gRPC server.
pub(crate) struct NodeAgentApiServer {
    endpoint: SocketAddr,
    path_cache: NvmePathCache,
    path_connection_timeout: Duration,
    subsys_refresh_period: Duration,
}

impl NodeAgentApiServer {
    /// Returns a new `Self` with the given parameters.
    pub(crate) fn new(args: &Cli, path_cache: NvmePathCache) -> Self {
        Self {
            endpoint: args.grpc_endpoint(),
            path_cache,
            path_connection_timeout: *args.path_connection_timeout,
            subsys_refresh_period: *args.subsys_refresh_period,
        }
    }

    /// Runs this server as a future until a shutdown signal is received.
    pub(crate) async fn serve(&self) -> Result<(), agents::ServiceError> {
        let r = NodeAgentServer::new(Arc::new(NodeAgentSvc::new(
            self.path_cache.clone(),
            self.path_connection_timeout,
            self.subsys_refresh_period,
        )));
        tracing::info!(
            endpoint=?self.endpoint,
            path_connection_timeout=?self.path_connection_timeout,
            subsys_refresh_period=?self.subsys_refresh_period,
            "Starting gRPC server"
        );
        agents::Service::builder()
            .with_service(r.into_grpc_server())
            .run_err(self.endpoint)
            .await
    }
}

/// The gRPC server implementation for the HA Node agent.
struct NodeAgentSvc {
    path_cache: NvmePathCache,
    path_connection_timeout: Duration,
    subsys_refresh_period: Duration,
}

impl NodeAgentSvc {
    /// Returns a new `Self` with the given parameters.
    pub(crate) fn new(
        path_cache: NvmePathCache,
        path_connection_timeout: Duration,
        subsys_refresh_period: Duration,
    ) -> Self {
        Self {
            path_cache,
            path_connection_timeout,
            subsys_refresh_period,
        }
    }
}

fn is_same_controller(new_path_uri: &ParsedUri, subsystem: &Subsystem) -> Result<bool, SvcError> {
    let ctrlr_transport = subsystem
        .transport
        .parse()
        .map_err(|_| SvcError::InvalidArguments {})?;

    let same_transport = new_path_uri.transport().eq(&ctrlr_transport);
    let same_host_port = subsystem
        .address
        .match_host_port(new_path_uri.host(), &new_path_uri.port().to_string());

    Ok(same_transport && same_host_port)
}

/// Disconnect cached NVMe controller.
async fn disconnect_controller(
    ctrlr: &NvmeController,
    new_path: String,
) -> Result<Option<Subsystem>, SvcError> {
    let new_path_uri = parse_uri(new_path.as_str())?;
    let path = &ctrlr.path;
    match get_nvme_path_entry(path) {
        Some(pbuf) => {
            let subsystem = Subsystem::new(pbuf.path()).map_err(|_| SvcError::NotFound {
                kind: ResourceKind::NvmeSubsystem,
                id: path.to_owned(),
            })?;

            // sanity check to make sure this information is still up to date!
            if subsystem.nqn != new_path_uri.nqn {
                return Err(SvcError::UnexpectedSubsystemNqn {
                    nqn: subsystem.nqn,
                    expected_nqn: new_path_uri.nqn,
                    path: subsystem.name,
                });
            }

            // It is same controller only if transport also match. Otherwise we disconnect it.
            // This also takes care to disconnect old path if transport hasn't changed but host addr
            // changed, for example due to interface name modified in io-engine args.
            let same_controller = is_same_controller(&new_path_uri, &subsystem)?;
            if same_controller {
                tracing::info!(path, "Not disconnecting same NVMe controller. old subsys {subsystem:?}, new path {new_path_uri:?}");
                Ok(None)
            } else {
                tracing::info!(path, "Disconnecting NVMe controller");

                let path = path.to_owned();
                let subsystem_cln = subsystem.clone();
                // clarification: we're not disconnecting the subsystem, but rather the controller
                tokio::task::spawn_blocking(move || match subsystem_cln.disconnect() {
                    Ok(_) => {
                        tracing::info!(path, "Disconnected NVMe controller");
                    }
                    Err(error) => {
                        tracing::error!(path, %error, "Failed to disconnect NVMe controller");
                    }
                });

                Ok(Some(subsystem))
            }
        }
        None => {
            tracing::error!(path, "Failed to get system path for controller");

            Err(SvcError::NotFound {
                kind: ResourceKind::NvmePath,
                id: path.to_owned(),
            })
        }
    }
}

impl NodeAgentSvc {
    /// Connect to the NVMe controller.
    /// Waits until the controller is fully connected.
    async fn connect_controller(
        &self,
        new_path: String,
        nqn: String,
        publish_context: Option<HashMap<String, String>>,
    ) -> Result<(), SvcError> {
        let parsed_uri = parse_uri(new_path.as_str())?;
        // todo: this won't work for nqn prefix upgrades
        if !parsed_uri.nqn().starts_with(&nvme_target_nqn_prefix()) {
            return Err(SvcError::InvalidArguments {});
        }

        // Get the client to the nvme operations service running in csi-node.
        let mut client = csi_node_nvme_client().clone();

        tracing::info!(new_path, "Connecting to NVMe target");

        // Check to ensure Nqn is already connected
        if Subsystem::try_from_nqn(&nqn).is_err() {
            return Err(SvcError::ReplaceNqnNotFound { nqn });
        }

        // Check if the NVMe subsystem already exists to not
        // delete it in case of unsuccessful path replacement.
        let preexisted_subsystem = Subsystem::get(
            parsed_uri.host(),
            &parsed_uri.port(),
            parsed_uri.transport(),
            parsed_uri.nqn().as_str(),
        )
        .is_ok();

        // Open connection to the new target: ANA will automatically create
        // the second path and add it as an alternative for the first broken one,
        // which immediately resumes all stalled I/O
        let mut subsystem = match client
            .nvme_connect(NvmeConnectRequest {
                uri: new_path.clone(),
                publish_context: publish_context.map(|map| MapWrapper { map }),
            })
            .await
        {
            Ok(_) => match Subsystem::get(
                parsed_uri.host(),
                &parsed_uri.port(),
                parsed_uri.transport(),
                parsed_uri.nqn().as_str(),
            ) {
                Ok(subsystem) => {
                    tracing::info!(new_path, "Successfully connected to NVMe target");
                    Ok(subsystem)
                }
                Err(error) => Err(SubsystemNotFound {
                    nqn: nqn.clone(),
                    details: error.to_string(),
                }),
            },
            Err(error) => {
                tracing::error!(
                    new_path,
                    %error,
                    "Failed to connect to new NVMe target"
                );
                let details = format!(
                    "Failed to connect to new NVMe target: {error}, new path: {new_path}, nqn: {nqn}",
                    error = error.full_string(),
                );
                Err(SvcError::NvmeConnectError { details })
            }
        }?;

        let now = Instant::now();
        let timeout = self.path_connection_timeout;
        // Wait till new controller is fully connected before completing the call.
        // Straight after connection subsystem transitions into 'new' state, then
        // proceeds to 'connecting' till the connection is fully established,
        // so wait a bit before checking the state.
        loop {
            sleep(self.subsys_refresh_period).await;

            // Refresh subsystem to get the latest state.
            if let Err(error) = subsystem.sync() {
                tracing::error!(
                    new_path,
                    %error,
                    "Failed to synchronize NVMe subsystem state"
                );
                // Just log error and exit, since such a situation can take
                // place when the path is explicitly removed by user.
                break;
            }

            // TODO: consider max retries to prevent controller from
            //  not reaching 'live' state.
            match subsystem.state.as_str() {
                "connecting" | "new" => {
                    tracing::info!(
                        new_path,
                        state = "connecting",
                        "New NVMe path is not ready to serve I/O, waiting"
                    );
                }
                "live" => {
                    tracing::info!(new_path, "New NVMe path is ready to serve I/O");
                    self.event(EventAction::NvmePathFix, &nqn, event_meta(&nqn, &new_path))
                        .generate();
                    break;
                }
                _ => {
                    tracing::error!(
                        new_path,
                        state = subsystem.state.as_str(),
                        "New NVMe path is in incorrect state"
                    );
                    return Err(SvcError::Internal {
                        details: format!(
                            "New NVMe path is in incorrect state: {}",
                            subsystem.state.as_str()
                        ),
                    });
                }
            }

            // Check if path reconnection timeout has elapsed.
            if now.elapsed() >= timeout {
                tracing::error!(
                    new_path,
                    nqn,
                    "Timeout while connecting NVMe path, disconnecting NVMe subsystem"
                );

                let nvme_err = format!(
                    "Timeout while connecting to new NVMe target: new path: {new_path}, nqn: {nqn}"
                );

                // Delete the subsystem in case it is not pre-existed.
                // Re-read the subsystem to get the most recent data on raw device path.
                if !preexisted_subsystem {
                    let curr_subsystem = match Subsystem::get(
                        parsed_uri.host(),
                        &parsed_uri.port(),
                        parsed_uri.transport(),
                        parsed_uri.nqn().as_str(),
                    ) {
                        Ok(s) => s,
                        Err(error) => {
                            tracing::warn!(
                                new_path,
                                ?error,
                                "Can't lookup NVMe subsystem, skipping removal"
                            );
                            return Err(SvcError::NvmeConnectError { details: nvme_err });
                        }
                    };

                    tokio::task::block_in_place(move || {
                        // Remove the subsystem.
                        if let Err(error) = curr_subsystem.disconnect() {
                            tracing::error!(
                                ?curr_subsystem,
                                ?error,
                                "Failed to disconnect NVMe subsystem"
                            );
                        } else {
                            tracing::info!(
                                ?curr_subsystem,
                                "NVMe subsystem successfully disconnected"
                            );
                        }
                    });
                } else {
                    tracing::info!(?subsystem, "Keeping NVMe subsystem after connect timeout");
                }

                return Err(SvcError::NvmeConnectError { details: nvme_err });
            }
        }

        Ok(())
    }
}

#[tonic::async_trait]
impl NodeAgentOperations for NodeAgentSvc {
    async fn replace_path(
        &self,
        request: &dyn ReplacePathInfo,
        _context: Option<Context>,
    ) -> Result<(), ReplyError> {
        let deadline = std::time::Duration::from_secs(50);
        let start = std::time::Instant::now();
        tracing::info!(?request, "Replacing failed NVMe path");
        // Lookup NVMe controller whose path has failed.
        let ctrlrs = self
            .path_cache
            .lookup_controllers(request.target_nqn())
            .await
            .map_err(|_| {
                ReplyError::failed_precondition(
                    ResourceKind::NvmePath,
                    HA_AGENT_ERR_SOURCE.to_string(),
                    "Failed to lookup controller".to_string(),
                )
            })?;

        // Step 1: populate an additional healthy path to target NQN in addition to
        // existing failed path. Once this additional path is created, client I/O
        // automatically resumes.
        self.connect_controller(
            request.new_path(),
            request.target_nqn(),
            request.publish_context(),
        )
        .await?;

        // Step 2: disconnect broken path to leave the only new healthy path.
        // Note that errors under disconnection are not critical, since the second I/O
        // path has been successfully created, so having the first failed path in addition
        // to the second healthy one is OK: just display a warning and proceed as if
        // the call has completed successfully.
        let mut subsystems = Vec::with_capacity(ctrlrs.len());
        let mut disc_error = None;
        for ctrl in ctrlrs {
            match disconnect_controller(&ctrl, request.new_path()).await {
                Ok(subsystem) => subsystems.push(subsystem.map(|s| (ctrl, s))),
                Err(error) => {
                    tracing::warn!(
                        uri=%request.new_path(),
                        %error,
                        "Failed to disconnect failed path"
                    );
                    disc_error = Some(error);
                }
            }
        }
        if let Some(error) = disc_error {
            return Err(error.into());
        }
        for (ctrl, mut subsystem) in subsystems.into_iter().flatten() {
            loop {
                if subsystem.sync().is_err() {
                    tracing::info!(path = ctrl.path, "Path has been removed");
                    break;
                }
                tracing::info!(path = ctrl.path, state = %subsystem.state, "Path state sync");
                if subsystem.state.starts_with("deleting") {
                    break;
                }

                if start.elapsed() >= deadline {
                    return Err(ReplyError::deadline_exceeded(
                        ResourceKind::NvmePath,
                        HA_AGENT_ERR_SOURCE.to_string(),
                        "Failed to wait for previous path to transition to deleted/deleting"
                            .to_string(),
                    ));
                }
                sleep(self.subsys_refresh_period).await;
            }
        }

        Ok(())
    }

    async fn get_nvme_controller(
        &self,
        request: &dyn GetControllerInfo,
        _context: Option<Context>,
    ) -> Result<NvmeSubsys, ReplyError> {
        let uri = request
            .nvme_path()
            .parse::<Uri>()
            .map_err(|_| SvcError::InvalidArguments {})?;
        let nqn = uri.path()[1..].to_string();
        match Subsystem::try_from_nqn(nqn.as_str()) {
            Ok(subsys_list) => {
                let controller_list = subsys_list
                    .into_iter()
                    .map(|controller| NvmeCtrller::new(controller.address.to_raw()))
                    .collect();
                Ok(NvmeSubsys(controller_list))
            }
            Err(_) => Err(ReplyError::not_found(
                ResourceKind::NvmeSubsystem,
                "Node agent".to_string(),
                "Could not find any subsystems for the supplied nqn".to_string(),
            )),
        }
    }
}

// Returns the host, port, nqn respectively from the path after parsing.
fn parse_uri(new_path: &str) -> Result<ParsedUri, SvcError> {
    let uri = new_path
        .parse::<Uri>()
        .map_err(|_| SvcError::InvalidArguments {})?;

    ParsedUri::new(uri)
}

struct ParsedUri {
    host: String,
    port: u16,
    transport: TrType,
    nqn: String,
}

impl std::fmt::Debug for ParsedUri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParsedUri")
            .field("host", &self.host())
            .field("port", &self.port())
            .field("transport", &self.transport())
            .field("nqn", &self.nqn())
            .finish()
    }
}
impl ParsedUri {
    fn new(uri: Uri) -> Result<ParsedUri, SvcError> {
        let host = uri.host().ok_or(SvcError::InvalidArguments {})?.to_string();
        let port = uri.port().ok_or(SvcError::InvalidArguments {})?.as_u16();
        // split returning a None indicates we possibly received old style uri scheme nvmf:// for
        // new path, which implies transport as tcp.
        let trstring = uri
            .scheme_str()
            .ok_or(SvcError::InvalidArguments {})?
            .split('+')
            .nth(1);

        let transport = if let Some(t) = trstring {
            t.parse().map_err(|_| SvcError::InvalidArguments {})?
        } else {
            TrType::default()
        };

        let nqn = uri.path()[1..].to_string();

        Ok(Self {
            host,
            port,
            transport,
            nqn,
        })
    }
    fn host(&self) -> &str {
        &self.host
    }
    fn port(&self) -> u16 {
        self.port
    }
    fn transport(&self) -> TrType {
        self.transport
    }
    fn nqn(&self) -> String {
        self.nqn.clone()
    }
}

/// Event trait definition for creating events and adding meta data for NvmePath events.
trait EventWithMeta {
    /// Create event message with meta data.
    fn event(&self, action: EventAction, nqn: &str, meta: EventMeta) -> EventMessage;
}

impl EventWithMeta for NodeAgentSvc {
    fn event(&self, event_action: EventAction, nqn: &str, meta: EventMeta) -> EventMessage {
        let volume_id = nqn.split(':').last().unwrap_or_default().to_string();
        EventMessage {
            category: EventCategory::NvmePath as i32,
            action: event_action as i32,
            target: volume_id,
            metadata: Some(meta),
        }
    }
}

// Get event meta data for nvme path connect event
fn event_meta(nqn: &str, path: &str) -> EventMeta {
    let event_source = EventSource::new("".to_string()).with_nvme_path_data(nqn, path);

    EventMeta::from_source(event_source)
}
