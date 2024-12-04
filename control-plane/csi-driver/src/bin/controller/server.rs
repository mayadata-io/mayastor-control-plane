use crate::{controller::CsiControllerSvc, identity::CsiIdentitySvc};
use rpc::csi::{controller_server::ControllerServer, identity_server::IdentityServer};

use std::{fs, io::ErrorKind, ops::Add};
use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;
use tracing::{debug, error, info};

pub(super) struct CsiServer {}
impl CsiServer {
    /// Runs the CSI Server identity and controller services.
    pub async fn run(csi_socket: &str) -> anyhow::Result<()> {
        // It seems we're not ensuring only 1 csi server is running at a time because here
        // we don't bind to a port for example but to a unix socket.
        // todo: Can we do something about this?

        // Remove existing CSI socket from previous runs.
        match fs::remove_file(csi_socket) {
            Ok(_) => info!("Removed stale CSI socket {}", csi_socket),
            Err(err) => {
                if err.kind() != ErrorKind::NotFound {
                    return Err(anyhow::anyhow!(
                        "Error removing stale CSI socket {csi_socket}: {err}"
                    ));
                }
            }
        }

        let incoming = {
            let uds = UnixListener::bind(csi_socket)?;

            info!("CSI RPC server is listening on {csi_socket}");

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

        let cfg = crate::CsiControllerConfig::get_config();

        Server::builder()
            .timeout(cfg.io_timeout().add(std::time::Duration::from_secs(3)))
            .add_service(IdentityServer::new(CsiIdentitySvc::default()))
            .add_service(ControllerServer::new(CsiControllerSvc::new(cfg)))
            .serve_with_incoming_shutdown(incoming, shutdown::Shutdown::wait())
            .await
            .inspect_err(|error| {
                use stor_port::transport_api::ErrorChain;
                error!(error = error.full_string(), "NodePluginGrpcServer failed");
            })?;
        Ok(())
    }
}
