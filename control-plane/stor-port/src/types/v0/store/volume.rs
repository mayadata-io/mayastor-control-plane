//! Definition of volume types that can be saved to the persistent store.

use crate::{
    types::v0::{
        openapi::models,
        store::{
            definitions::{ObjectKey, StorableObject, StorableObjectType},
            AsOperationSequencer, OperationSequence, SpecStatus, SpecTransaction,
        },
        transport::{
            self, AffinityGroup, CreateVolume, HostNqn, NexusId, NexusNvmfConfig, NodeId,
            ReplicaId, SnapshotId, Topology, VolumeId, VolumeLabels, VolumePolicy, VolumeProperty,
            VolumeShareProtocol, VolumeStatus,
        },
    },
    IntoOption,
};

use pstor::ApiVersion;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Key used by the store to uniquely identify a VolumeState structure.
pub struct VolumeStateKey(VolumeId);

impl From<&VolumeId> for VolumeStateKey {
    fn from(id: &VolumeId) -> Self {
        Self(id.clone())
    }
}

impl ObjectKey for VolumeStateKey {
    type Kind = StorableObjectType;

    fn version(&self) -> ApiVersion {
        ApiVersion::V0
    }

    fn key_type(&self) -> StorableObjectType {
        StorableObjectType::VolumeState
    }

    fn key_uuid(&self) -> String {
        self.0.to_string()
    }
}

/// Frontend nodes configuration.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct FrontendConfig {
    host_acl: Vec<InitiatorAC>,
}
impl FrontendConfig {
    /// Create new `Self` based on the host access list.
    pub fn from_acls(host_acl: Vec<InitiatorAC>) -> Self {
        Self { host_acl }
    }
    /// Check if the nodename is allowed.
    pub fn nodename_allowed(&self, nodename: &str) -> bool {
        self.host_acl.is_empty() || self.host_acl.iter().any(|n| n.node_name() == nodename)
    }
    /// Get the node names of the acl.
    pub fn node_names(&self) -> Vec<String> {
        self.host_acl.iter().map(|h| h.node_name.clone()).collect()
    }
    /// Get the first node name.
    pub fn node_name(&self) -> Option<String> {
        self.host_acl.first().map(|h| h.node_name.clone())
    }
    /// Get the hostnqn's for the acl.
    pub fn node_nqns(&self) -> Vec<HostNqn> {
        self.host_acl.iter().map(|h| h.node_nqn.clone()).collect()
    }
    /// Get the frontend nodes information reference.
    pub fn nodes_info(&self) -> &Vec<InitiatorAC> {
        &self.host_acl
    }
}

/// Volume Frontend
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct InitiatorAC {
    /// The nodename where front-end IO will be sent from.
    node_name: String,
    /// The nvme nqn of the front-end node.
    node_nqn: HostNqn,
}
impl InitiatorAC {
    /// Get a new `Self` with the given parameters.
    pub fn new(node_name: String, node_nqn: HostNqn) -> Self {
        Self {
            node_name,
            node_nqn,
        }
    }
    /// Get the nodename of the front-end initiator.
    pub fn node_name(&self) -> &str {
        &self.node_name
    }
    /// Get the hostnqn of the front-end initiator.
    pub fn node_nqn(&self) -> &HostNqn {
        &self.node_nqn
    }
}

/// Volume Target (node and nexus)
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct VolumeTarget {
    /// The node where front-end IO will be sent to.
    node: NodeId,
    /// The identification of the nexus where the frontend-IO will be sent to.
    nexus: NexusId,
    /// The protocol to use on the target.
    protocol: Option<VolumeShareProtocol>,
}
impl VolumeTarget {
    /// Create a new `Self` based on the given parameters.
    pub fn new(node: NodeId, nexus: NexusId, protocol: Option<VolumeShareProtocol>) -> Self {
        Self {
            node,
            nexus,
            protocol,
        }
    }
    /// Get a reference to the node identification.
    pub fn node(&self) -> &NodeId {
        &self.node
    }
    /// Get a reference to the nexus identification.
    pub fn nexus(&self) -> &NexusId {
        &self.nexus
    }
    /// Get a reference to the volume protocol.
    pub fn protocol(&self) -> Option<&VolumeShareProtocol> {
        self.protocol.as_ref()
    }
}
impl From<&InitiatorAC> for models::NodeAccessInfo {
    fn from(node: &InitiatorAC) -> Self {
        models::NodeAccessInfo::new_all(node.node_name(), node.node_nqn().to_string())
    }
}
impl From<&TargetConfig> for models::VolumeTarget {
    fn from(src: &TargetConfig) -> Self {
        let node_names = &src.frontend.nodes_info();
        let node_names = if node_names.is_empty() {
            None
        } else {
            Some(
                node_names
                    .iter()
                    .map(|n| n.into())
                    .collect::<Vec<models::NodeAccessInfo>>(),
            )
        };
        Self::new_all(
            src.target.node.clone(),
            src.target.protocol.into_opt(),
            node_names,
        )
    }
}

/// User specification of a volume.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct VolumeSpec {
    /// Volume Id.
    pub uuid: VolumeId,
    /// Size that the volume should be.
    pub size: u64,
    /// Volume labels.
    pub labels: Option<VolumeLabels>,
    /// Number of children the volume should have.
    pub num_replicas: u8,
    /// Status that the volume should eventually achieve.
    pub status: VolumeSpecStatus,
    /// Volume policy.
    pub policy: VolumePolicy,
    /// Replica placement topology for the volume creation only.
    pub topology: Option<Topology>,
    /// Update of the state in progress.
    #[serde(skip)]
    pub sequencer: OperationSequence,
    /// Id of the last Nexus used by the volume.
    pub last_nexus_id: Option<NexusId>,
    /// Record of the operation in progress.
    pub operation: Option<VolumeOperationState>,
    /// Flag indicating whether the volume should be thin provisioned.
    #[serde(default)]
    pub thin: bool,
    /// Volume Content Source i.e the snapshot or a volume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_source: Option<VolumeContentSource>,
    /// Last used Target or current Configuration.
    #[serde(default, rename = "target")]
    pub target_config: Option<TargetConfig>,
    /// The publish context of the volume.
    #[serde(default)]
    pub publish_context: Option<HashMap<String, String>>,
    /// Affinity Group related information.
    #[serde(default)]
    pub affinity_group: Option<AffinityGroup>,
    /// Number of snapshots taken on this volume.
    #[serde(skip)]
    pub num_snapshots: u32,
    /// Volume metadata information.
    #[serde(default, skip_serializing_if = "super::is_default")]
    pub metadata: VolumeMetadata,
    /// Max snapshots limit per volume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_snapshots: Option<u32>,
}

/// Volume Content Source i.e the snapshot or a volume.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum VolumeContentSource {
    Snapshot(SnapshotId, VolumeId),
}

impl VolumeContentSource {
    /// Create a new `VolumeContentSource::Snapshot` from the params.
    pub fn new_snapshot_source(snapshot: SnapshotId, snap_source_vol: VolumeId) -> Self {
        Self::Snapshot(snapshot, snap_source_vol)
    }
}

/// Volume meta information.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct VolumeMetadata {
    /// Persisted metadata information.
    persisted: VolumePersistedMetadata,
    /// Runtime information, useful to quick checks without having to read out from PSTOR
    /// or any other control-plane related registry.
    #[serde(skip)]
    runtime: VolumeRuntimeMetadata,
}
impl VolumeMetadata {
    /// Create a new `Self` from the given parameters.
    pub fn new(as_thin: Option<bool>) -> Self {
        Self {
            persisted: VolumePersistedMetadata {
                snapshot_as_thin: as_thin,
            },
            runtime: Default::default(),
        }
    }
    /// Insert snapshot in the list.
    fn insert_snapshot(&mut self, snapshot: SnapshotId) {
        self.runtime.snapshots.insert(snapshot);
        // we become thin provisioned!
        self.persisted.snapshot_as_thin = Some(true);
    }
    /// Number of snapshots taken on this volume.
    pub fn num_snapshots(&self) -> usize {
        self.runtime.snapshots.len()
    }
    /// Check if there's any snapshot.
    pub fn has_snapshots(&self) -> bool {
        self.runtime.has_snapshots()
    }
}

/// Volume meta information.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct VolumePersistedMetadata {
    /// Volume becomes thin if a snapshot has been created for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot_as_thin: Option<bool>,
}

/// Runtime volume information.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VolumeRuntimeMetadata {
    /// Runtime list of all volume snapshots.
    snapshots: super::snapshots::volume::VolumeSnapshotList,
}
impl VolumeRuntimeMetadata {
    /// Check if there's any snapshot.
    pub fn has_snapshots(&self) -> bool {
        !self.snapshots.is_empty()
    }
}

/// The volume's Nvmf Configuration.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct TargetConfig {
    /// Back-compat target parameters.
    #[serde(flatten)]
    target: VolumeTarget,
    /// Indicates whether the target is active.
    /// We keep this config even if the target is no longer active so we can inspect
    /// what the previous config was like.
    #[serde(default = "default_active")]
    active: bool,
    /// The nexus nvmf configuration.
    #[serde(default)]
    config: NexusNvmfConfig,
    /// Config of frontend-nodes where IO will be sent from.
    #[serde(default)]
    frontend: FrontendConfig,
}

/// Default value for the active field in TargetConfig.
fn default_active() -> bool {
    true
}

impl TargetConfig {
    /// Get the uuid of the target.
    pub fn new(target: VolumeTarget, config: NexusNvmfConfig, frontend: FrontendConfig) -> Self {
        Self {
            target,
            active: true,
            config,
            frontend,
        }
    }

    /// Get the last target configuration.
    /// # Note: It may or may not the the current active target.
    pub fn target(&self) -> &VolumeTarget {
        &self.target
    }
    /// Get the active target.
    pub fn active_target(&self) -> Option<&VolumeTarget> {
        match self.active {
            true => Some(&self.target),
            false => None,
        }
    }
    /// Get the initiators.
    pub fn frontend(&self) -> &FrontendConfig {
        &self.frontend
    }
    /// Get the uuid of the target.
    pub fn uuid(&self) -> &NexusId {
        &self.target.nexus
    }
    /// Get the target's nvmf configuration.
    pub fn config(&self) -> &NexusNvmfConfig {
        &self.config
    }
}

impl AsOperationSequencer for VolumeSpec {
    fn as_ref(&self) -> &OperationSequence {
        &self.sequencer
    }

    fn as_mut(&mut self) -> &mut OperationSequence {
        &mut self.sequencer
    }
}

impl AsOperationSequencer for AffinityGroupSpec {
    fn as_ref(&self) -> &OperationSequence {
        &self.sequencer
    }

    fn as_mut(&mut self) -> &mut OperationSequence {
        &mut self.sequencer
    }
}

impl VolumeSpec {
    /// Insert snapshot in the list.
    pub fn insert_snapshot(&mut self, snapshot: &SnapshotId) {
        self.metadata.insert_snapshot(snapshot.clone());
    }
    /// Check if there's any snapshot.
    pub fn has_snapshots(&self) -> bool {
        self.metadata.runtime.has_snapshots()
    }
    /// Check if the volume behaves as thin provisioned.
    /// This can happen when a volume has snapshots.
    pub fn as_thin(&self) -> bool {
        self.thin || self.metadata.persisted.snapshot_as_thin == Some(true)
    }
    /// Check if the volume has had a snapshot ond if so if it behaves as thin provisioned.
    pub fn snapshot_as_thin(&self) -> Option<bool> {
        self.metadata.persisted.snapshot_as_thin
    }
    /// Get the currently active target.
    pub fn target(&self) -> Option<&VolumeTarget> {
        self.target_config
            .as_ref()
            .filter(|t| t.active)
            .map(|t| &t.target)
    }
    /// Get the currently active target uuid.
    pub fn target_uuid(&self) -> Option<&NexusId> {
        self.target().map(|t| &t.nexus)
    }
    /// Get the target.
    pub fn target_mut(&mut self) -> Option<&mut VolumeTarget> {
        self.target_config.as_mut().map(|t| &mut t.target)
    }
    /// Explicitly selected allowed_nodes.
    pub fn allowed_nodes(&self) -> Vec<NodeId> {
        match &self.topology {
            None => vec![],
            Some(t) => t
                .explicit()
                .map(|t| t.allowed_nodes.clone())
                .unwrap_or_default(),
        }
    }
    /// Desired volume replica count if during `SetReplica` operation
    /// or otherwise the current num_replicas.
    pub fn desired_num_replicas(&self) -> u8 {
        match &self.operation {
            Some(operation) => match operation.operation {
                VolumeOperation::SetReplica(count) => count,
                _ => self.num_replicas,
            },
            _ => self.num_replicas,
        }
    }
    /// Get the last known target configuration.
    /// # Warning: the target may no longer be active.
    /// To get the current active target configuration use `VolumeSpec::active_config`.
    pub fn config(&self) -> &Option<TargetConfig> {
        &self.target_config
    }
    /// Get the current target configuration.
    pub fn active_config(&self) -> Option<&TargetConfig> {
        self.target_config.as_ref().filter(|t| t.active)
    }
    /// Deactivate the current target but keep it's information around.
    pub fn deactivate_target(&mut self) {
        if let Some(cfg) = self.target_config.as_mut() {
            cfg.active = false;
        }
    }
    /// Get the health info key which is used to retrieve the volume's replica health information.
    pub fn health_info_id(&self) -> Option<&NexusId> {
        if let Some(id) = &self.last_nexus_id {
            return Some(id);
        }
        self.target_config.as_ref().map(|c| &c.target.nexus)
    }
    /// Set the content source.
    pub fn set_content_source(&mut self, content_source: Option<VolumeContentSource>) {
        self.content_source = content_source;
    }
}

/// Operation State for a Volume resource.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct VolumeOperationState {
    /// Record of the operation.
    pub operation: VolumeOperation,
    /// Result of the operation.
    pub result: Option<bool>,
}

impl From<VolumeOperationState> for models::VolumeSpecOperation {
    fn from(src: VolumeOperationState) -> Self {
        models::VolumeSpecOperation::new_all(src.operation, src.result)
    }
}

impl SpecTransaction<VolumeOperation> for VolumeSpec {
    fn has_pending_op(&self) -> bool {
        self.operation.is_some()
    }

    fn commit_op(&mut self) {
        if let Some(op) = self.operation.take() {
            match op.operation {
                VolumeOperation::Destroy => {
                    self.status = SpecStatus::Deleted;
                }
                VolumeOperation::Create => {
                    self.status = SpecStatus::Created(transport::VolumeStatus::Online);
                }
                VolumeOperation::Share(share) => {
                    if let Some(target) = self.target_mut() {
                        target.protocol = share.into();
                    }
                }
                VolumeOperation::Unshare => {
                    if let Some(target) = self.target_mut() {
                        target.protocol = None
                    }
                }
                VolumeOperation::SetReplica(count) => self.num_replicas = count,
                VolumeOperation::RemoveUnusedReplica(_) => {}
                VolumeOperation::PublishOld(args) => {
                    let (node, nexus, protocol) = (args.node, args.nexus, args.protocol);
                    let target = VolumeTarget::new(node, nexus, protocol);
                    self.last_nexus_id = None;
                    self.target_config = Some(TargetConfig::new(
                        target,
                        NexusNvmfConfig::default().with_no_resv(),
                        Default::default(),
                    ));
                }
                VolumeOperation::Publish(args) => {
                    self.last_nexus_id = None;
                    self.target_config = Some(args.config);
                    self.publish_context = Some(args.publish_context);
                }
                VolumeOperation::Republish(args) => {
                    self.last_nexus_id = None;
                    self.target_config = Some(args.config);
                }
                VolumeOperation::Unpublish => {
                    self.deactivate_target();
                }
                VolumeOperation::CreateSnapshot(snapshot) => {
                    self.metadata.insert_snapshot(snapshot);
                }
                VolumeOperation::DestroySnapshot(snapshot) => {
                    self.metadata.runtime.snapshots.remove(&snapshot);
                }
                VolumeOperation::Resize(size) => {
                    self.size = size;
                }
                VolumeOperation::SetVolumeProperty(property) => match property {
                    VolumeProperty::MaxSnapshots(max_snapshots) => {
                        self.max_snapshots = Some(max_snapshots);
                    }
                },
            }
        }
        self.clear_op();
    }

    fn clear_op(&mut self) {
        self.operation = None;
    }

    fn start_op(&mut self, operation: VolumeOperation) {
        self.operation = Some(VolumeOperationState {
            operation,
            result: None,
        })
    }

    fn set_op_result(&mut self, result: bool) {
        if let Some(op) = &mut self.operation {
            op.result = Some(result);
        }
    }

    fn log_op(&self, operation: &VolumeOperation) -> (bool, bool) {
        match operation {
            VolumeOperation::CreateSnapshot(_) => {
                // first snapshot created, now we're becoming thin provisioned, we need to store it
                if !self.thin && !self.has_snapshots() {
                    (
                        false,
                        self.metadata.persisted.snapshot_as_thin != Some(true),
                    )
                } else {
                    (false, false)
                }
            }
            VolumeOperation::DestroySnapshot(_) => (false, false),
            VolumeOperation::RemoveUnusedReplica(_) => (false, false),
            _ => (true, true),
        }
    }

    fn pending_op(&self) -> Option<&VolumeOperation> {
        self.operation.as_ref().map(|o| &o.operation)
    }
}

/// Available Volume Operations.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum VolumeOperation {
    Create,
    Destroy,
    Share(VolumeShareProtocol),
    Unshare,
    SetReplica(u8),
    #[serde(rename = "Publish")]
    PublishOld(OldPublishOperation),
    #[serde(rename = "Publish2")]
    Publish(PublishOperation),
    Republish(RepublishOperation),
    Unpublish,
    RemoveUnusedReplica(ReplicaId),
    CreateSnapshot(SnapshotId),
    DestroySnapshot(SnapshotId),
    Resize(u64),
    SetVolumeProperty(VolumeProperty),
}

#[test]
fn volume_op_deserializer() {
    #[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
    struct TestSpec {
        op: VolumeOperation,
    }
    #[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
    struct Test<'a> {
        input: &'a str,
        expected: VolumeOperation,
    }
    let tests: Vec<Test> = vec![Test {
        input: r#"{"op":{"Publish":["4ffe7e43-46dd-4912-9d0f-6c9844fa7c6e","4ffe7e43-46dd-4912-9d0f-6c9844fa7c6e",null]}}"#,
        expected: VolumeOperation::PublishOld(OldPublishOperation {
            node: "4ffe7e43-46dd-4912-9d0f-6c9844fa7c6e".into(),
            nexus: "4ffe7e43-46dd-4912-9d0f-6c9844fa7c6e".try_into().unwrap(),
            protocol: None,
        }),
    }];

    for test in &tests {
        println!("{}", serde_json::to_string(&test.expected).unwrap());
        let spec: TestSpec = serde_json::from_str(test.input).unwrap();
        assert_eq!(test.expected, spec.op);
    }
}

/// The `PublishOperation` which is easier to manage and update.
#[derive(serde_tuple::Serialize_tuple, serde_tuple::Deserialize_tuple, Debug, Clone, PartialEq)]
pub struct OldPublishOperation {
    node: NodeId,
    nexus: NexusId,
    protocol: Option<VolumeShareProtocol>,
}

/// The `PublishOperation` which is easier to manage and update.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PublishOperation {
    config: TargetConfig,
    publish_context: HashMap<String, String>,
}
impl PublishOperation {
    /// Return new `Self` from the given parameters.
    pub fn new(config: TargetConfig, publish_context: HashMap<String, String>) -> Self {
        Self {
            config,
            publish_context,
        }
    }
    /// Get the share protocol.
    pub fn protocol(&self) -> Option<VolumeShareProtocol> {
        self.config.target.protocol
    }
    /// Get the publish context.
    pub fn publish_context(&self) -> HashMap<String, String> {
        self.publish_context.clone()
    }
}

/// Volume Republish Operation parameters.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RepublishOperation {
    config: TargetConfig,
}
impl RepublishOperation {
    /// Return new `Self` from the given parameters.
    pub fn new(config: TargetConfig) -> Self {
        Self { config }
    }
    /// Get the share protocol.
    pub fn protocol(&self) -> VolumeShareProtocol {
        self.config.target.protocol.unwrap_or_default()
    }
}

impl From<VolumeOperation> for models::volume_spec_operation::Operation {
    fn from(src: VolumeOperation) -> Self {
        match src {
            VolumeOperation::Create => models::volume_spec_operation::Operation::Create,
            VolumeOperation::Destroy => models::volume_spec_operation::Operation::Destroy,
            VolumeOperation::Share(_) => models::volume_spec_operation::Operation::Share,
            VolumeOperation::Unshare => models::volume_spec_operation::Operation::Unshare,
            VolumeOperation::SetReplica(_) => models::volume_spec_operation::Operation::SetReplica,
            VolumeOperation::PublishOld(_) => models::volume_spec_operation::Operation::Publish,
            VolumeOperation::Publish(_) => models::volume_spec_operation::Operation::Publish,
            VolumeOperation::Republish(_) => models::volume_spec_operation::Operation::Republish,
            VolumeOperation::Unpublish => models::volume_spec_operation::Operation::Unpublish,
            VolumeOperation::RemoveUnusedReplica(_) => {
                models::volume_spec_operation::Operation::RemoveUnusedReplica
            }
            VolumeOperation::CreateSnapshot(_) => {
                models::volume_spec_operation::Operation::CreateSnapshot
            }
            VolumeOperation::DestroySnapshot(_) => {
                models::volume_spec_operation::Operation::DestroySnapshot
            }
            VolumeOperation::SetVolumeProperty(_) => {
                models::volume_spec_operation::Operation::SetVolumeProperty
            }
            VolumeOperation::Resize(_) => models::volume_spec_operation::Operation::Resize,
        }
    }
}

/// Key used by the store to uniquely identify a VolumeSpec structure.
pub struct VolumeSpecKey(VolumeId);

impl From<&VolumeId> for VolumeSpecKey {
    fn from(id: &VolumeId) -> Self {
        Self(id.clone())
    }
}

impl ObjectKey for VolumeSpecKey {
    type Kind = StorableObjectType;

    fn version(&self) -> ApiVersion {
        ApiVersion::V0
    }

    fn key_type(&self) -> StorableObjectType {
        StorableObjectType::VolumeSpec
    }

    fn key_uuid(&self) -> String {
        self.0.to_string()
    }
}

impl StorableObject for VolumeSpec {
    type Key = VolumeSpecKey;

    fn key(&self) -> Self::Key {
        VolumeSpecKey(self.uuid.clone())
    }
}

/// State of the Volume Spec.
pub type VolumeSpecStatus = SpecStatus<transport::VolumeStatus>;

impl From<&CreateVolume> for VolumeSpec {
    fn from(request: &CreateVolume) -> Self {
        Self {
            uuid: request.uuid.clone(),
            size: request.size,
            labels: request.labels.clone(),
            num_replicas: request.replicas as u8,
            status: VolumeSpecStatus::Creating,
            policy: request.policy.clone(),
            topology: request.topology.clone(),
            sequencer: OperationSequence::new(),
            last_nexus_id: None,
            operation: None,
            thin: request.thin,
            target_config: None,
            publish_context: None,
            affinity_group: request.affinity_group.clone(),
            max_snapshots: request.max_snapshots,
            ..Default::default()
        }
    }
}
impl PartialEq<CreateVolume> for VolumeSpec {
    fn eq(&self, other: &CreateVolume) -> bool {
        let mut other = VolumeSpec::from(other);
        other.status = self.status.clone();
        other.sequencer = self.sequencer.clone();
        other.content_source = self.content_source.clone();
        &other == self
    }
}
impl From<&VolumeSpec> for transport::VolumeState {
    fn from(spec: &VolumeSpec) -> Self {
        Self {
            uuid: spec.uuid.clone(),
            size: spec.size,
            usage: None,
            status: transport::VolumeStatus::Unknown,
            target: None,
            replica_topology: HashMap::new(),
            health: None,
        }
    }
}
impl PartialEq<transport::VolumeState> for VolumeSpec {
    fn eq(&self, other: &transport::VolumeState) -> bool {
        match self.target() {
            None => other.target_node().flatten().is_none(),
            Some(target) => {
                target.protocol == other.target_protocol()
                    && Some(&target.node) == other.target_node().flatten().as_ref()
                    && other.status == VolumeStatus::Online
            }
        }
    }
}

impl From<VolumeSpec> for models::VolumeSpec {
    fn from(src: VolumeSpec) -> Self {
        let target_cfg = src.active_config().into_opt();
        Self::new_all(
            src.labels,
            src.num_replicas,
            src.operation.into_opt(),
            src.size,
            src.status,
            target_cfg,
            src.uuid,
            src.topology.into_opt(),
            src.policy,
            src.thin,
            src.metadata.persisted.snapshot_as_thin,
            src.affinity_group.into_opt(),
            src.content_source.into_opt(),
            src.num_snapshots,
            src.max_snapshots,
        )
    }
}

impl From<VolumeContentSource> for models::VolumeContentSource {
    fn from(value: VolumeContentSource) -> Self {
        match value {
            VolumeContentSource::Snapshot(snap_id, vol_id) => {
                Self::snapshot(models::SnapshotAsSource::new_all(snap_id, vol_id))
            }
        }
    }
}

impl From<models::VolumeContentSource> for VolumeContentSource {
    fn from(value: models::VolumeContentSource) -> Self {
        match value {
            models::VolumeContentSource::snapshot(snap_source) => {
                Self::Snapshot(snap_source.snapshot.into(), snap_source.volume.into())
            }
        }
    }
}

/// AffinityGroupId type.
pub type AffinityGroupId = String;

/// In memory representation of Affinity Group.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct AffinityGroupSpec {
    /// Name of the Affinity Group.
    id: AffinityGroupId,
    /// List of ids of volumes that belong to the Affinity Group.
    volumes: Vec<VolumeId>,
    /// The operation sequence resource is in.
    #[serde(skip)]
    sequencer: OperationSequence,
}

impl AffinityGroupSpec {
    /// Create a new AffinityGroupSpec.
    pub fn new(id: AffinityGroupId, volumes: Vec<VolumeId>) -> Self {
        Self {
            id,
            volumes,
            sequencer: OperationSequence::new(),
        }
    }
    /// Name of the Affinity Group.
    pub fn id(&self) -> &AffinityGroupId {
        &self.id
    }
    /// List of volumes in Affinity Group.
    pub fn volumes(&self) -> &Vec<VolumeId> {
        &self.volumes
    }
    /// Add a new volume to the list of volumes.
    pub fn append(&mut self, id: VolumeId) {
        if !self.volumes.contains(&id) {
            self.volumes.push(id)
        }
    }
    /// Add a new volume to the list of volumes.
    pub fn remove(&mut self, id: &VolumeId) {
        self.volumes.retain(|vol| vol != id)
    }
    /// Check if the Affinity Group has any more volumes.
    pub fn is_empty(&self) -> bool {
        self.volumes.is_empty()
    }
}
