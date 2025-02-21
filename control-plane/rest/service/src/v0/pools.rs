use super::*;
use grpc::operations::pool::traits::PoolOperations;
use rest_client::versions::v0::apis::Uuid;
use std::collections::HashMap;
use stor_port::{
    transport_api::{ReplyError, ReplyErrorKind, ResourceKind},
    types::v0::transport::{DestroyPool, Filter, UnlabelPool},
};

fn client() -> impl PoolOperations {
    core_grpc().pool()
}

async fn destroy_pool(filter: Filter) -> Result<(), RestError<RestJsonError>> {
    let destroy = match filter.clone() {
        Filter::NodePool(node_id, pool_id) => DestroyPool {
            node: node_id,
            id: pool_id,
        },
        Filter::Pool(pool_id) => {
            let node_id = match client().get(filter, None).await {
                Ok(pools) => pool(pool_id.to_string(), pools.into_inner().first())?.node(),
                Err(error) => return Err(RestError::from(error)),
            };
            DestroyPool {
                node: node_id,
                id: pool_id,
            }
        }
        _ => {
            return Err(RestError::from(ReplyError {
                kind: ReplyErrorKind::Internal,
                resource: ResourceKind::Pool,
                source: "destroy_pool".to_string(),
                extra: "invalid filter for resource".to_string(),
            }))
        }
    };
    client().destroy(&destroy, None).await?;
    Ok(())
}

#[async_trait::async_trait]
impl apis::actix_server::Pools for RestApi {
    async fn del_node_pool(
        Path((node_id, pool_id)): Path<(String, String)>,
    ) -> Result<(), RestError<RestJsonError>> {
        destroy_pool(Filter::NodePool(node_id.into(), pool_id.into())).await
    }

    async fn del_pool(Path(pool_id): Path<String>) -> Result<(), RestError<RestJsonError>> {
        destroy_pool(Filter::Pool(pool_id.into())).await
    }

    async fn get_node_pool(
        Path((node_id, pool_id)): Path<(String, String)>,
    ) -> Result<models::Pool, RestError<RestJsonError>> {
        let pool = pool(
            pool_id.clone(),
            client()
                .get(Filter::NodePool(node_id.into(), pool_id.into()), None)
                .await?
                .into_inner()
                .first(),
        )?;
        Ok(pool.into())
    }

    async fn get_node_pools(
        Path(id): Path<String>,
    ) -> Result<Vec<models::Pool>, RestError<RestJsonError>> {
        let pools = client().get(Filter::Node(id.into()), None).await?;
        Ok(pools.into_inner().into_iter().map(From::from).collect())
    }

    async fn get_pool(
        Path(pool_id): Path<String>,
    ) -> Result<models::Pool, RestError<RestJsonError>> {
        let pool = pool(
            pool_id.clone(),
            client()
                .get(Filter::Pool(pool_id.clone().into()), None)
                .await?
                .into_inner()
                .first(),
        )?;
        Ok(pool.into())
    }

    async fn get_pools(
        Query(volume_id): Query<Option<Uuid>>,
    ) -> Result<Vec<models::Pool>, RestError<RestJsonError>> {
        let pools = match volume_id {
            Some(vol_id) => client().get(Filter::Volume(vol_id.into()), None).await?,
            None => client().get(Filter::None, None).await?,
        };
        Ok(pools.into_inner().into_iter().map(From::from).collect())
    }

    async fn put_node_pool(
        Path((node_id, pool_id)): Path<(String, String)>,
        Body(create_pool_body): Body<models::CreatePoolBody>,
    ) -> Result<models::Pool, RestError<RestJsonError>> {
        let create =
            CreatePoolBody::from(create_pool_body).to_request(node_id.into(), pool_id.into());
        let pool = client().create(&create, None).await?;
        Ok(pool.into())
    }

    async fn put_pool_label(
        Path((pool_id, key, value)): Path<(String, String, String)>,
        Query(overwrite): Query<Option<bool>>,
    ) -> Result<models::Pool, RestError<RestJsonError>> {
        let labels = HashMap::from([(key, value)]);
        let label_pool_request = LabelPool {
            pool_id: pool_id.into(),
            labels,
            overwrite: overwrite.unwrap_or(false),
        };

        let pool = client().label(&label_pool_request, None).await?;
        Ok(pool.into())
    }

    async fn del_pool_label(
        Path((pool_id, label_key)): Path<(String, String)>,
    ) -> Result<models::Pool, RestError<RestJsonError>> {
        let unlabel_pool_request = UnlabelPool {
            pool_id: pool_id.into(),
            label_key,
        };

        let pool = client().unlabel(&unlabel_pool_request, None).await?;
        Ok(pool.into())
    }
}

/// returns pool from pool option and returns an error on non existence
pub fn pool(pool_id: String, pool: Option<&Pool>) -> Result<Pool, ReplyError> {
    match pool {
        Some(pool) => Ok(pool.clone()),
        None => Err(ReplyError {
            kind: ReplyErrorKind::NotFound,
            resource: ResourceKind::Pool,
            source: "Requested pool was not found".to_string(),
            extra: format!("Pool id : {pool_id}"),
        }),
    }
}
