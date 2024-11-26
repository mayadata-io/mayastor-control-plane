//! Definition of pool types that can be saved to the persistent store.

use crate::types::v0::{
    openapi::models,
    store::{
        definitions::{ObjectKey, StorableObject, StorableObjectType},
        AsOperationSequencer, OperationSequence, SpecStatus, SpecTransaction,
    },
    transport::{self, CreatePool, NodeId, PoolDeviceUri, PoolId},
};

// PoolLabel is the type for the labels
pub type PoolLabel = std::collections::HashMap<String, String>;

use crate::types::v0::transport::ImportPool;
use pstor::ApiVersion;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, convert::From, fmt::Debug};

/// Pool data structure used by the persistent store.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Pool {
    /// Current state of the pool.
    pub state: Option<PoolState>,
    /// Desired pool specification.
    pub spec: Option<PoolSpec>,
}

/// Runtime state of the pool.
/// This should eventually satisfy the PoolSpec.
#[derive(Serialize, Deserialize, Debug, PartialEq, Default, Clone)]
pub struct PoolState {
    /// Pool information returned by the Io-Engine.
    pub pool: transport::PoolState,
}

impl From<transport::PoolState> for PoolState {
    fn from(pool: transport::PoolState) -> Self {
        Self { pool }
    }
}

/// Status of the Pool Spec
pub type PoolSpecStatus = SpecStatus<transport::PoolStatus>;

impl From<&CreatePool> for PoolSpec {
    fn from(request: &CreatePool) -> Self {
        Self {
            node: request.node.clone(),
            id: request.id.clone(),
            disks: request.disks.clone(),
            status: PoolSpecStatus::Creating,
            labels: request.labels.clone(),
            sequencer: OperationSequence::new(),
            operation: None,
            creat_tsc: None,
        }
    }
}
impl From<&PoolSpec> for CreatePool {
    fn from(pool: &PoolSpec) -> Self {
        Self {
            node: pool.node.clone(),
            id: pool.id.clone(),
            disks: pool.disks.clone(),
            labels: pool.labels.clone(),
        }
    }
}
impl PartialEq<CreatePool> for PoolSpec {
    fn eq(&self, other: &CreatePool) -> bool {
        let mut other = PoolSpec::from(other);
        other.status = self.status.clone();
        other.sequencer = self.sequencer.clone();
        other.creat_tsc = self.creat_tsc;
        &other == self
    }
}

/// User specification of a pool.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct PoolSpec {
    /// id of the io-engine instance
    pub node: NodeId,
    /// id of the pool
    pub id: PoolId,
    /// absolute disk paths claimed by the pool
    pub disks: Vec<PoolDeviceUri>,
    /// status of the pool
    pub status: PoolSpecStatus,
    /// labels to be set on the pool
    pub labels: Option<PoolLabel>,
    /// Update in progress
    #[serde(skip)]
    pub sequencer: OperationSequence,
    /// Record of the operation in progress
    pub operation: Option<PoolOperationState>,
    /// Last modification timestamp.
    #[serde(skip)]
    pub creat_tsc: Option<std::time::SystemTime>,
}

impl PoolSpec {
    /// Pool identification.
    pub fn id(&self) -> &PoolId {
        &self.id
    }

    /// Label pool by applying the labels.
    pub fn label(&mut self, labels: HashMap<String, String>) {
        match &mut self.labels {
            Some(existing_labels) => {
                existing_labels.extend(labels);
            }
            None => {
                self.labels = Some(labels);
            }
        }
    }

    /// Check if the pool has the given topology label key.
    pub fn has_labels_key(&self, key: &str) -> bool {
        if let Some(labels) = &self.labels {
            return labels.contains_key(key);
        }
        false
    }

    /// Remove label from pool.
    pub fn unlabel(&mut self, label_key: &str) {
        if let Some(labels) = &mut self.labels {
            labels.remove(label_key);
        }
    }

    /// Check if there are key collisions between current topology labels and the given labels.
    pub fn label_collisions<'a>(
        &'a self,
        labels: &'a HashMap<String, String>,
    ) -> (HashMap<&'a String, &'a String>, bool) {
        let mut conflict = false;
        let mut existing_conflicts = HashMap::new();

        if let Some(existing_labels) = &self.labels {
            for (key, value) in labels {
                if let Some(existing_value) = existing_labels.get(key) {
                    if existing_value != value {
                        conflict = true;
                        existing_conflicts.insert(key, existing_value);
                    }
                }
            }
        }

        (existing_conflicts, conflict)
    }
}

impl From<&PoolSpec> for ImportPool {
    fn from(value: &PoolSpec) -> Self {
        Self {
            node: value.node.clone(),
            id: value.id.clone(),
            disks: value.disks.clone(),
            uuid: None,
        }
    }
}

impl AsOperationSequencer for PoolSpec {
    fn as_ref(&self) -> &OperationSequence {
        &self.sequencer
    }

    fn as_mut(&mut self) -> &mut OperationSequence {
        &mut self.sequencer
    }
}

impl From<PoolSpec> for models::PoolSpec {
    fn from(src: PoolSpec) -> Self {
        Self::new_all(src.disks, src.id, src.labels, src.node, src.status)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PoolOperationState {
    /// Record of the operation
    pub operation: PoolOperation,
    /// Result of the operation
    pub result: Option<bool>,
}

impl SpecTransaction<PoolOperation> for PoolSpec {
    fn has_pending_op(&self) -> bool {
        self.operation.is_some()
    }

    fn commit_op(&mut self) {
        if let Some(op) = self.operation.clone() {
            match op.operation {
                PoolOperation::Destroy => {
                    self.status = SpecStatus::Deleted;
                }
                PoolOperation::Create => {
                    self.status = SpecStatus::Created(transport::PoolStatus::Online);
                }
                PoolOperation::Label(PoolLabelOp { labels, .. }) => {
                    self.label(labels);
                }
                PoolOperation::Unlabel(PoolUnLabelOp { label_key }) => {
                    self.unlabel(&label_key);
                }
            }
        }
        self.clear_op();
    }

    fn clear_op(&mut self) {
        self.operation = None;
    }

    fn start_op(&mut self, operation: PoolOperation) {
        if matches!(operation, PoolOperation::Create) && self.creat_tsc.is_none() {
            self.creat_tsc = Some(std::time::SystemTime::now());
        }
        self.operation = Some(PoolOperationState {
            operation,
            result: None,
        })
    }

    fn set_op_result(&mut self, result: bool) {
        if let Some(op) = &mut self.operation {
            op.result = Some(result);
        }
    }

    fn pending_op(&self) -> Option<&PoolOperation> {
        self.operation.as_ref().map(|o| &o.operation)
    }

    fn log_op(&self, _operation: &PoolOperation) -> (bool, bool) {
        (false, true)
    }
}

/// Available Pool Operations
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum PoolOperation {
    Create,
    Destroy,
    Label(PoolLabelOp),
    Unlabel(PoolUnLabelOp),
}

/// Parameter for adding pool labels.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PoolLabelOp {
    pub labels: HashMap<String, String>,
    pub overwrite: bool,
}
impl From<(HashMap<String, String>, bool)> for PoolLabelOp {
    fn from((labels, overwrite): (HashMap<String, String>, bool)) -> Self {
        Self { labels, overwrite }
    }
}
/// Parameter for removing pool labels.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PoolUnLabelOp {
    pub label_key: String,
}
impl From<String> for PoolUnLabelOp {
    fn from(label_key: String) -> Self {
        Self { label_key }
    }
}

impl PartialEq<transport::PoolState> for PoolSpec {
    fn eq(&self, other: &transport::PoolState) -> bool {
        self.node == other.node
    }
}

/// Key used by the store to uniquely identify a PoolSpec structure.
pub struct PoolSpecKey(PoolId);

impl From<&PoolId> for PoolSpecKey {
    fn from(id: &PoolId) -> Self {
        Self(id.clone())
    }
}

impl ObjectKey for PoolSpecKey {
    type Kind = StorableObjectType;

    fn version(&self) -> ApiVersion {
        ApiVersion::V0
    }

    fn key_type(&self) -> StorableObjectType {
        StorableObjectType::PoolSpec
    }

    fn key_uuid(&self) -> String {
        self.0.to_string()
    }
}

impl StorableObject for PoolSpec {
    type Key = PoolSpecKey;

    fn key(&self) -> Self::Key {
        PoolSpecKey(self.id.clone())
    }
}

impl From<&PoolSpec> for transport::PoolState {
    fn from(pool: &PoolSpec) -> Self {
        Self {
            node: pool.node.clone(),
            id: pool.id.clone(),
            disks: pool.disks.clone(),
            status: transport::PoolStatus::Unknown,
            capacity: 0,
            used: 0,
            committed: None,
        }
    }
}
