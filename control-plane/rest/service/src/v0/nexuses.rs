use super::*;
use grpc::operations::nexus::traits::NexusOperations;
use stor_port::{
    transport_api::{ReplyError, ReplyErrorKind, ResourceKind},
    types::v0::{
        openapi::apis::Uuid,
        transport::{DestroyNexus, Filter, UnshareNexus},
    },
};

fn client() -> impl NexusOperations {
    core_grpc().nexus()
}

async fn destroy_nexus(filter: Filter) -> Result<(), RestError<RestJsonError>> {
    let destroy = match filter.clone() {
        Filter::NodeNexus(node_id, nexus_id) => DestroyNexus::new(node_id, nexus_id),
        Filter::Nexus(nexus_id) => {
            let node_id = match client().get(filter, None).await {
                Ok(nexuses) => {
                    nexus(Some(nexus_id.to_string()), nexuses.into_inner().first())?.node
                }
                Err(error) => return Err(RestError::from(error)),
            };
            DestroyNexus::new(node_id, nexus_id)
        }
        _ => {
            return Err(RestError::from(ReplyError {
                kind: ReplyErrorKind::Internal,
                resource: ResourceKind::Nexus,
                source: "destroy_nexus".to_string(),
                extra: "invalid filter for resource".to_string(),
            }))
        }
    };

    client().destroy(&destroy, None).await?;

    Ok(())
}

#[async_trait::async_trait]
impl apis::actix_server::Nexuses for RestApi {
    async fn del_nexus(Path(nexus_id): Path<Uuid>) -> Result<(), RestError<RestJsonError>> {
        destroy_nexus(Filter::Nexus(nexus_id.into())).await
    }

    async fn del_node_nexus(
        Path((node_id, nexus_id)): Path<(String, Uuid)>,
    ) -> Result<(), RestError<RestJsonError>> {
        destroy_nexus(Filter::NodeNexus(node_id.into(), nexus_id.into())).await
    }

    async fn del_node_nexus_share(
        Path((node_id, nexus_id)): Path<(String, Uuid)>,
    ) -> Result<(), RestError<RestJsonError>> {
        client()
            .unshare(
                &UnshareNexus {
                    node: node_id.into(),
                    uuid: nexus_id.into(),
                },
                None,
            )
            .await?;
        Ok(())
    }

    async fn get_nexus(
        Path(nexus_id): Path<Uuid>,
    ) -> Result<models::Nexus, RestError<RestJsonError>> {
        let nexus = nexus(
            Some(nexus_id.to_string()),
            client()
                .get(Filter::Nexus(nexus_id.into()), None)
                .await?
                .into_inner()
                .first(),
        )?;
        Ok(nexus.into())
    }

    async fn get_nexuses() -> Result<Vec<models::Nexus>, RestError<RestJsonError>> {
        let nexuses = client().get(Filter::None, None).await?;
        Ok(nexuses.into_inner().into_iter().map(From::from).collect())
    }

    async fn get_node_nexus(
        Path((node_id, nexus_id)): Path<(String, Uuid)>,
    ) -> Result<models::Nexus, RestError<RestJsonError>> {
        let nexus = nexus(
            Some(nexus_id.to_string()),
            client()
                .get(Filter::NodeNexus(node_id.into(), nexus_id.into()), None)
                .await?
                .into_inner()
                .first(),
        )?;
        Ok(nexus.into())
    }

    async fn get_node_nexuses(
        Path(id): Path<String>,
    ) -> Result<Vec<models::Nexus>, RestError<RestJsonError>> {
        let nexuses = client().get(Filter::Node(id.into()), None).await?;
        Ok(nexuses.into_inner().into_iter().map(From::from).collect())
    }

    async fn put_node_nexus(
        Path((node_id, nexus_id)): Path<(String, Uuid)>,
        Body(create_nexus_body): Body<models::CreateNexusBody>,
    ) -> Result<models::Nexus, RestError<RestJsonError>> {
        let create =
            CreateNexusBody::from(create_nexus_body).to_request(node_id.into(), nexus_id.into());
        let nexus = client().create(&create, None).await?;
        Ok(nexus.into())
    }

    async fn put_node_nexus_share(
        Path((node_id, nexus_id, protocol)): Path<(String, Uuid, models::NexusShareProtocol)>,
    ) -> Result<String, RestError<RestJsonError>> {
        let share = ShareNexus {
            node: node_id.into(),
            uuid: nexus_id.into(),
            key: None,
            protocol: protocol.into(),
            allowed_hosts: vec![],
        };
        let share_uri = client().share(&share, None).await?;
        Ok(share_uri)
    }
}

/// returns nexus from nexus option and returns an error on non existence
pub fn nexus(nexus_id: Option<String>, nexus: Option<&Nexus>) -> Result<Nexus, ReplyError> {
    match nexus {
        Some(nexus) => Ok(nexus.clone()),
        None => Err(ReplyError {
            kind: ReplyErrorKind::NotFound,
            resource: ResourceKind::Nexus,
            source: "Requested nexus was not found".to_string(),
            extra: match nexus_id {
                None => "".to_string(),
                Some(id) => format!("Nexus id : {id}"),
            },
        }),
    }
}
