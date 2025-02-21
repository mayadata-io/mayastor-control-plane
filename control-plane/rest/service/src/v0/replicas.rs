use super::*;
use crate::v0::pools::pool;
use grpc::operations::{pool::traits::PoolOperations, replica::traits::ReplicaOperations};
use std::convert::{TryFrom, TryInto};
use stor_port::{
    transport_api::{ReplyError, ReplyErrorKind, ResourceKind},
    types::v0::openapi::apis::Uuid,
};

fn pool_client() -> impl PoolOperations {
    core_grpc().pool()
}

fn replica_client() -> impl ReplicaOperations {
    core_grpc().replica()
}

async fn put_replica(
    filter: Filter,
    body: CreateReplicaBody,
) -> Result<models::Replica, RestError<RestJsonError>> {
    let create = match filter.clone() {
        Filter::NodePoolReplica(node_id, pool_id, replica_id) => {
            body.to_request(node_id, pool_id, replica_id)
        }
        Filter::PoolReplica(pool_id, replica_id) => {
            let node_id = match pool_client().get(Filter::Pool(pool_id.clone()), None).await {
                Ok(pools) => pool(pool_id.to_string(), pools.into_inner().first())?.node(),
                Err(error) => return Err(RestError::from(error)),
            };
            body.to_request(node_id, pool_id, replica_id)
        }
        _ => {
            return Err(RestError::from(ReplyError {
                kind: ReplyErrorKind::Internal,
                resource: ResourceKind::Replica,
                source: "put_replica".to_string(),
                extra: "invalid filter for resource".to_string(),
            }))
        }
    };
    let replica = replica_client().create(&create, None).await?;
    Ok(replica.into())
}

async fn destroy_replica(filter: Filter) -> Result<(), RestError<RestJsonError>> {
    let destroy = match filter.clone() {
        Filter::NodePoolReplica(node_id, pool_id, replica_id) => DestroyReplica {
            node: node_id,
            pool_id,
            pool_uuid: None,
            name: None,
            uuid: replica_id,
            ..Default::default()
        },
        Filter::PoolReplica(pool_id, replica_id) => {
            let node_id = match replica_client().get(filter, None).await {
                Ok(replicas) => {
                    replica(replica_id.to_string(), replicas.into_inner().first())?.node
                }
                Err(error) => return Err(RestError::from(error)),
            };

            DestroyReplica {
                node: node_id,
                pool_id,
                pool_uuid: None,
                name: None,
                uuid: replica_id,
                ..Default::default()
            }
        }
        _ => {
            return Err(RestError::from(ReplyError {
                kind: ReplyErrorKind::Internal,
                resource: ResourceKind::Replica,
                source: "destroy_replica".to_string(),
                extra: "invalid filter for resource".to_string(),
            }))
        }
    };
    replica_client().destroy(&destroy, None).await?;
    Ok(())
}

async fn share_replica(
    filter: Filter,
    protocol: ReplicaShareProtocol,
    allowed_hosts: Option<Vec<String>>,
) -> Result<String, RestError<RestJsonError>> {
    let conv_hosts = |h: Option<Vec<String>>| {
        h.unwrap_or_default()
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<_, _>>()
            .map_err(ReplyError::from)
    };
    let share = match filter.clone() {
        Filter::NodePoolReplica(node_id, pool_id, replica_id) => ShareReplica {
            node: node_id,
            pool_id,
            pool_uuid: None,
            name: None,
            uuid: replica_id,
            protocol,
            allowed_hosts: conv_hosts(allowed_hosts)?,
        },
        Filter::PoolReplica(pool_id, replica_id) => {
            let node_id = match replica_client().get(filter, None).await {
                Ok(replicas) => {
                    replica(replica_id.to_string(), replicas.into_inner().first())?.node
                }
                Err(error) => return Err(RestError::from(error)),
            };

            ShareReplica {
                node: node_id,
                pool_id,
                pool_uuid: None,
                name: None,
                uuid: replica_id,
                protocol,
                allowed_hosts: conv_hosts(allowed_hosts)?,
            }
        }
        _ => {
            return Err(RestError::from(ReplyError {
                kind: ReplyErrorKind::Internal,
                resource: ResourceKind::Replica,
                source: "share_replica".to_string(),
                extra: "invalid filter for resource".to_string(),
            }))
        }
    };
    let share_uri = replica_client().share(&share, None).await?;
    Ok(share_uri)
}

async fn unshare_replica(filter: Filter) -> Result<(), RestError<RestJsonError>> {
    let unshare = match filter.clone() {
        Filter::NodePoolReplica(node_id, pool_id, replica_id) => UnshareReplica {
            node: node_id,
            pool_id,
            pool_uuid: None,
            name: None,
            uuid: replica_id,
        },
        Filter::PoolReplica(pool_id, replica_id) => {
            let node_id = match replica_client().get(filter, None).await {
                Ok(replicas) => {
                    replica(replica_id.to_string(), replicas.into_inner().first())?.node
                }
                Err(error) => return Err(RestError::from(error)),
            };

            UnshareReplica {
                node: node_id,
                pool_id,
                pool_uuid: None,
                name: None,
                uuid: replica_id,
            }
        }
        _ => {
            return Err(RestError::from(ReplyError {
                kind: ReplyErrorKind::Internal,
                resource: ResourceKind::Replica,
                source: "unshare_replica".to_string(),
                extra: "invalid filter for resource".to_string(),
            }))
        }
    };
    replica_client().unshare(&unshare, None).await?;
    Ok(())
}

#[async_trait::async_trait]
impl apis::actix_server::Replicas for RestApi {
    async fn del_node_pool_replica(
        Path((node_id, pool_id, replica_id)): Path<(String, String, Uuid)>,
    ) -> Result<(), RestError<RestJsonError>> {
        destroy_replica(Filter::NodePoolReplica(
            node_id.into(),
            pool_id.into(),
            replica_id.into(),
        ))
        .await
    }

    async fn del_node_pool_replica_share(
        Path((node_id, pool_id, replica_id)): Path<(String, String, Uuid)>,
    ) -> Result<(), RestError<RestJsonError>> {
        unshare_replica(Filter::NodePoolReplica(
            node_id.into(),
            pool_id.into(),
            replica_id.into(),
        ))
        .await
    }

    async fn del_pool_replica(
        Path((pool_id, replica_id)): Path<(String, Uuid)>,
    ) -> Result<(), RestError<RestJsonError>> {
        destroy_replica(Filter::PoolReplica(pool_id.into(), replica_id.into())).await
    }

    async fn del_pool_replica_share(
        Path((pool_id, replica_id)): Path<(String, Uuid)>,
    ) -> Result<(), RestError<RestJsonError>> {
        unshare_replica(Filter::PoolReplica(pool_id.into(), replica_id.into())).await
    }

    async fn get_node_pool_replica(
        Path((node_id, pool_id, replica_id)): Path<(String, String, Uuid)>,
    ) -> Result<models::Replica, RestError<RestJsonError>> {
        let replica = replica(
            replica_id.to_string(),
            replica_client()
                .get(
                    Filter::NodePoolReplica(node_id.into(), pool_id.into(), replica_id.into()),
                    None,
                )
                .await?
                .into_inner()
                .first(),
        )?;
        Ok(replica.into())
    }

    async fn get_node_pool_replicas(
        Path((node_id, pool_id)): Path<(String, String)>,
    ) -> Result<Vec<models::Replica>, RestError<RestJsonError>> {
        let replicas = replica_client()
            .get(Filter::NodePool(node_id.into(), pool_id.into()), None)
            .await?;
        Ok(replicas.into_inner().into_iter().map(From::from).collect())
    }

    async fn get_node_replicas(
        Path(id): Path<String>,
    ) -> Result<Vec<models::Replica>, RestError<RestJsonError>> {
        let replicas = replica_client().get(Filter::Node(id.into()), None).await?;
        Ok(replicas.into_inner().into_iter().map(From::from).collect())
    }

    async fn get_replica(
        Path(id): Path<Uuid>,
    ) -> Result<models::Replica, RestError<RestJsonError>> {
        let replica = replica(
            id.to_string(),
            replica_client()
                .get(Filter::Replica(id.into()), None)
                .await?
                .into_inner()
                .first(),
        )?;
        Ok(replica.into())
    }

    async fn get_replicas() -> Result<Vec<models::Replica>, RestError<RestJsonError>> {
        let replicas = replica_client().get(Filter::None, None).await?;
        Ok(replicas.into_inner().into_iter().map(From::from).collect())
    }

    async fn put_node_pool_replica(
        Path((node_id, pool_id, replica_id)): Path<(String, String, Uuid)>,
        Body(create_replica_body): Body<models::CreateReplicaBody>,
    ) -> Result<models::Replica, RestError<RestJsonError>> {
        put_replica(
            Filter::NodePoolReplica(node_id.into(), pool_id.into(), replica_id.into()),
            CreateReplicaBody::try_from(create_replica_body)?,
        )
        .await
    }

    async fn put_node_pool_replica_share(
        Path((node_id, pool_id, replica_id)): Path<(String, String, Uuid)>,
        Query(allowed_hosts): Query<Option<Vec<String>>>,
    ) -> Result<String, RestError<RestJsonError>> {
        share_replica(
            Filter::NodePoolReplica(node_id.into(), pool_id.into(), replica_id.into()),
            ReplicaShareProtocol::Nvmf,
            allowed_hosts,
        )
        .await
    }

    async fn put_pool_replica(
        Path((pool_id, replica_id)): Path<(String, Uuid)>,
        Body(create_replica_body): Body<models::CreateReplicaBody>,
    ) -> Result<models::Replica, RestError<RestJsonError>> {
        put_replica(
            Filter::PoolReplica(pool_id.into(), replica_id.into()),
            CreateReplicaBody::try_from(create_replica_body)?,
        )
        .await
    }

    async fn put_pool_replica_share(
        Path((pool_id, replica_id)): Path<(String, Uuid)>,
        Query(allowed_hosts): Query<Option<Vec<String>>>,
    ) -> Result<String, RestError<RestJsonError>> {
        share_replica(
            Filter::PoolReplica(pool_id.into(), replica_id.into()),
            ReplicaShareProtocol::Nvmf,
            allowed_hosts,
        )
        .await
    }
}

/// returns replica from replica option and returns an error on non existence
fn replica(replica_id: String, replica: Option<&Replica>) -> Result<Replica, ReplyError> {
    match replica {
        Some(replica) => Ok(replica.clone()),
        None => Err(ReplyError {
            kind: ReplyErrorKind::NotFound,
            resource: ResourceKind::Replica,
            source: "Requested replica was not found".to_string(),
            extra: format!("Replica id : {replica_id}"),
        }),
    }
}
