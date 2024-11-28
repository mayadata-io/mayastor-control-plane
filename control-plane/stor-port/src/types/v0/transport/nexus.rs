use super::*;

use crate::{
    transport_api::{ReplyError, ResourceKind},
    types::v0::store::{
        definitions::ObjectKey, nexus_child::NexusChild, nexus_persistence::NexusInfoKey,
    },
    IntoVec,
};
use serde::{Deserialize, Serialize};
use std::{convert::TryFrom, fmt::Debug, ops::RangeInclusive, time::SystemTime};
use strum_macros::{Display, EnumString};

/// Volume Nexuses
///
/// Get all the nexuses with a filter selection
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct GetNexuses {
    /// Filter request
    pub filter: Filter,
}

/// Nexus information
#[derive(Serialize, Deserialize, Default, Debug, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Nexus {
    /// id of the io-engine instance
    pub node: NodeId,
    /// name of the nexus
    pub name: String,
    /// uuid of the nexus
    pub uuid: NexusId,
    /// size of the volume in bytes
    pub size: u64,
    /// current status of the nexus
    pub status: NexusStatus,
    /// array of children
    pub children: Vec<Child>,
    /// URI of the device for the volume (missing if not published).
    /// Missing property and empty string are treated the same.
    pub device_uri: String,
    /// number of active rebuild jobs
    pub rebuilds: u32,
    /// protocol used for exposing the nexus
    pub share: Protocol,
    /// host nqn's allowed to connect to the target.
    pub allowed_hosts: Vec<HostNqn>,
}
impl Nexus {
    /// Check if the nexus contains the provided `ChildUri`.
    pub fn contains_child(&self, uri: &ChildUri) -> bool {
        self.children.iter().any(|c| &c.uri == uri)
    }
    /// Check if the nexus contains the provided `ChildUri`.
    pub fn child(&self, uri: &str) -> Option<&Child> {
        self.children.iter().find(|c| c.uri.as_str() == uri)
    }
    /// Add query parameter to the Uri.
    pub fn with_query(mut self, name: &str, value: &str) -> Self {
        self.device_uri = add_query(self.device_uri, name, value);
        self
    }
    /// Check if the nexus is able to process IO.
    pub fn io_online(&self) -> bool {
        matches!(self.status, NexusStatus::Online | NexusStatus::Degraded)
    }
}

impl From<Nexus> for models::Nexus {
    fn from(src: Nexus) -> Self {
        models::Nexus::new(
            src.children,
            src.device_uri,
            src.node,
            src.rebuilds,
            src.share,
            src.size,
            src.status,
            src.uuid,
        )
    }
}

rpc_impl_string_uuid!(NexusId, "UUID of a nexus");

/// A request for creating snapshot of a nexus, which essentially means a snapshot
/// of all(or selected) replicas associated with that nexus.
#[derive(Debug)]
pub struct CreateNexusSnapshot {
    params: SnapshotParameters<NexusId>,
    replica_desc: Vec<CreateNexusSnapReplDescr>,
}

impl CreateNexusSnapshot {
    /// Create a new request.
    pub fn new(
        params: SnapshotParameters<NexusId>,
        replica_desc: Vec<CreateNexusSnapReplDescr>,
    ) -> Self {
        Self {
            params,
            replica_desc,
        }
    }
    /// Get a reference to the transaction id.
    pub fn params(&self) -> &SnapshotParameters<NexusId> {
        &self.params
    }
    /// Get a reference to the nexus uuid.
    pub fn nexus(&self) -> &NexusId {
        self.params.target()
    }
    /// Get a reference to the replica descriptor.
    pub fn replica_desc(&self) -> &Vec<CreateNexusSnapReplDescr> {
        &self.replica_desc
    }
}

/// A descriptor that specifies the replicas which need to taken snapshot
/// of specifically.
#[derive(Clone, Debug)]
pub struct CreateNexusSnapReplDescr {
    /// UUID of the replica involved in snapshot operation.
    pub replica: ReplicaId,
    /// UUID input for the snapshot to be created.
    pub snap_uuid: SnapshotId,
}
impl CreateNexusSnapReplDescr {
    /// Return a new `Self` from the given parameters.
    pub fn new(replica: &ReplicaId, snap_uuid: SnapshotId) -> Self {
        Self {
            replica: replica.clone(),
            snap_uuid,
        }
    }
}

/// Nexus State information
#[derive(Serialize, Deserialize, Debug, Clone, EnumString, Display, Eq, PartialEq)]
pub enum NexusStatus {
    /// Default Unknown state.
    Unknown,
    /// Healthy and working.
    Online,
    /// Not healthy but is able to serve IO (i.e. rebuild is in progress).
    Degraded,
    /// Broken and unable to serve IO.
    Faulted,
    /// Shutdown state, i.e. blocked from serving IO.
    Shutdown,
    /// Shutdown in progress: not able to serve I/O.
    ShuttingDown,
}
impl Default for NexusStatus {
    fn default() -> Self {
        Self::Unknown
    }
}
impl From<NexusStatus> for models::NexusState {
    fn from(src: NexusStatus) -> Self {
        match src {
            NexusStatus::Online => Self::Online,
            NexusStatus::Degraded => Self::Degraded,
            NexusStatus::Faulted => Self::Faulted,
            NexusStatus::Unknown => Self::Unknown,
            NexusStatus::Shutdown => Self::Shutdown,
            NexusStatus::ShuttingDown => Self::ShuttingDown,
        }
    }
}

/// The protocol used to share the nexus.
#[derive(Serialize, Deserialize, Debug, Copy, Clone, EnumString, Display, Eq, PartialEq)]
#[strum(serialize_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub enum NexusShareProtocol {
    /// shared as NVMe-oF TCP
    Nvmf = 1,
    /// shared as iSCSI
    Iscsi = 2,
}

impl std::cmp::PartialEq<Protocol> for NexusShareProtocol {
    fn eq(&self, other: &Protocol) -> bool {
        &Protocol::from(*self) == other
    }
}
impl Default for NexusShareProtocol {
    fn default() -> Self {
        Self::Nvmf
    }
}
impl From<NexusShareProtocol> for Protocol {
    fn from(src: NexusShareProtocol) -> Self {
        match src {
            NexusShareProtocol::Nvmf => Self::Nvmf,
            NexusShareProtocol::Iscsi => Self::Iscsi,
        }
    }
}
impl From<models::NexusShareProtocol> for NexusShareProtocol {
    fn from(src: models::NexusShareProtocol) -> Self {
        match src {
            models::NexusShareProtocol::Nvmf => Self::Nvmf,
            models::NexusShareProtocol::Iscsi => Self::Iscsi,
        }
    }
}
impl TryFrom<Protocol> for NexusShareProtocol {
    type Error = String;

    fn try_from(value: Protocol) -> Result<Self, Self::Error> {
        match value {
            Protocol::None => Err(format!("Invalid protocol: {value:?}")),
            Protocol::Nvmf => Ok(Self::Nvmf),
            Protocol::Iscsi => Ok(Self::Iscsi),
            Protocol::Nbd => Err(format!("Invalid protocol: {value:?}")),
        }
    }
}

/// Create Nexus Request
#[derive(Serialize, Deserialize, Default, Debug, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CreateNexus {
    /// id of the io-engine instance
    pub node: NodeId,
    /// the nexus uuid will be set to this
    pub uuid: NexusId,
    /// size of the device in bytes
    pub size: u64,
    /// replica can be iscsi and nvmf remote targets or a local spdk bdev
    /// (i.e. bdev:///name-of-the-bdev).
    ///
    /// uris to the targets we connect to
    pub children: Vec<NexusChild>,
    /// Managed by our control plane
    pub managed: bool,
    /// Volume which owns this nexus, if any
    pub owner: Option<VolumeId>,
    /// Nexus Nvmf Configuration
    pub config: Option<NexusNvmfConfig>,
}

/// A request to resize a Nexus.
#[derive(Serialize, Deserialize, Default, Debug, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResizeNexus {
    /// Id of the io-engine instance.
    pub node: NodeId,
    /// Uuid of the nexus to be resized.
    pub uuid: NexusId,
    /// Requested size for the nexus.
    pub requested_size: u64,
}

impl ResizeNexus {
    /// Return new `Self` from the given parameters.
    pub fn new(node_id: &NodeId, uuid: &NexusId, requested_size: u64) -> Self {
        Self {
            node: node_id.clone(),
            uuid: uuid.clone(),
            requested_size,
        }
    }
}

/// NVMe reservation types.
#[derive(Serialize, Deserialize, Debug, Copy, Clone, Eq, PartialEq)]
pub enum NvmeReservation {
    Reserved = 0,
    WriteExclusive = 1,
    ExclusiveAccess = 2,
    WriteExclusiveRegsOnly = 3,
    ExclusiveAccessRegsOnly = 4,
    WriteExclusiveAllRegs = 5,
    ExclusiveAccessAllRegs = 6,
}

/// Nexus NVMe preemption policy.
#[derive(Serialize, Deserialize, Debug, Copy, Clone, Eq, PartialEq)]
pub enum NexusNvmePreemption {
    /// A "manual" preemption where we explicitly specify the reservation key,
    /// type and preempt key.
    ArgKey(Option<u64>),
    /// An "automatic" preemption where we can preempt whatever is current
    /// holder. Useful when we just want to boot the existing holder out.
    Holder,
}

#[test]
fn nvmf_controller_range() {
    #[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
    struct TestSpec {
        nvmf: NvmfControllerIdRange,
    }
    #[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
    struct Test<'a> {
        input: &'a str,
        expected: NvmfControllerIdRange,
    }
    let tests: Vec<Test> = vec![
        Test {
            input: r#"{"nvmf":{"start":1,"end":1}}"#,
            expected: NvmfControllerIdRange::new_min(1),
        },
        Test {
            input: r#"{"nvmf":{"start":1,"end":5}}"#,
            expected: NvmfControllerIdRange::new_min(5),
        },
        Test {
            input: r#"{"nvmf":{"start":1,"end":65519}}"#,
            expected: NvmfControllerIdRange::new_min(0xffff),
        },
        Test {
            input: r#"{"nvmf":{"start":1,"end":1}}"#,
            expected: NvmfControllerIdRange::new(1, 0xffef).unwrap().next(1),
        },
        Test {
            input: r#"{"nvmf":{"start":1,"end":2}}"#,
            expected: NvmfControllerIdRange::new(1, 0xffef).unwrap().next(2),
        },
        Test {
            input: r#"{"nvmf":{"start":65519,"end":65519}}"#,
            expected: NvmfControllerIdRange::new(1, 0xffee).unwrap().next(1),
        },
        Test {
            input: r#"{"nvmf":{"start":1,"end":2}}"#,
            expected: NvmfControllerIdRange::new(1, 0xffee).unwrap().next(2),
        },
        Test {
            input: r#"{"nvmf":{"start":6,"end":7}}"#,
            expected: NvmfControllerIdRange::new_min(5).next(2),
        },
    ];

    for test in &tests {
        println!("{}", serde_json::to_string(&test.expected).unwrap());
        let spec: TestSpec = serde_json::from_str(test.input).unwrap();
        assert_eq!(test.expected, spec.nvmf);
    }

    let mut range = NvmfControllerIdRange::new_min(1);
    loop {
        range = range.next(8);
        if range.min() == &1 {
            break;
        }
    }
}

/// Nvmf Controller Id Range
#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NvmfControllerIdRange(std::ops::RangeInclusive<u16>);
impl NvmfControllerIdRange {
    /// Create `Self` with the minimum controller id.
    pub fn new_min(width: u16) -> Self {
        let min = *Self::controller_id_range().start();
        let max = Self::end_from_min(min, width);
        Self(min..=max)
    }
    fn end_from_min(min: u16, width: u16) -> u16 {
        let width = width.min(*Self::controller_id_range().end());

        if width > 0 {
            let end = min as u32 + width as u32 - 1;
            u16::try_from(end).unwrap_or(u16::MAX)
        } else {
            min
        }
    }
    /// create `Self` with a random minimum controller id
    pub fn random_min() -> Self {
        let min = *Self::controller_id_range().start();
        let max = *Self::controller_id_range().end();
        let rand_min = u16::min(rand::random::<u16>() + min, max);
        Self(rand_min..=max)
    }
    /// minimum controller id
    pub fn min(&self) -> &u16 {
        self.0.start()
    }
    /// maximum controller id
    pub fn max(&self) -> &u16 {
        self.0.end()
    }
    fn controller_id_range() -> std::ops::RangeInclusive<u16> {
        const MIN_CONTROLLER_ID: u16 = 1;
        const MAX_CONTROLLER_ID: u16 = 0xffef;
        MIN_CONTROLLER_ID..=MAX_CONTROLLER_ID
    }
    /// create a new NvmfControllerIdRange using provided range
    pub fn new(start: u16, end: u16) -> Result<Self, ReplyError> {
        if NvmfControllerIdRange::controller_id_range().contains(&start)
            && NvmfControllerIdRange::controller_id_range().contains(&end)
            && start <= end
        {
            Ok(NvmfControllerIdRange(RangeInclusive::new(start, end)))
        } else {
            Err(ReplyError::invalid_argument(
                ResourceKind::Nexus,
                "nvmf_controller_id_range",
                format!("{start}, {end} values don't fall in controller id range"),
            ))
        }
    }
    /// Bump the controller id range.
    /// The new range shall use the given `width`.
    /// # Note: this will wrap around back to the minimum.
    pub fn next(&self, width: u16) -> Self {
        let range = self.0.clone();

        let start = u16::try_from(*range.end() as u32 + 1).unwrap_or_default();
        let end = Self::end_from_min(start, width);

        if let Ok(range) = Self::new(start, end) {
            // wrap around to the start of the range
            range
        } else {
            Self::new_min(width)
        }
    }
}
impl Default for NvmfControllerIdRange {
    fn default() -> Self {
        Self(Self::controller_id_range())
    }
}

/// Nexus Nvmf target configuration.
#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NexusNvmfConfig {
    /// Limits the controller id range.
    controller_id_range: NvmfControllerIdRange,
    /// Persistent reservation key.
    reservation_key: u64,
    /// Persistent reservation type.
    reservation_type: NvmeReservation,
    /// Preemption policy.
    preempt_policy: NexusNvmePreemption,
}

impl NexusNvmfConfig {
    /// The minimum controller id that can be used by the nvmf target.
    pub fn min_cntl_id(&self) -> u16 {
        *self.controller_id_range.min()
    }
    /// The maximum controller id that can be used by the nvmf target.
    pub fn max_cntl_id(&self) -> u16 {
        *self.controller_id_range.max()
    }
    /// The persistent reservation key.
    pub fn resv_key(&self) -> u64 {
        self.reservation_key
    }
    /// The persistent reservation type.
    pub fn resv_type(&self) -> NvmeReservation {
        self.reservation_type
    }
    /// The reservation key to be preempted.
    pub fn preempt_key(&self) -> u64 {
        match self.preempt_policy {
            NexusNvmePreemption::ArgKey(v) => v.unwrap_or_default(),
            NexusNvmePreemption::Holder => 0,
        }
    }
    /// The optional preemption reservation key value.
    pub fn preempt_key_opt(&self) -> Option<u64> {
        match self.preempt_policy {
            NexusNvmePreemption::ArgKey(v) => v,
            NexusNvmePreemption::Holder => None,
        }
    }
    /// The nexus preemption policy.
    pub fn preempt_policy(&self) -> NexusNvmePreemption {
        self.preempt_policy
    }
    /// Create a new NexusNvmfConfig with the args.
    pub fn new(
        controller_id_range: NvmfControllerIdRange,
        reservation_key: u64,
        reservation_type: NvmeReservation,
        preempt_policy: NexusNvmePreemption,
    ) -> Self {
        Self {
            controller_id_range,
            reservation_key,
            reservation_type,
            preempt_policy,
        }
    }
    /// Get the controller id range.
    pub fn controller_id_range(&self) -> NvmfControllerIdRange {
        self.controller_id_range.clone()
    }

    /// Disable reservations, mostly useful for testing only.
    pub fn with_no_resv(mut self) -> Self {
        self.reservation_key = 0;
        self.preempt_policy = NexusNvmePreemption::ArgKey(None);
        self.reservation_type = NvmeReservation::Reserved;
        self
    }
    /// On v0 reservations are mostly disabled, simply set key to 1 to avoid failure.
    pub fn with_no_resv_v0(mut self) -> Self {
        self.reservation_key = 1;
        self.preempt_policy = NexusNvmePreemption::ArgKey(None);
        self.reservation_type = NvmeReservation::Reserved;
        self
    }
}

impl Default for NexusNvmfConfig {
    fn default() -> Self {
        Self {
            controller_id_range: NvmfControllerIdRange::default(),
            reservation_key: 1,
            reservation_type: NvmeReservation::ExclusiveAccess,
            preempt_policy: NexusNvmePreemption::Holder,
        }
    }
}

impl CreateNexus {
    /// Create new `Self` from the given parameters.
    pub fn new(
        node: &NodeId,
        uuid: &NexusId,
        size: u64,
        children: &[NexusChild],
        managed: bool,
        owner: Option<&VolumeId>,
        config: Option<NexusNvmfConfig>,
    ) -> Self {
        Self {
            node: node.clone(),
            uuid: uuid.clone(),
            size,
            children: children.to_owned(),
            managed,
            owner: owner.cloned(),
            config,
        }
    }
    /// Name of the nexus.
    /// When part of a volume, it's set to its `VolumeId`. Otherwise it's set to its `NexusId`.
    pub fn name(&self) -> String {
        let name = self.owner.as_ref().map(|i| i.to_string());
        name.unwrap_or_else(|| self.uuid.to_string())
    }
    /// Name of the nexus as uuid.
    /// When part of a volume, it's set to its `VolumeId`. Otherwise it's set to its `NexusId`.
    pub fn name_uuid(&self) -> uuid::Uuid {
        let name = self.owner.as_ref().map(|i| *i.uuid());
        name.unwrap_or_else(|| *self.uuid.uuid())
    }

    /// Return the key that should be used by the Io-Engine to persist the NexusInfo.
    pub fn nexus_info_key(&self) -> String {
        match &self.owner {
            Some(volume_id) => NexusInfoKey::new(&Some(volume_id.clone()), &self.uuid).key(),
            None => NexusInfoKey::new(&None, &self.uuid).key(),
        }
    }
}

/// Destroy Nexus Request.
#[derive(Serialize, Deserialize, Default, Debug, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DestroyNexus {
    /// The id of the io-engine instance.
    pub node: NodeId,
    /// The uuid of the nexus.
    pub uuid: NexusId,
    /// Nexus disowners.
    disowners: NexusOwners,
    /// Sets the nexus spec to deleting even if the node is offline.
    /// The reconcilers will pick up the slack.
    lazy: bool,
}
impl DestroyNexus {
    /// Create new `Self` from the given node and nexus id's.
    pub fn new(node: NodeId, uuid: NexusId) -> Self {
        Self {
            node,
            uuid,
            disowners: NexusOwners::None,
            lazy: false,
        }
    }
    /// With lazy deletion.
    pub fn with_lazy(mut self, lazy: bool) -> Self {
        self.lazy = lazy;
        self
    }
    /// Get the lazy flag.
    pub fn lazy(&self) -> bool {
        self.lazy
    }
    /// Disown all owners.
    pub fn with_disown_all(mut self) -> Self {
        self.disowners = NexusOwners::All;
        self
    }
    /// Disown volume owner.
    pub fn with_disown(mut self, volume: &VolumeId) -> Self {
        self.disowners = NexusOwners::Volume(volume.clone());
        self
    }
    /// Return a reference to the disowners.
    pub fn disowners(&self) -> &NexusOwners {
        &self.disowners
    }
}

/// Shutdown Nexus Request.
#[derive(Serialize, Deserialize, Default, Debug, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ShutdownNexus {
    /// The uuid of the nexus.
    uuid: NexusId,
    /// Shutdown the nexus spec even if the node is offline.
    /// The reconcilers will pick up the slack.
    lazy: bool,
}

impl ShutdownNexus {
    /// Create a new `ShutdownNexus` using `uuid`.
    pub fn new(uuid: NexusId, lazy: bool) -> Self {
        Self { uuid, lazy }
    }
    /// Get uuid of the nexus.
    pub fn uuid(&self) -> NexusId {
        self.uuid.clone()
    }
    /// With lazy shutdown.
    pub fn with_lazy(mut self, lazy: bool) -> Self {
        self.lazy = lazy;
        self
    }
    /// Get the lazy flag.
    pub fn lazy(&self) -> bool {
        self.lazy
    }
}

/// Nexus owners which is a volume, none or disowned by all.
#[derive(Serialize, Deserialize, Default, Debug, Clone, Eq, PartialEq)]
pub enum NexusOwners {
    #[default]
    None,
    Volume(VolumeId),
    All,
}
impl NexusOwners {
    /// Create a special `Self` that will disown all owners.
    pub fn new_disown_all() -> Self {
        Self::All
    }
    /// Set to disown all owners.
    pub fn with_disown_all(self) -> Self {
        Self::All
    }
    /// Disown all owners.
    pub fn disown_all(&self) -> bool {
        match self {
            NexusOwners::None => false,
            NexusOwners::Volume(_) => false,
            NexusOwners::All => true,
        }
    }
    /// Return the volume owner, if any
    pub fn volume(&self) -> Option<&VolumeId> {
        match self {
            NexusOwners::None => None,
            NexusOwners::Volume(volume) => Some(volume),
            NexusOwners::All => None,
        }
    }
}

impl From<Nexus> for DestroyNexus {
    fn from(nexus: Nexus) -> Self {
        Self {
            node: nexus.node,
            uuid: nexus.uuid,
            ..Default::default()
        }
    }
}

/// Share Nexus Request
#[derive(Serialize, Deserialize, Default, Debug, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ShareNexus {
    /// id of the io-engine instance
    pub node: NodeId,
    /// uuid of the nexus
    pub uuid: NexusId,
    /// encryption key
    pub key: Option<String>,
    /// share protocol
    pub protocol: NexusShareProtocol,
    /// host nqn's allowed to connect to the target.
    pub allowed_hosts: Vec<HostNqn>,
}
impl ShareNexus {
    /// Return new `Self` from the given parameters.
    pub fn new(nexus: &Nexus, protocol: NexusShareProtocol, nqns: Vec<HostNqn>) -> Self {
        Self {
            node: nexus.node.clone(),
            uuid: nexus.uuid.clone(),
            protocol,
            allowed_hosts: nqns.into_vec(),
            ..Default::default()
        }
    }
}
impl From<&Nexus> for UnshareNexus {
    fn from(from: &Nexus) -> Self {
        Self {
            node: from.node.clone(),
            uuid: from.uuid.clone(),
        }
    }
}
impl From<ShareNexus> for UnshareNexus {
    fn from(share: ShareNexus) -> Self {
        Self {
            node: share.node,
            uuid: share.uuid,
        }
    }
}

/// Unshare Nexus Request
#[derive(Serialize, Deserialize, Default, Debug, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UnshareNexus {
    /// id of the io-engine instance
    pub node: NodeId,
    /// uuid of the nexus
    pub uuid: NexusId,
}

/// Get rebuild history request.
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct GetRebuildRecord {
    /// UUID of Nexus.
    pub nexus: NexusId,
}

/// List rebuild history request.
#[derive(Default, Debug, Clone)]
pub struct ListRebuildRecord {
    /// Maximum number of rebuild records per nexus.
    count: Option<u32>,
    since_end_time: Option<prost_types::Timestamp>,
}

impl ListRebuildRecord {
    /// Returns instance of self.
    pub fn new(count: Option<u32>, since_end_time: Option<prost_types::Timestamp>) -> Self {
        Self {
            count,
            since_end_time,
        }
    }

    /// Get the max rebuild record count.
    pub fn count(&self) -> Option<u32> {
        self.count
    }
    /// Get the since_end timestamp.
    pub fn since_time_ref(&self) -> Option<&prost_types::Timestamp> {
        self.since_end_time.as_ref()
    }
    /// Get the since_end timestamp.
    pub fn since_time(self) -> Option<prost_types::Timestamp> {
        self.since_end_time
    }
}

impl GetRebuildRecord {
    /// Returns instance of self.
    pub fn new(uuid: NexusId) -> Self {
        Self { nexus: uuid }
    }

    /// Get nexus id from request.
    pub fn nexus(&self) -> NexusId {
        self.nexus.clone()
    }
}

/// Rebuild history lists all rebuild jobs for a nexus.
#[derive(Serialize, Deserialize, Default, Debug, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RebuildHistory {
    /// Nexus UUID.
    pub uuid: NexusId,
    /// Nexus Name.
    pub name: String,
    /// List of all rebuilds finalised on the nexus.
    pub records: Vec<RebuildRecord>,
}

/// Rebuild job record of a child. This record is for the finalised rebuild job.
#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RebuildRecord {
    /// Rebuilding child uri.
    pub child_uri: ChildUri,
    /// Uri of the src child for rebuild job.
    pub src_uri: ChildUri,
    /// Final state of the rebuild job.
    pub state: RebuildJobState,
    /// Total blocks to rebuild.
    pub blocks_total: u64,
    /// Number of blocks recovered.
    pub blocks_recovered: u64,
    /// Number of blocks transferred when job ended.
    pub blocks_transferred: u64,
    /// Number of blocks remaining when job ended.
    pub blocks_remaining: u64,
    /// Number of blocks per task in the job.
    pub blocks_per_task: u64,
    /// Size of each blocks in the task.
    pub block_size: u64,
    /// True means its Partial rebuild job. If false, its Full rebuild job.
    pub is_partial: bool,
    /// Start time of the rebuild job (UTC).
    pub start_time: SystemTime,
    /// End time of the rebuild job (UTC).
    pub end_time: SystemTime,
}

/// Defines the state of Rebuild job.
#[derive(Serialize, Deserialize, Default, Debug, Clone, Eq, PartialEq)]
pub enum RebuildJobState {
    #[default]
    /// Rebuild Job is created.
    Init,
    /// Rebuild job is in progress.
    Rebuilding,
    /// Rebuild job is stopped.
    Stopped,
    /// Rebuild job is manually paused.
    Paused,
    /// Rebuild job failed in between.
    Failed,
    /// Rebuild job is completed, Successful.
    Completed,
}
