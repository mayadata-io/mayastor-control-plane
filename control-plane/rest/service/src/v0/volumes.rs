use super::*;
use grpc::operations::{
    nexus::traits::NexusOperations, volume::traits::VolumeOperations, MaxEntries, Pagination,
    StartingToken,
};
use stor_port::types::v0::{
    openapi::apis::Uuid,
    transport::{
        DestroyShutdownTargets, DestroyVolume, Filter, GetRebuildRecord, PublishVolume,
        RebuildHistory, RebuildJobState, RebuildRecord, RepublishVolume, ResizeVolume,
        SetVolumeProperty, SetVolumeReplica, ShareVolume, UnpublishVolume, UnshareVolume, Volume,
    },
};

use humantime::Timestamp;

fn client() -> impl VolumeOperations {
    core_grpc().volume()
}

fn nexus_client() -> impl NexusOperations {
    core_grpc().nexus()
}

#[async_trait::async_trait]
impl apis::actix_server::Volumes for RestApi {
    async fn del_share(Path(volume_id): Path<Uuid>) -> Result<(), RestError<RestJsonError>> {
        client()
            .unshare(
                &UnshareVolume {
                    uuid: volume_id.into(),
                },
                None,
            )
            .await?;
        Ok(())
    }

    async fn del_volume(Path(volume_id): Path<Uuid>) -> Result<(), RestError<RestJsonError>> {
        client()
            .destroy(
                &DestroyVolume {
                    uuid: volume_id.into(),
                },
                None,
            )
            .await?;
        Ok(())
    }

    async fn del_volume_shutdown_targets(
        Path(volume_id): Path<Uuid>,
    ) -> Result<(), RestError<RestJsonError>> {
        let destroy = DestroyShutdownTargets::new(volume_id.into(), None);
        client().destroy_shutdown_target(&destroy, None).await?;
        Ok(())
    }

    async fn del_volume_target(
        Path(volume_id): Path<Uuid>,
        Query((force, frontend_host)): Query<(Option<bool>, Option<String>)>,
    ) -> Result<models::Volume, RestError<RestJsonError>> {
        let volume = client()
            .unpublish(
                &UnpublishVolume::new(
                    &volume_id.into(),
                    force.unwrap_or(false),
                    frontend_host.into_iter().collect(),
                ),
                None,
            )
            .await?;
        Ok(volume.into())
    }

    async fn get_rebuild_history(
        Path(volume_id): Path<Uuid>,
    ) -> Result<models::RebuildHistory, RestError<RestJsonError>> {
        let volume = volume(
            volume_id.to_string(),
            client()
                .get(Filter::Volume(volume_id.into()), false, None, None)
                .await?
                .entries
                .get(0),
        )?;
        let target = volume
            .state()
            .target
            .ok_or(ReplyError::failed_precondition(
                ResourceKind::Nexus,
                "volume target not available".to_string(),
                String::new(),
            ))?;
        let record = nexus_client()
            .get_rebuild_history(&GetRebuildRecord::new(target.uuid), None)
            .await?;
        Ok(rebuild_history_to_rest(record))
    }

    async fn get_volume(
        Path(volume_id): Path<Uuid>,
    ) -> Result<models::Volume, RestError<RestJsonError>> {
        let volume = volume(
            volume_id.to_string(),
            client()
                .get(Filter::Volume(volume_id.into()), false, None, None)
                .await?
                .entries
                .get(0),
        )?;
        Ok(volume.into())
    }

    async fn get_volumes(
        Query((volume_id, max_entries, starting_token)): Query<(
            Option<Uuid>,
            isize,
            Option<isize>,
        )>,
    ) -> Result<models::Volumes, RestError<RestJsonError>> {
        let starting_token = starting_token.unwrap_or_default();

        // If max entries is 0, pagination is disabled. All volumes will be returned in a single
        // call.
        let pagination = if max_entries > 0 {
            Some(Pagination::new(
                max_entries as MaxEntries,
                starting_token as StartingToken,
            ))
        } else {
            None
        };
        let volumes = match volume_id {
            Some(volume_id) => {
                client()
                    .get(Filter::Volume(volume_id.into()), true, pagination, None)
                    .await?
            }
            None => client().get(Filter::None, false, pagination, None).await?,
        };

        Ok(models::Volumes {
            entries: volumes.entries.into_iter().map(|e| e.into()).collect(),
            next_token: volumes.next_token.map(|t| t as isize),
        })
    }

    async fn put_snapshot_volume(
        Path((snapshot_id, volume_id)): Path<(Uuid, Uuid)>,
        Body(create_volume_body): Body<models::CreateVolumeBody>,
    ) -> Result<models::Volume, RestError<RestJsonError>> {
        let create = CreateVolumeBody::from(create_volume_body)
            .to_create_snapshot_volume(snapshot_id.into(), volume_id.into());
        let volume = client().create_snapshot_volume(&create, None).await?;
        Ok(volume.into())
    }

    async fn put_volume(
        Path(volume_id): Path<Uuid>,
        Body(create_volume_body): Body<models::CreateVolumeBody>,
    ) -> Result<models::Volume, RestError<RestJsonError>> {
        let create = CreateVolumeBody::from(create_volume_body).to_create_volume(volume_id.into());
        let volume = client().create(&create, None).await?;
        Ok(volume.into())
    }

    async fn put_volume_replica_count(
        Path((volume_id, replica_count)): Path<(Uuid, u8)>,
    ) -> Result<models::Volume, RestError<RestJsonError>> {
        let volume = client()
            .set_replica(
                &SetVolumeReplica {
                    uuid: volume_id.into(),
                    replicas: replica_count,
                },
                None,
            )
            .await?;
        Ok(volume.into())
    }

    async fn put_volume_property(
        Path(volume_id): Path<Uuid>,
        Body(set_volume_property_body): Body<models::SetVolumePropertyBody>,
    ) -> Result<models::Volume, RestError<RestJsonError>> {
        let prop = SetVolumePropertyBody::from(set_volume_property_body);
        let volume = client()
            .set_property(
                &SetVolumeProperty {
                    uuid: volume_id.into(),
                    property: prop.property,
                },
                None,
            )
            .await?;
        Ok(volume.into())
    }

    async fn put_volume_share(
        Path((volume_id, protocol)): Path<(Uuid, models::VolumeShareProtocol)>,
        Query(frontend_host): Query<Option<String>>,
    ) -> Result<String, RestError<RestJsonError>> {
        let share_uri = client()
            .share(
                &ShareVolume {
                    uuid: volume_id.into(),
                    protocol: protocol.into(),
                    frontend_hosts: match frontend_host {
                        Some(host) => vec![host],
                        None => vec![],
                    },
                },
                None,
            )
            .await?;
        Ok(share_uri)
    }

    async fn put_volume_size(
        Path(volume_id): Path<Uuid>,
        Body(resize_volume_body): Body<models::ResizeVolumeBody>,
    ) -> Result<models::Volume, RestError<RestJsonError>> {
        let volume = client()
            .resize(
                &ResizeVolume {
                    uuid: volume_id.into(),
                    requested_size: resize_volume_body.size as u64,
                    cluster_capacity_limit: None,
                },
                None,
            )
            .await?;

        Ok(volume.into())
    }

    async fn put_volume_target(
        Path(volume_id): Path<Uuid>,
        Body(publish_volume_body): Body<models::PublishVolumeBody>,
    ) -> Result<models::Volume, RestError<RestJsonError>> {
        let volume = match publish_volume_body.republish.unwrap_or(false) {
            true => {
                client()
                    .republish(
                        &RepublishVolume {
                            uuid: volume_id.into(),
                            target_node: publish_volume_body.node.map(|id| id.into()),
                            share: publish_volume_body.protocol.into(),
                            reuse_existing: publish_volume_body.reuse_existing.unwrap_or(true),
                            reuse_existing_fallback: false,
                            frontend_node: publish_volume_body
                                .frontend_node
                                .unwrap_or_default()
                                .into(),
                        },
                        None,
                    )
                    .await?
            }
            false => {
                client()
                    .publish(
                        &PublishVolume {
                            uuid: volume_id.into(),
                            target_node: publish_volume_body.node.map(|id| id.into()),
                            share: Some(publish_volume_body.protocol.into()),
                            publish_context: publish_volume_body.publish_context,
                            frontend_nodes: publish_volume_body.frontend_node.into_iter().collect(),
                        },
                        None,
                    )
                    .await?
            }
        };

        Ok(volume.into())
    }
}

/// returns volume from volume option and returns an error on non existence
fn volume(volume_id: String, volume: Option<&Volume>) -> Result<Volume, ReplyError> {
    match volume {
        Some(volume) => Ok(volume.clone()),
        None => Err(ReplyError {
            kind: ReplyErrorKind::NotFound,
            resource: ResourceKind::Volume,
            source: "Requested volume was not found".to_string(),
            extra: format!("Volume id : {volume_id}"),
        }),
    }
}

fn rebuild_history_to_rest(value: RebuildHistory) -> models::RebuildHistory {
    let records: Vec<_> = value
        .records
        .into_iter()
        .map(rebuild_record_to_rest)
        .collect();
    models::RebuildHistory::new(value.uuid, records)
}

fn rebuild_record_to_rest(value: RebuildRecord) -> models::RebuildRecord {
    let start = Timestamp::from(value.start_time).to_string();
    let end = Timestamp::from(value.end_time).to_string();
    let state = rebuild_state_to_rest(value.state);
    models::RebuildRecord::new(
        value.child_uri,
        value.src_uri,
        state,
        value.blocks_total as isize,
        value.blocks_recovered as isize,
        value.blocks_transferred as isize,
        value.blocks_remaining as isize,
        value.block_size as isize,
        value.is_partial,
        start,
        end,
    )
}

fn rebuild_state_to_rest(value: RebuildJobState) -> models::RebuildJobState {
    match value {
        RebuildJobState::Init => models::RebuildJobState::Init,
        RebuildJobState::Rebuilding => models::RebuildJobState::Rebuilding,
        RebuildJobState::Stopped => models::RebuildJobState::Stopped,
        RebuildJobState::Paused => models::RebuildJobState::Paused,
        RebuildJobState::Failed => models::RebuildJobState::Failed,
        RebuildJobState::Completed => models::RebuildJobState::Completed,
    }
}
