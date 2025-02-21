use super::*;
use crate::v0::nexuses::nexus;
use grpc::operations::nexus::traits::NexusOperations;
use stor_port::{
    transport_api::{ReplyError, ReplyErrorKind, ResourceKind},
    types::v0::{
        openapi::apis::Uuid,
        transport::{AddNexusChild, Child, ChildUri, Filter, Nexus, RemoveNexusChild},
    },
};

fn client() -> impl NexusOperations {
    core_grpc().nexus()
}

async fn get_children_response(
    filter: Filter,
) -> Result<Vec<models::Child>, RestError<RestJsonError>> {
    let nexus = nexus(
        match &filter {
            Filter::NodeNexus(_, id) => Some(id.to_string()),
            Filter::Nexus(id) => Some(id.to_string()),
            _ => None,
        },
        client().get(filter, None).await?.into_inner().first(),
    )?;
    Ok(nexus.children.into_iter().map(From::from).collect())
}

async fn get_child_response(
    child_id: ChildUri,
    query: &str,
    filter: Filter,
) -> Result<models::Child, RestError<RestJsonError>> {
    let child_id = build_child_uri(child_id, query);
    let nexus = nexus(
        match &filter {
            Filter::NodeNexus(_, id) => Some(id.to_string()),
            Filter::Nexus(id) => Some(id.to_string()),
            _ => None,
        },
        client().get(filter, None).await?.into_inner().first(),
    )?;
    let child = find_nexus_child(&nexus, &child_id)?;
    Ok(child.into())
}

fn find_nexus_child(nexus: &Nexus, child_uri: &ChildUri) -> Result<Child, ReplyError> {
    if let Some(child) = nexus.children.iter().find(|&c| &c.uri == child_uri) {
        Ok(child.clone())
    } else {
        Err(ReplyError {
            kind: ReplyErrorKind::NotFound,
            resource: ResourceKind::Child,
            source: "find_nexus_child".to_string(),
            extra: "".to_string(),
        })
    }
}

async fn add_child_filtered(
    child_id: ChildUri,
    query: &str,
    filter: Filter,
) -> Result<models::Child, RestError<RestJsonError>> {
    let child_uri = build_child_uri(child_id, query);

    let nexus = match nexus(
        match &filter {
            Filter::NodeNexus(_, id) => Some(id.to_string()),
            Filter::Nexus(id) => Some(id.to_string()),
            _ => None,
        },
        client().get(filter, None).await?.into_inner().first(),
    ) {
        Ok(nexus) => nexus,
        Err(error) => return Err(RestError::from(error)),
    };

    let create = AddNexusChild {
        node: nexus.node,
        nexus: nexus.uuid,
        uri: child_uri,
        auto_rebuild: true,
    };
    let child = client().add_nexus_child(&create, None).await?;
    Ok(child.into())
}

async fn delete_child_filtered(
    child_id: ChildUri,
    query: &str,
    filter: Filter,
) -> Result<(), RestError<RestJsonError>> {
    let child_uri = build_child_uri(child_id, query);

    let nexus = match nexus(
        match &filter {
            Filter::NodeNexus(_, id) => Some(id.to_string()),
            Filter::Nexus(id) => Some(id.to_string()),
            _ => None,
        },
        client().get(filter, None).await?.into_inner().first(),
    ) {
        Ok(nexus) => nexus,
        Err(error) => return Err(RestError::from(error)),
    };

    let destroy = RemoveNexusChild {
        node: nexus.node,
        nexus: nexus.uuid,
        uri: child_uri,
    };

    client().remove_nexus_child(&destroy, None).await?;
    Ok(())
}

/// The child uri should be in the "percent-encode" format, but if it's not try to use
/// the query string to build up the url
fn build_child_uri(child_id: ChildUri, query: &str) -> ChildUri {
    let child_id = child_id.to_string();
    ChildUri::from(match url::Url::parse(child_id.as_str()) {
        Ok(_) => {
            if query.is_empty() {
                child_id
            } else {
                format!("{child_id}?{query}")
            }
        }
        _ => {
            // not a URL, it's probably legacy, default to AIO
            format!("aio://{child_id}")
        }
    })
}

#[async_trait::async_trait]
impl apis::actix_server::Children for RestApi {
    async fn del_nexus_child(
        query: &str,
        Path((nexus_id, child_id)): Path<(Uuid, String)>,
    ) -> Result<(), RestError<RestJsonError>> {
        delete_child_filtered(child_id.into(), query, Filter::Nexus(nexus_id.into())).await
    }

    async fn del_node_nexus_child(
        query: &str,
        Path((node_id, nexus_id, child_id)): Path<(String, Uuid, String)>,
    ) -> Result<(), RestError<RestJsonError>> {
        delete_child_filtered(
            child_id.into(),
            query,
            Filter::NodeNexus(node_id.into(), nexus_id.into()),
        )
        .await
    }

    async fn get_nexus_child(
        query: &str,
        Path((nexus_id, child_id)): Path<(Uuid, String)>,
    ) -> Result<models::Child, RestError<RestJsonError>> {
        get_child_response(child_id.into(), query, Filter::Nexus(nexus_id.into())).await
    }

    async fn get_nexus_children(
        Path(nexus_id): Path<Uuid>,
    ) -> Result<Vec<models::Child>, RestError<RestJsonError>> {
        get_children_response(Filter::Nexus(nexus_id.into())).await
    }

    async fn get_node_nexus_child(
        query: &str,
        Path((node_id, nexus_id, child_id)): Path<(String, Uuid, String)>,
    ) -> Result<models::Child, RestError<RestJsonError>> {
        get_child_response(
            child_id.into(),
            query,
            Filter::NodeNexus(node_id.into(), nexus_id.into()),
        )
        .await
    }

    async fn get_node_nexus_children(
        Path((node_id, nexus_id)): Path<(String, Uuid)>,
    ) -> Result<Vec<models::Child>, RestError<RestJsonError>> {
        get_children_response(Filter::NodeNexus(node_id.into(), nexus_id.into())).await
    }

    async fn put_nexus_child(
        query: &str,
        Path((nexus_id, child_id)): Path<(Uuid, String)>,
    ) -> Result<models::Child, RestError<RestJsonError>> {
        add_child_filtered(child_id.into(), query, Filter::Nexus(nexus_id.into())).await
    }

    async fn put_node_nexus_child(
        query: &str,
        Path((node_id, nexus_id, child_id)): Path<(String, Uuid, String)>,
    ) -> Result<models::Child, RestError<RestJsonError>> {
        add_child_filtered(
            child_id.into(),
            query,
            Filter::NodeNexus(node_id.into(), nexus_id.into()),
        )
        .await
    }
}
