use crate::CsiControllerConfig;
use std::collections::HashMap;
use stor_port::types::v0::openapi::{
    clients,
    clients::tower::StatusCode,
    models,
    models::{
        AffinityGroup, CreateVolumeBody, Node, NodeTopology, Pool, PoolTopology, PublishVolumeBody,
        RestJsonError, Topology, Volume, VolumePolicy, VolumeShareProtocol, Volumes,
    },
};

use anyhow::{anyhow, Result};
use once_cell::sync::OnceCell;
use tracing::{debug, info, instrument};

#[derive(Debug, PartialEq, Eq)]
pub enum ApiClientError {
    // Error while communicating with the server.
    ServerCommunication(String),
    // Requested resource already exists. This error has a dedicated variant
    // in order to handle resource idempotency properly.
    ResourceAlreadyExists(String),
    // No resource instance exists.
    ResourceNotExists(String),
    NotImplemented(String),
    RequestTimeout(String),
    Aborted(String),
    Conflict(String),
    ResourceExhausted(String),
    // Generic operation errors.
    GenericOperation(StatusCode, String),
    // Problems with parsing response body.
    InvalidResponse(String),
    /// URL is malformed.
    MalformedUrl(String),
    /// Invalid argument.
    InvalidArgument(String),
    /// Unavailable.
    Unavailable(String),
    /// Precondition Failed.
    PreconditionFailed(String),
}

/// Placeholder for volume topology for volume creation operation.
#[derive(Debug)]
pub struct CreateVolumeTopology {
    node_topology: Option<NodeTopology>,
    pool_topology: Option<PoolTopology>,
}

impl CreateVolumeTopology {
    pub fn new(node_topology: Option<NodeTopology>, pool_topology: Option<PoolTopology>) -> Self {
        Self {
            node_topology,
            pool_topology,
        }
    }
}

impl From<clients::tower::Error<RestJsonError>> for ApiClientError {
    fn from(error: clients::tower::Error<RestJsonError>) -> Self {
        match error {
            clients::tower::Error::Request(request) => {
                Self::ServerCommunication(request.to_string())
            }
            clients::tower::Error::Response(response) => match response {
                clients::tower::ResponseError::Expected(_) => {
                    // TODO: Revisit status codes checks after improving REST API HTTP codes
                    // (CAS-1124).
                    let detailed = response.to_string();
                    match response.status() {
                        StatusCode::NOT_FOUND => Self::ResourceNotExists(detailed),
                        StatusCode::UNPROCESSABLE_ENTITY => Self::ResourceAlreadyExists(detailed),
                        StatusCode::NOT_IMPLEMENTED => Self::NotImplemented(detailed),
                        StatusCode::REQUEST_TIMEOUT => Self::RequestTimeout(detailed),
                        StatusCode::CONFLICT => Self::Conflict(detailed),
                        StatusCode::INSUFFICIENT_STORAGE => Self::ResourceExhausted(detailed),
                        StatusCode::SERVICE_UNAVAILABLE => Self::Unavailable(detailed),
                        StatusCode::PRECONDITION_FAILED => Self::PreconditionFailed(detailed),
                        status => Self::GenericOperation(status, detailed),
                    }
                }
                clients::tower::ResponseError::PayloadError { .. } => {
                    Self::InvalidResponse(response.to_string())
                }
                clients::tower::ResponseError::Unexpected(_) => {
                    Self::InvalidResponse(response.to_string())
                }
            },
        }
    }
}

static REST_CLIENT: OnceCell<IoEngineApiClient> = OnceCell::new();

/// Single instance API client for accessing REST API gateway.
/// Encapsulates communication with REST API by exposing a set of
/// high-level API functions, which perform (de)serialization
/// of API request/response objects.
#[derive(Debug)]
pub struct IoEngineApiClient {
    rest_client: clients::tower::ApiClient,
}

impl IoEngineApiClient {
    /// Initialize API client instance. Must be called prior to
    /// obtaining the client instance.
    pub(crate) fn initialize() -> Result<()> {
        if REST_CLIENT.get().is_some() {
            return Err(anyhow!("API client already initialized"));
        }

        let cfg = CsiControllerConfig::get_config();
        let endpoint = cfg.rest_endpoint();

        let url = clients::tower::Url::parse(endpoint)
            .map_err(|error| anyhow!("Invalid API endpoint URL {}: {:?}", endpoint, error))?;
        let concurrency_limit = cfg.create_volume_limit() * 2;
        let tower = clients::tower::Configuration::builder()
            .with_timeout(cfg.io_timeout())
            .with_concurrency_limit(Some(concurrency_limit))
            .build_url(url)
            .map_err(|error| {
                anyhow::anyhow!(
                    "Failed to create openapi configuration, Error: '{:?}'",
                    error
                )
            })?;

        REST_CLIENT.get_or_init(|| Self {
            rest_client: clients::tower::ApiClient::new(tower.clone()),
        });

        info!(
            "API client is initialized with endpoint {}, I/O timeout = {:?}",
            endpoint,
            cfg.io_timeout(),
        );
        Ok(())
    }

    /// Obtain client instance. Panics if called before the client
    /// has been initialized.
    pub(crate) fn get_client() -> &'static IoEngineApiClient {
        REST_CLIENT.get().expect("Rest client is not initialized")
    }
}

impl IoEngineApiClient {
    /// List all nodes available in IoEngine cluster.
    pub(crate) async fn list_nodes(&self) -> Result<Vec<Node>, ApiClientError> {
        let response = self.rest_client.nodes_api().get_nodes(None).await?;
        Ok(response.into_body())
    }

    /// Get a particular node available in IoEngine cluster.
    pub(crate) async fn get_node(&self, node_id: &str) -> Result<Node, ApiClientError> {
        let response = self
            .rest_client
            .nodes_api()
            .get_nodes(Some(node_id))
            .await?;
        match response.into_body().pop() {
            Some(node) => Ok(node),
            None => Err(ApiClientError::ResourceNotExists("Node not found".into())),
        }
    }

    /// List all pools available in IoEngine cluster.
    pub(crate) async fn list_pools(&self) -> Result<Vec<Pool>, ApiClientError> {
        let response = self.rest_client.pools_api().get_pools().await?;
        Ok(response.into_body())
    }

    /// List all volumes available in IoEngine cluster.
    pub(crate) async fn list_volumes(
        &self,
        max_entries: i32,
        starting_token: String,
    ) -> Result<Volumes, ApiClientError> {
        let max_entries = max_entries as isize;
        let starting_token = if starting_token.is_empty() {
            0
        } else {
            starting_token.parse::<isize>().map_err(|_| {
                ApiClientError::InvalidArgument(
                    "Failed to parse starting token as an isize".to_string(),
                )
            })?
        };

        let response = self
            .rest_client
            .volumes_api()
            .get_volumes(max_entries, None, Some(starting_token))
            .await?;
        Ok(response.into_body())
    }

    /// List pools available on target IoEngine node.
    pub(crate) async fn get_node_pools(&self, node: &str) -> Result<Vec<Pool>, ApiClientError> {
        let pools = self.rest_client.pools_api().get_node_pools(node).await?;
        Ok(pools.into_body())
    }

    /// Create a volume of target size and provision storage resources for it.
    /// This operation is not idempotent, so the caller is responsible for taking
    /// all actions with regards to idempotency.
    #[instrument(fields(volume.uuid = %volume_id), skip(self, volume_id))]
    pub(crate) async fn create_volume(
        &self,
        volume_id: &uuid::Uuid,
        replicas: u8,
        size: u64,
        volume_topology: CreateVolumeTopology,
        thin: bool,
        affinity_group: Option<AffinityGroup>,
    ) -> Result<Volume, ApiClientError> {
        let topology =
            Topology::new_all(volume_topology.node_topology, volume_topology.pool_topology);

        let req = CreateVolumeBody {
            replicas,
            size,
            thin,
            topology: Some(topology),
            policy: VolumePolicy::new_all(true),
            labels: None,
            affinity_group,
        };

        let result = self
            .rest_client
            .volumes_api()
            .put_volume(volume_id, req)
            .await?;
        Ok(result.into_body())
    }

    /// Delete volume and reclaim all storage resources associated with it.
    /// This operation is idempotent, so the caller does not see errors indicating
    /// absence of the resource.
    #[instrument(fields(volume.uuid = %volume_id), skip(self, volume_id))]
    pub(crate) async fn delete_volume(&self, volume_id: &uuid::Uuid) -> Result<(), ApiClientError> {
        Self::delete_idempotent(
            self.rest_client.volumes_api().del_volume(volume_id).await,
            true,
        )?;
        debug!(volume.uuid=%volume_id, "Volume successfully deleted");
        Ok(())
    }

    /// Check HTTP status code, handle DELETE idempotency transparently.
    pub(crate) fn delete_idempotent<T>(
        result: Result<clients::tower::ResponseContent<T>, clients::tower::Error<RestJsonError>>,
        idempotent: bool,
    ) -> Result<(), ApiClientError> {
        match result {
            Ok(_) => Ok(()),
            Err(clients::tower::Error::Request(error)) => {
                Err(clients::tower::Error::Request(error).into())
            }
            Err(clients::tower::Error::Response(response)) => match response.status() {
                // Handle idempotency as requested by the caller.
                StatusCode::NOT_FOUND
                | StatusCode::NO_CONTENT
                | StatusCode::PRECONDITION_FAILED => {
                    if idempotent {
                        Ok(())
                    } else {
                        Err(clients::tower::Error::Response(response).into())
                    }
                }
                _ => Err(clients::tower::Error::Response(response).into()),
            },
        }
    }

    /// Get specific volume.
    #[instrument(fields(volume.uuid = %volume_id), skip(self, volume_id))]
    pub(crate) async fn get_volume(
        &self,
        volume_id: &uuid::Uuid,
    ) -> Result<Volume, ApiClientError> {
        let volume = self.rest_client.volumes_api().get_volume(volume_id).await?;
        Ok(volume.into_body())
    }

    /// Get specific volume.
    #[instrument(fields(volume.uuid = %volume_id), skip(self, volume_id))]
    pub(crate) async fn get_volume_for_create(
        &self,
        volume_id: &uuid::Uuid,
    ) -> Result<Volume, ApiClientError> {
        let response = self
            .rest_client
            .volumes_api()
            .get_volumes(1, Some(volume_id), None)
            .await?;
        let mut entries = response.into_body().entries;
        match entries.pop() {
            Some(volume) => Ok(volume),
            None => Err(ApiClientError::ResourceNotExists("Volume Not Found".into())),
        }
    }

    /// Unpublish volume (i.e. destroy a target which exposes the volume).
    #[instrument(fields(volume.uuid = %volume_id), skip(self, volume_id))]
    pub(crate) async fn unpublish_volume(
        &self,
        volume_id: &uuid::Uuid,
        force: bool,
    ) -> Result<(), ApiClientError> {
        Self::delete_idempotent(
            self.rest_client
                .volumes_api()
                .del_volume_target(volume_id, Some(force))
                .await,
            true,
        )?;
        debug!(volume.uuid=%volume_id, "Volume target successfully deleted");
        Ok(())
    }

    /// Publish volume (i.e. make it accessible via specified protocol by creating a target).
    #[instrument(fields(volume.uuid = %volume_id), skip(self, volume_id))]
    pub(crate) async fn publish_volume(
        &self,
        volume_id: &uuid::Uuid,
        node: Option<&str>,
        protocol: VolumeShareProtocol,
        frontend_node: String,
        publish_context: &HashMap<String, String>,
    ) -> Result<Volume, ApiClientError> {
        let publish_volume_body = PublishVolumeBody::new_all(
            publish_context.clone(),
            None,
            node.map(|node| node.to_string()),
            protocol,
            None,
            frontend_node,
        );
        let volume = self
            .rest_client
            .volumes_api()
            .put_volume_target(volume_id, publish_volume_body)
            .await?;
        Ok(volume.into_body())
    }

    /// Create a volume snapshot.
    #[instrument(fields(volume.uuid = %volume_id, snapshot.source_uuid = %volume_id, snapshot.uuid = %snapshot_id), skip(self, volume_id, snapshot_id))]
    pub(crate) async fn create_volume_snapshot(
        &self,
        volume_id: &uuid::Uuid,
        snapshot_id: &uuid::Uuid,
    ) -> Result<models::VolumeSnapshot, ApiClientError> {
        let snapshot = self
            .rest_client
            .snapshots_api()
            .put_volume_snapshot(volume_id, snapshot_id)
            .await?;

        Ok(snapshot.into_body())
    }

    /// Delete a volume snapshot.
    #[instrument(fields(snapshot.uuid = %snapshot_id), skip(self, snapshot_id))]
    pub(crate) async fn delete_volume_snapshot(
        &self,
        snapshot_id: &uuid::Uuid,
    ) -> Result<(), ApiClientError> {
        Self::delete_idempotent(
            self.rest_client
                .snapshots_api()
                .del_snapshot(snapshot_id)
                .await,
            true,
        )?;
        debug!(snapshot.uuid=%snapshot_id, "Volume Snapshot successfully deleted");
        Ok(())
    }

    /// List volume snapshots.
    #[instrument(fields(snapshot.source_uuid = ?volume_id, snapshot.uuid = ?snapshot_id), skip(self, volume_id, snapshot_id))]
    pub(crate) async fn list_volume_snapshots(
        &self,
        volume_id: Option<uuid::Uuid>,
        snapshot_id: Option<uuid::Uuid>,
        max_entries: i32,
        starting_token: String,
    ) -> Result<models::VolumeSnapshots, ApiClientError> {
        let max_entries = max_entries as isize;
        let starting_token = if starting_token.is_empty() {
            0
        } else {
            starting_token.parse::<isize>().map_err(|_| {
                ApiClientError::InvalidArgument(
                    "Failed to parse starting token as an isize".to_string(),
                )
            })?
        };

        let snapshots = self
            .rest_client
            .snapshots_api()
            .get_volumes_snapshots(
                max_entries,
                snapshot_id.as_ref(),
                volume_id.as_ref(),
                Some(starting_token),
            )
            .await?;

        let next_token = snapshots.body().next_token;
        // Don't return snapshots that are still in Creating state.
        Ok(models::VolumeSnapshots {
            entries: snapshots
                .into_body()
                .entries
                .into_iter()
                .filter(|s| s.definition.metadata.status != models::SpecStatus::Creating)
                .collect(),
            next_token,
        })
    }

    /// Get volume snapshot.
    #[instrument(fields(snapshot.uuid = ?snapshot_id), skip(self, snapshot_id))]
    pub(crate) async fn get_volumes_snapshot(
        &self,
        snapshot_id: &uuid::Uuid,
    ) -> Result<models::VolumeSnapshot, ApiClientError> {
        let snapshot = self
            .rest_client
            .snapshots_api()
            .get_volumes_snapshot(snapshot_id)
            .await?;

        Ok(snapshot.into_body())
    }
}
