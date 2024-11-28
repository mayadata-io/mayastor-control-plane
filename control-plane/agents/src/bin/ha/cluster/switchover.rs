use crate::{core_grpc, etcd::EtcdStore, nodes::NodeList};
use agents::eventing::Event;
use anyhow::anyhow;
use chrono::Utc;
use events_api::event::{
    EventAction, EventCategory, EventMessage, EventMeta, EventSource, SwitchOverStatus,
};
use grpc::operations::{
    ha_node::{client::NodeAgentClient, traits::NodeAgentOperations},
    volume::traits::VolumeOperations,
};
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use stor_port::{
    transport_api::{ReplyError, ReplyErrorKind},
    types::v0::{
        store::{
            switchover::{Operation, OperationState, SwitchOverSpec, SwitchOverTime},
            SpecTransaction,
        },
        transport::{
            DestroyShutdownTargets, GetController, NodeId, NvmeSubsystem, ReplacePath,
            RepublishVolume, Volume, VolumeId, VolumeShareProtocol,
        },
    },
};
use tokio::sync::{
    mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
    Mutex,
};
use tonic::transport::Uri;
use tracing::{error, info, warn};

fn client() -> impl VolumeOperations {
    core_grpc().volume()
}

const REPLACE_PATH_DEADLINE_RETRIES: u64 = 3;

/// Stage represents the steps for switchover request.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Copy, Clone, Ord, PartialOrd)]
pub(crate) enum Stage {
    /// Initialize switchover request.
    Init,
    /// Shutdown original/old volume target. Create new nexus for existing vol obj.
    RepublishVolume,
    /// Send updated path of volume to node-agent.
    ReplacePath,
    /// Delete original/old volume target.
    DeleteTarget,
    /// Marks switchover process as Complete.
    Successful,
    /// Represent failed switchover request.
    Errored,
}

/// A ref counted switchover stage which can be used to share stage information.
#[derive(Debug, Clone)]
pub(crate) struct SwitchOverStage(Arc<parking_lot::RwLock<Stage>>);
impl SwitchOverStage {
    /// Create a new `Self` for the given stage.
    pub(crate) fn new(stage: Stage) -> Self {
        SwitchOverStage(Arc::new(parking_lot::RwLock::new(stage)))
    }
    /// Read the stage value.
    pub(crate) fn read(&self) -> Stage {
        *self.0.read()
    }
}

impl PartialEq for SwitchOverStage {
    fn eq(&self, other: &Self) -> bool {
        *self.0.read() == *other.0.read()
    }
}
impl Eq for SwitchOverStage {}

/// SwitchOverRequest defines spec for switchover.
#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct SwitchOverRequest {
    callback_uri: SocketAddr,
    volume_id: VolumeId,
    stage: SwitchOverStage,
    /// The nodename of the request's originator node.
    node_name: NodeId,
    /// Timestamp when switchover request was initialized.
    timestamp: SwitchOverTime,
    /// Failed nexus path of the volume.
    existing_nqn: String,
    /// New nexus path of the volume.
    new_path: Option<String>,
    /// Number of failed attempts in the current Stage.
    retry_count: u64,
    /// Reuse existing target by default.
    reuse_existing: bool,
    /// Publish context.
    publish_context: Option<HashMap<String, String>>,
    /// The first time we handle exhaustion, retry right away.
    fast_exhaustion_retry: bool,
}

impl Ord for SwitchOverRequest {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.stage().cmp(&other.stage()) {
            Ordering::Equal => self.timestamp.cmp(&other.timestamp),
            other => other,
        }
    }
}

impl PartialOrd for SwitchOverRequest {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl SwitchOverRequest {
    /// Create a new switchover request for every failed Nvme path.
    pub(crate) fn new(
        callback_uri: SocketAddr,
        volume: VolumeId,
        node_name: NodeId,
        existing_path: String,
    ) -> SwitchOverRequest {
        SwitchOverRequest {
            callback_uri,
            volume_id: volume,
            stage: SwitchOverStage::new(Stage::Init),
            node_name,
            timestamp: Utc::now(),
            existing_nqn: existing_path,
            new_path: None,
            retry_count: 0,
            reuse_existing: true,
            publish_context: None,
            fast_exhaustion_retry: true,
        }
    }

    /// Get a ref-counted switchover stage.
    pub(crate) fn stage_arc(&self) -> SwitchOverStage {
        self.stage.clone()
    }

    /// Set the reuse_existing flag.
    pub(crate) fn set_reuse_existing(&mut self, reuse_existing: bool) {
        self.reuse_existing = reuse_existing;
    }

    /// Get the node agent socket address.
    pub(crate) fn socket(&self) -> SocketAddr {
        self.callback_uri
    }
    /// Get the nqn of this path.
    pub(crate) fn nqn(&self) -> &str {
        &self.existing_nqn
    }

    /// Update stage with next stage.
    /// If a stage is PublishPath or Errored then it will not be updated.
    pub(crate) fn update_next_stage(&mut self) {
        self.set_stage(match self.stage() {
            Stage::Init => Stage::RepublishVolume,
            Stage::RepublishVolume => Stage::ReplacePath,
            Stage::ReplacePath => Stage::DeleteTarget,
            Stage::DeleteTarget => Stage::Successful,
            // Successful and Errored stage mark request as complete, so no need to update
            Stage::Successful => Stage::Successful,
            Stage::Errored => Stage::Errored,
        });
    }

    fn node_uri(&self) -> Result<Uri, http::Error> {
        Uri::builder()
            .scheme("http")
            .authority(self.callback_uri.to_string())
            .path_and_query("")
            .build()
    }
    fn build_node_uri(&self) -> Result<Uri, anyhow::Error> {
        match self.node_uri() {
            Ok(node) => Ok(node),
            Err(error) => {
                self.set_stage(Stage::Errored);
                error!(%error, volume.uuid=%self.volume_id, "Could not get grpc address of the node agent. Moving Switchover req to Errored");
                Err(anyhow!(
                    "Could not get grpc address of the node agent: {error}"
                ))
            }
        }
    }
    fn set_stage(&self, stage: Stage) {
        *self.stage.0.write() = stage;
    }
    fn stage(&self) -> Stage {
        *self.stage.0.read()
    }

    /// Start_op must be called before starting the respective stage operation on the request.
    /// Start_op will store the request with updated stage in formation in etcd.
    pub(crate) async fn start_op(&self, etcd: &EtcdStore) -> Result<(), anyhow::Error> {
        let spec: SwitchOverSpec = self.into();
        etcd.store_obj(&spec).await
    }

    /// Complete_op must be called after completion of the respective stage operation on the
    /// request.
    /// Complete_op will store the request in etcd with either updated stage or error message.
    pub(crate) async fn complete_op(
        &self,
        result: bool,
        msg: String,
        etcd: &EtcdStore,
    ) -> Result<(), anyhow::Error> {
        let mut spec = SwitchOverSpec::from(self);

        if result {
            spec.set_op_result(result);
            spec.commit_op();
        } else {
            spec.set_error_msg(msg);
        }
        etcd.store_obj(&spec).await?;

        Ok(())
    }

    /// Delete the request from the persistent store.
    pub(crate) async fn delete_request(&self, etcd: &EtcdStore) -> Result<(), anyhow::Error> {
        let spec = SwitchOverSpec::from(self);
        etcd.delete_obj(&spec).await
    }

    /// Initialize the switchover request.
    async fn initialize(&mut self, etcd: &EtcdStore) -> Result<(), anyhow::Error> {
        self.start_op(etcd).await?;
        info!(volume.uuid=%self.volume_id, "Initializing");
        self.complete_op(true, "".to_string(), etcd).await?;
        self.update_next_stage();
        Ok(())
    }

    async fn send_republish_volume(
        &self,
        reuse_existing_fallback: bool,
    ) -> Result<Volume, ReplyError> {
        let republish_req = RepublishVolume {
            uuid: self.volume_id.clone(),
            target_node: None,
            share: VolumeShareProtocol::Nvmf,
            reuse_existing: self.reuse_existing,
            frontend_node: self.node_name.clone(),
            reuse_existing_fallback,
        };
        client().republish(&republish_req, None).await
    }
    /// Cleanup volume targets that are not registered as Nvme Subsystems in the Node.
    async fn destroy_shutdown(&self) {
        let Ok(uri) = self.build_node_uri() else {
            return;
        };
        if let Some(new_path) = self.new_path.clone() {
            let node_client = NodeAgentClient::new(uri, None).await;
            let request = GetController::new(new_path);
            if let Err(error) = match node_client.get_nvme_controller(&request, None).await {
                Ok(target_addresses) => {
                    self.destroy_shutdown_targets(target_addresses.into_inner())
                        .await
                }
                Err(err) if matches!(err.kind, ReplyErrorKind::NotFound) => {
                    self.destroy_all_shutdown_targets().await
                }
                Err(error) => Err(anyhow!(
                    "Failed to list Nvme subsystems for the volume, error: {error}"
                )),
            } {
                tracing::error!(%error, "Failed to destroy all shutdown targets");
            } else {
                tracing::info!("Retrying Republish after cleaning up targets");
            }
        }
    }

    /// Attempts to republish the volume by either recreating the existing target or by
    /// creating a new target (on a different node).
    #[tracing::instrument(level = "info", skip(self), fields(volume.uuid = %self.volume_id), err)]
    async fn republish_volume(&mut self, etcd: &EtcdStore) -> Result<(), anyhow::Error> {
        self.start_op(etcd).await?;
        info!(volume.uuid=%self.volume_id, frontend_node=%self.node_name, "Republishing");
        let vol = match self.send_republish_volume(false).await {
            Ok(vol) => Ok(vol),
            Err(error)
                if matches!(
                    error.kind,
                    ReplyErrorKind::PermissionDenied
                        | ReplyErrorKind::NotPublished
                        | ReplyErrorKind::NotFound
                ) =>
            {
                error!(volume.uuid=%self.volume_id, %error, "Cancelling switchover");
                self.set_stage(Stage::Errored);
                Err(error)
            }
            Err(error) if error.kind == ReplyErrorKind::ResourceExhausted => {
                self.destroy_shutdown().await;
                self.send_republish_volume(true).await
            }
            Err(error) => Err(error),
        }?;
        self.publish_context = vol.spec().publish_context;
        self.new_path = match vol.state().target {
            Some(target) => Some(target.device_uri),
            _ => None,
        };
        if self.new_path.is_none() {
            error!(volume.uuid=%self.volume_id, "Could not find device uri for the volume");
            return Err(anyhow!("Couldnt find device uri for the volume"));
        }
        self.complete_op(true, "".to_string(), etcd).await?;
        self.update_next_stage();
        Ok(())
    }

    /// Destroy old/original target.
    async fn delete_target(&mut self, etcd: &EtcdStore) -> Result<(), anyhow::Error> {
        self.start_op(etcd).await?;

        self.destroy_all_shutdown_targets().await?;

        self.complete_op(true, "".to_string(), etcd).await?;
        self.update_next_stage();
        Ok(())
    }

    async fn destroy_all_shutdown_targets(&self) -> Result<(), anyhow::Error> {
        let destroy_request = DestroyShutdownTargets::new(self.volume_id.clone(), None);
        self.request_destroy_shutdown_targets(destroy_request).await
    }
    async fn request_destroy_shutdown_targets(
        &self,
        request: DestroyShutdownTargets,
    ) -> Result<(), anyhow::Error> {
        info!(volume.uuid=%self.volume_id, "Deleting shutdown volume targets");

        match client().destroy_shutdown_target(&request, None).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind == ReplyErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
    async fn destroy_shutdown_targets(
        &self,
        subsystems: Vec<NvmeSubsystem>,
    ) -> Result<(), anyhow::Error> {
        let targets = subsystems
            .into_iter()
            .map(|addr| addr.into_address())
            .collect::<Vec<_>>();
        let destroy_request = DestroyShutdownTargets::new(self.volume_id.clone(), Some(targets));
        self.request_destroy_shutdown_targets(destroy_request).await
    }

    /// Deletes Successful Switchover request from etcd.
    async fn delete_switchover(&mut self, etcd: &EtcdStore) -> Result<(), anyhow::Error> {
        self.start_op(etcd).await?;
        info!(volume.uuid=%self.volume_id, "Deleting Switchover request from etcd as request completed successfully");
        match self.delete_request(etcd).await {
            Ok(_) => Ok(()),
            Err(_) => Err(anyhow!(
                "Encountered error while trying to delete SwitchOverSpec from etcd for {}",
                self.volume_id
            )),
        }
    }

    /// Deletes Errored Switchover request from etcd.
    async fn errored_switchover(
        &mut self,
        etcd: &EtcdStore,
        nodes: &NodeList,
    ) -> Result<(), anyhow::Error> {
        self.start_op(etcd).await?;
        info!(volume.uuid=%self.volume_id, "Error occurred while processing Switchover request");

        // Should we do this here, or go via the delete targets stage with an extra error
        // information in order to transition to either success or error stage?
        self.destroy_all_shutdown_targets().await?;

        match self.delete_request(etcd).await {
            Ok(_) => {
                nodes.remove_failed_path(&self.existing_nqn).await;
                Ok(())
            }
            Err(_) => Err(anyhow!(
                "Encountered error while trying to delete SwitchOverSpec from etcd for {}",
                self.volume_id
            )),
        }
    }

    /// Publish updated path for the volume to node-agent.
    async fn replace_path(
        &mut self,
        etcd: &EtcdStore,
        nodes: &NodeList,
    ) -> Result<(), anyhow::Error> {
        self.start_op(etcd).await?;
        info!(volume.uuid=%self.volume_id, "Sending new volume target to node agent");
        let uri = self.build_node_uri()?;
        info!(%uri, "Creating node agent client using callback uri");

        let Some(new_path) = self.new_path.clone() else {
            return Err(anyhow!("Could not to get new nexus target for the volume"));
        };

        let replace_request = ReplacePath::new(
            self.existing_nqn.clone(),
            new_path,
            self.publish_context.clone(),
        );
        let client = NodeAgentClient::new(uri, None).await;
        match client.replace_path(&replace_request, None).await {
            Ok(_) => Ok(()),
            Err(error) if error.kind == ReplyErrorKind::FailedPrecondition => {
                warn!(path=%self.existing_nqn, "HA Node agent could not find failed Nvme path");
                // otherwise this means we were able to reconnect even before we got called!
                if !self.reuse_existing {
                    info!(volume.uuid=%self.volume_id, "Moving Switchover request to Errored state");
                    self.set_stage(Stage::Errored);
                    Err(anyhow!("HA Node agent could not lookup old Nvme path"))
                } else {
                    Ok(())
                }
            }
            Err(error)
                if matches!(
                    error.kind,
                    ReplyErrorKind::Aborted
                        | ReplyErrorKind::Timeout
                        | ReplyErrorKind::NotFound
                        | ReplyErrorKind::DeadlineExceeded
                ) =>
            {
                if !matches!(error.kind, ReplyErrorKind::NotFound) {
                    warn!(path=%self.existing_nqn, "HA Node agent failed to replace path within timeout");
                    if self.retry_count < REPLACE_PATH_DEADLINE_RETRIES {
                        return Err(anyhow!("Nvme path deadline exceeded"));
                    }
                }
                self.set_reuse_existing(!self.reuse_existing);
                info!(volume.uuid=%self.volume_id, reuse_existing=self.reuse_existing, "Retrying Republish");
                self.set_stage(Stage::RepublishVolume);
                Err(anyhow!("Nvme path replacement failed with older target"))
            }
            Err(error) => Err(anyhow!("Nvme path replacement failed: {error}")),
        }?;

        nodes.remove_failed_path(&self.existing_nqn).await;
        self.complete_op(true, "".to_string(), etcd).await?;
        self.update_next_stage();
        Ok(())
    }
}

/// SwitchOverEngine defines spec for switchover engine.
#[derive(Debug, Clone)]
pub(crate) struct SwitchOverEngine {
    etcd: EtcdStore,
    fast_requeue: Option<humantime::Duration>,
    nodes: NodeList,
    channel: UnboundedSender<SwitchOverRequest>,
}

/// Number of Switchover worker tasks to be spawned.
const WORKER_NUM: u8 = 4;

enum ReQueue {
    /// Sleep duration during Fast requeue phase.
    Fast = 10,
    /// Sleep duration during Slow requeue phase.
    Slow = 30,
    /// Number of Fast request requeue per stage.
    NumFast = 18,
}

impl SwitchOverEngine {
    /// Creates a new switchover engine to process Nvme path failures.
    pub(crate) fn new(
        etcd: EtcdStore,
        fast_requeue: Option<humantime::Duration>,
        nodes: NodeList,
    ) -> Self {
        let (rq_tx, rq_rx) = unbounded_channel();

        let sw = SwitchOverEngine {
            channel: rq_tx,
            etcd,
            nodes,
            fast_requeue,
        };

        sw.init_worker(Arc::new(Mutex::new(rq_rx)));
        sw
    }
    pub(crate) fn nodes(&self) -> &NodeList {
        &self.nodes
    }

    /// Instantiates worker task to asynchronously process Switchover request.
    pub(crate) fn init_worker(&self, recv: Arc<Mutex<UnboundedReceiver<SwitchOverRequest>>>) {
        for i in 0..WORKER_NUM {
            info!(worker = i, "Spawning Switchover Engine worker");
            let cloned_self = self.clone();
            let cloned_channel = recv.clone();
            tokio::spawn(async move { cloned_self.worker(cloned_channel, i).await });
        }
    }

    /// Switchover request to be handled synchronously in each worker task.
    async fn worker(self, recv: Arc<Mutex<UnboundedReceiver<SwitchOverRequest>>>, worker_num: u8) {
        loop {
            let request = match recv.lock().await.recv().await {
                None => break,
                Some(request) => request,
            };

            info!(
                volume.uuid = %request.volume_id,
                worker = worker_num,
                "Volume switchover picked up by worker"
            );
            self.work_request(request).await;
        }
        info!(worker = worker_num, "Switchover Engine worker stopped");
    }

    /// Handle Switchover request synchronously as long as it's succeeding on each stage.
    /// Failed requests are sent back to the work queue to be picked up later.
    async fn work_request(&self, mut request: SwitchOverRequest) {
        loop {
            let result = match request.stage() {
                Stage::Init => {
                    request.event(EventAction::SwitchOver).generate();
                    request.initialize(&self.etcd).await
                }
                Stage::RepublishVolume => request.republish_volume(&self.etcd).await,
                Stage::ReplacePath => request.replace_path(&self.etcd, &self.nodes).await,
                Stage::DeleteTarget => request.delete_target(&self.etcd).await,
                Stage::Errored => {
                    let event = request.event(EventAction::SwitchOver);
                    match request.errored_switchover(&self.etcd, &self.nodes).await {
                        Ok(_) => {
                            event.generate();
                            break;
                        }
                        Err(e) => Err(e),
                    }
                }
                Stage::Successful => match request.delete_switchover(&self.etcd).await {
                    Ok(_) => {
                        request.event(EventAction::SwitchOver).generate();
                        break;
                    }
                    Err(e) => Err(e),
                },
            };
            match result {
                Ok(_) => {
                    // reset retry count back to the start after successfully completing a stage.
                    request.retry_count = 0;
                }
                Err(error) => {
                    info!(
                        volume.uuid = %request.volume_id,
                        %error,
                        "Sending failed Switchover request back to the work queue"
                    );
                    request.retry_count += 1;
                    self.enqueue(request);
                    break;
                }
            }
        }
    }

    /// Sends Switchover request to the channel after sleeping for sometime (if necessary).
    pub(crate) fn enqueue(&self, req: SwitchOverRequest) {
        let tx_clone = self.channel.clone();
        let fast_requeue = self.fast_requeue;
        tokio::spawn(async move {
            let errored_request = req.retry_count > 0;
            let retry_delay = if req.retry_count < ReQueue::NumFast as u64 {
                match fast_requeue {
                    None => ReQueue::Fast as u64,
                    Some(duration) => duration.as_secs(),
                }
            } else {
                ReQueue::Slow as u64
            };
            if errored_request {
                tokio::time::sleep(Duration::from_secs(retry_delay)).await;
            }
            tx_clone.send(req)
        });
    }
}

impl From<&SwitchOverRequest> for SwitchOverSpec {
    fn from(req: &SwitchOverRequest) -> Self {
        let op = OperationState::new(req.stage().into(), None);
        Self {
            callback_uri: req.callback_uri,
            node_name: req.node_name.clone(),
            volume: req.volume_id.clone(),
            operation: Some(op),
            timestamp: req.timestamp,
            existing_nqn: req.existing_nqn.clone(),
            new_path: req.new_path.clone(),
            retry_count: req.retry_count,
            reuse_existing: req.reuse_existing,
            publish_context: req.publish_context.clone(),
        }
    }
}

impl From<&SwitchOverSpec> for SwitchOverRequest {
    fn from(req: &SwitchOverSpec) -> Self {
        let mut stage = Stage::Init;
        if let Some(op) = req.operation() {
            stage = op.into();
        }

        Self {
            callback_uri: req.callback_uri,
            volume_id: req.volume.clone(),
            stage: SwitchOverStage::new(stage),
            node_name: req.node_name.clone(),
            timestamp: req.timestamp,
            existing_nqn: req.existing_nqn.clone(),
            new_path: req.new_path.clone(),
            retry_count: req.retry_count,
            reuse_existing: req.reuse_existing,
            publish_context: req.publish_context.clone(),
            fast_exhaustion_retry: true,
        }
    }
}

impl From<Stage> for Operation {
    fn from(stage: Stage) -> Self {
        match stage {
            Stage::Init => Operation::Init,
            Stage::RepublishVolume => Operation::RepublishVolume,
            Stage::ReplacePath => Operation::ReplacePath,
            Stage::DeleteTarget => Operation::DeleteTarget,
            Stage::Successful => Operation::Successful,
            Stage::Errored => Operation::Errored("".to_string()),
        }
    }
}

impl From<Operation> for Stage {
    fn from(op: Operation) -> Self {
        match op {
            Operation::Init => Stage::Init,
            Operation::RepublishVolume => Stage::RepublishVolume,
            Operation::ReplacePath => Stage::ReplacePath,
            Operation::DeleteTarget => Stage::DeleteTarget,
            Operation::Successful => Stage::Successful,
            Operation::Errored(_) => Stage::Errored,
        }
    }
}

impl Event for SwitchOverRequest {
    fn event(&self, action: EventAction) -> EventMessage {
        let node_id = &self.node_name;
        let status = match self.stage() {
            Stage::Init => SwitchOverStatus::SwitchOverStarted,
            Stage::Successful => SwitchOverStatus::SwitchOverCompleted,
            Stage::Errored => SwitchOverStatus::SwitchOverFailed,
            _ => SwitchOverStatus::UnknownSwitchOverStatus,
        };

        let event_source = EventSource::new(node_id.to_string()).with_switch_over_data(
            status,
            self.timestamp,
            &self.existing_nqn,
            self.new_path.clone(),
            self.retry_count,
        );
        EventMessage {
            category: EventCategory::HighAvailability as i32,
            action: action as i32,
            target: self.volume_id.to_string(),
            metadata: Some(EventMeta::from_source(event_source)),
        }
    }
}

#[test]
fn switchover_ord() {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    let sock1: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080);
    let vol1 = VolumeId::try_from("ec4e66fd-3b33-4439-b504-d49aba53da26").unwrap();
    let sw1 = SwitchOverRequest::new(sock1, vol1.clone(), "nn".into(), "nw".to_string());
    let sw2 = SwitchOverRequest::new(sock1, vol1.clone(), "nn".into(), "nw".to_string());
    sw2.set_stage(Stage::Errored);
    let sw3 = SwitchOverRequest::new(sock1, vol1.clone(), "nn".into(), "nw".to_string());
    sw3.set_stage(Stage::Successful);
    let sw4 = SwitchOverRequest::new(sock1, vol1.clone(), "nn".into(), "nw".to_string());
    sw4.set_stage(Stage::RepublishVolume);
    let sw5 = SwitchOverRequest::new(sock1, vol1, "nn".into(), "nw".to_string());
    sw5.set_stage(Stage::RepublishVolume);
    let mut test_vec = vec![
        sw3.clone(),
        sw2.clone(),
        sw1.clone(),
        sw5.clone(),
        sw4.clone(),
    ];
    assert_eq!(
        test_vec,
        vec![
            sw3.clone(),
            sw2.clone(),
            sw1.clone(),
            sw5.clone(),
            sw4.clone(),
        ]
    );
    test_vec.sort();
    assert_eq!(test_vec, vec![sw1, sw4, sw5, sw3, sw2]);
}
