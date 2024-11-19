use super::{
    super::registry::Registry, resource_map::ResourceMutexMap, OperationGuardArc, ResourceMutex,
    ResourceUid, UpdateInnerValue,
};
use crate::controller::{
    resources::migration::migrate_product_v1_to_v2, task_poller::PollTriggerEvent,
};
use agents::errors::SvcError;
use stor_port::{
    pstor::{product_v1_key_prefix, API_VERSION},
    transport_api::{ErrorChain, ResourceKind},
    types::v0::{
        openapi::apis::Uuid,
        store::{
            app_node::AppNodeSpec,
            definitions::{
                key_prefix_obj, ObjectKey, StorableObject, StorableObjectType, Store, StoreError,
            },
            nexus::NexusSpec,
            node::NodeSpec,
            pool::PoolSpec,
            replica::ReplicaSpec,
            snapshots::volume::VolumeSnapshot,
            volume::{AffinityGroupSpec, VolumeContentSource, VolumeSpec},
            AsOperationSequencer, OperationMode, SpecStatus, SpecTransaction,
        },
        transport::{AppNodeId, NexusId, NodeId, PoolId, ReplicaId, SnapshotId, VolumeId},
    },
};

use async_trait::async_trait;
use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use snafu::{ResultExt, Snafu};
use std::{fmt::Debug, ops::Deref, sync::Arc};

#[derive(Debug, Snafu)]
#[snafu(context(suffix(false)))]
enum SpecError {
    /// Failed to get entries from the persistent store.
    #[snafu(display("Failed to get entries from store. Error {}", source))]
    StoreGet { source: Box<StoreError> },
    #[snafu(display("Failed to migrate entries from v1 to v2 space. Error {}", source))]
    StoreMigrate { source: Box<StoreError> },
    /// Failed to get entries from the persistent store.
    #[snafu(display("Failed to deserialise object type {}", obj_type))]
    Deserialise {
        obj_type: StorableObjectType,
        source: serde_json::Error,
    },
}

/// What to do when creation fails.
#[derive(Debug)]
pub(crate) enum OnCreateFail {
    /// Leave object as `Creating`, could allow for frontend retries.
    LeaveAsIs,
    /// When frontend retries don't make sense, set it to deleting so we can clean-up.
    SetDeleting,
    /// When there's no need to garbage collect, simply delete it.
    Delete,
}
impl OnCreateFail {
    /// If result is a tonic invalid argument then delete the resource.
    /// # Warning:
    /// Use this only when a single operation has been attempted otherwise we might
    /// leave existing resources around for some time.
    pub(crate) fn eeinval_delete<O>(result: &Result<O, SvcError>) -> Self {
        match result {
            Err(error) if error.tonic_code() == tonic::Code::InvalidArgument => Self::Delete,
            _ => Self::SetDeleting,
        }
    }
    /// Map errors into `Self` for pool creation requests, specifically.
    pub(crate) fn on_pool_create_err<O>(result: &Result<O, SvcError>) -> Self {
        let Err(ref error) = result else {
            // nonsensical but that's how the api is today...
            return Self::SetDeleting;
        };
        match error.tonic_code() {
            // 1. the disk is open by another pool or bdev
            // 2. the disk contains a pool with another name
            tonic::Code::InvalidArgument => Self::Delete,
            // 1. the pool disk is not available (ie not found or broken somehow)
            tonic::Code::NotFound => Self::Delete,
            // In this case, it's the pool operator's job to attempt re-creation of the pool.
            // 1. pre-2.6 dataplane, contention on the pool service
            // 2. pool disk is very slow or extremely large
            // 3. dataplane core is shared with other processes
            // TODO: use higher timeout on larger pool sizes or potentially make this
            //  an async operation.
            tonic::Code::Cancelled => Self::LeaveAsIs,
            _ => Self::SetDeleting,
        }
    }
}

/// This trait is used to encapsulate common behaviour for all different types of resources,
/// including validation rules and error handling.
#[async_trait]
pub(crate) trait GuardedOperationsHelper:
    Debug + Sync + Send + Sized + Deref<Target = ResourceMutex<Self::Inner>> + UpdateInnerValue
{
    type Create: Debug + PartialEq + Sync + Send;
    type Owners: Default + Sync + Send;
    type Status: PartialEq + Sync + Send;
    type State: PartialEq + Sync + Send;
    type UpdateOp: Sync + Send;
    type Inner: SpecOperationsHelper<
        Create = Self::Create,
        Owners = Self::Owners,
        Status = Self::Status,
        State = Self::State,
        UpdateOp = Self::UpdateOp,
    >;

    /// Start a create operation and attempt to log the transaction to the store.
    /// In case of error, the log is undone and an error is returned.
    /// Also updates it's inner value to the operation.
    async fn start_create_update<O>(
        &mut self,
        registry: &Registry,
        request: &Self::Create,
    ) -> Result<Self::Inner, SvcError>
    where
        Self::Inner: PartialEq<Self::Create>,
        Self::Inner: SpecTransaction<O>,
        Self::Inner: StorableObject,
    {
        let result = self.start_create(registry, request).await?;
        self.update();
        Ok(result)
    }

    /// Start a create operation and attempt to log the transaction to the store.
    /// In case of error, the log is undone and an error is returned.
    async fn start_create<O>(
        &self,
        registry: &Registry,
        request: &Self::Create,
    ) -> Result<Self::Inner, SvcError>
    where
        Self::Inner: PartialEq<Self::Create>,
        Self::Inner: SpecTransaction<O>,
        Self::Inner: StorableObject,
    {
        let spec_clone = {
            let mut spec = self.lock();
            match spec.start_create_inner(request) {
                Err(SvcError::InvalidUuid { uuid, kind }) => {
                    drop(spec);
                    self.remove_spec(registry);
                    return Err(SvcError::InvalidUuid { uuid, kind });
                }
                Err(error) => Err(error),
                Ok(_) => Ok(()),
            }?;
            spec.clone()
        };
        match self.store_operation_log(registry, &spec_clone).await {
            Ok(_) => Ok(spec_clone),
            Err(e) => {
                self.delete_spec(registry).await.ok();
                Err(e)
            }
        }
    }

    /// Completes a create operation by trying to update the spec in the persistent store.
    /// If the persistent store operation fails then the spec is marked accordingly and the dirty
    /// spec reconciler will attempt to update the store when the store is back online.
    /// # Note:
    /// `on_err_destroy` is used to determine if the resource spec should be deleted on error.
    /// On most cases we don't want to destroy as that will prevent garbage collection.
    async fn complete_create<O, R: Send + Debug>(
        &mut self,
        result: Result<R, SvcError>,
        registry: &Registry,
        on_fail: OnCreateFail,
    ) -> Result<R, SvcError>
    where
        Self::Inner: SpecTransaction<O>,
    {
        match result {
            Ok(val) => {
                tracing::info!(?val, "complete_create");

                let mut spec_clone = self.lock().clone();
                spec_clone.commit_op();

                let stored = registry.store_obj(&spec_clone).await;
                match stored {
                    Ok(_) => {
                        self.complete_op();
                        Ok(val)
                    }
                    Err(error) => {
                        self.lock().set_op_result(true);
                        Err(error)
                    }
                }
            }
            Err(error) => Err(self.handle_create_failed(registry, error, on_fail).await),
        }
    }

    /// Validates the outcome of a create step.
    /// In case of an error, the object is set to deleting.
    #[allow(unused)]
    async fn validate_create_step<R: Send, O>(
        &self,
        registry: &Registry,
        result: Result<R, SvcError>,
    ) -> Result<R, SvcError>
    where
        Self::Inner: SpecTransaction<O>,
        Self::Inner: StorableObject,
    {
        self.validate_create_step_ext(registry, result, OnCreateFail::SetDeleting)
            .await
    }

    /// Validates the outcome of a create step.
    /// In case of an error, it is handled as per the `OnCreateFail` policy.
    async fn validate_create_step_ext<R: Send, O>(
        &self,
        registry: &Registry,
        result: Result<R, SvcError>,
        on_fail: OnCreateFail,
    ) -> Result<R, SvcError>
    where
        Self::Inner: SpecTransaction<O>,
        Self::Inner: StorableObject,
    {
        match result {
            Ok(val) => Ok(val),
            Err(error) => Err(self.handle_create_failed(registry, error, on_fail).await),
        }
    }

    /// Handles a failed creation according to the `OnCreateFail` policy.
    async fn handle_create_failed<O>(
        &self,
        registry: &Registry,
        error: SvcError,
        on_fail: OnCreateFail,
    ) -> SvcError
    where
        Self::Inner: SpecTransaction<O>,
        Self::Inner: StorableObject,
    {
        match on_fail {
            OnCreateFail::LeaveAsIs => {
                let mut spec_clone = self.lock().clone();
                spec_clone.clear_op();
                let stored = registry.store_obj(&spec_clone).await;
                match stored {
                    Ok(_) => {
                        self.lock().clear_op();
                    }
                    Err(_) => {
                        self.lock().set_op_result(false);
                    }
                }
                error
            }
            OnCreateFail::SetDeleting => {
                // Let the garbage collector delete the spec gracefully.
                // This will ensure we'll delete previously created resources.
                let spec = self.lock().fail_creating_to_deleting();
                registry.store_obj(&spec).await.ok();
                // TODO: we could use this to reconcile quicker?
                if std::env::var("CREATING_DELETING_NOTIFY").is_ok() {
                    registry
                        .notify(PollTriggerEvent::ResourceCreatingToDeleting)
                        .await;
                }
                error
            }
            OnCreateFail::Delete => {
                self.delete_spec(registry).await.ok();
                error
            }
        }
    }

    // Attempt to delete the spec from the persistent store and the registry.
    // If the persistent store is unavailable the spec is marked as dirty and the dirty spec
    // reconciler will attempt to update the store when the store is back online.
    async fn delete_spec<O>(&self, registry: &Registry) -> Result<(), SvcError>
    where
        Self::Inner: SpecTransaction<O>,
        Self::Inner: StorableObject,
    {
        let spec_clone = self.lock().clone();

        // Attempt to delete the spec from the persistent store.
        match registry.delete_kv(&spec_clone.key().key()).await {
            Ok(_) => {
                // Delete the spec from the registry.
                self.remove_spec(registry);
                Ok(())
            }
            Err(e) => {
                tracing::error!(
                    "Failed to delete spec {:?} from the persistent store. Error {:?}",
                    spec_clone,
                    e
                );
                // The spec failed to be deleted from the store, so don't delete it from the
                // registry. Instead, mark the result of the operation as failed so that the garbage
                // collector can tidy it up.
                self.lock().set_op_result(false);
                Err(e)
            }
        }
    }

    /// Start a destroy operation and attempt to log the transaction to the store.
    /// In case of error, the log is undone and an error is returned.
    /// If the del_owned flag is set, then we skip the check for owners.
    /// Otherwise, if the spec is still owned then we cannot proceed with deletion.
    async fn start_destroy<O>(&self, registry: &Registry) -> Result<(), SvcError>
    where
        Self::Inner: SpecTransaction<O>,
        Self::Inner: StorableObject,
    {
        self.start_destroy_by(registry, &Self::Owners::default())
            .await
    }

    /// Start a destroy operation by spec owners and attempt to log the transaction to the store.
    /// In case of error, the log is undone and an error is returned.
    /// If the del_owned flag is set, then we skip the check for owners.
    /// The del_by parameter specifies who is trying to delete the resource. If the resource has any
    /// other owners then we cannot proceed with deletion but we disown the resource from del_by.
    async fn start_destroy_by<O>(
        &self,
        registry: &Registry,
        owners: &Self::Owners,
    ) -> Result<(), SvcError>
    where
        Self::Inner: SpecTransaction<O>,
        Self::Inner: StorableObject,
    {
        {
            let mut spec = self.lock();
            let _ = spec.busy()?;
            if spec.status().deleted() {
                return Ok(());
            } else {
                spec.disown(owners);
                if spec.owned() {
                    tracing::error!(
                        "{:?} id '{:?}' cannot be deleted because it's owned by: '{:?}'",
                        spec.kind(),
                        spec.uuid_str(),
                        spec.owners()
                    );
                    return Err(SvcError::InUse {
                        kind: spec.kind(),
                        id: spec.uuid_str(),
                    });
                }
            }
        }

        // resource specific validation rules
        Self::validate_destroy(self, registry)?;

        let spec_clone = {
            let mut spec = self.lock();

            // once we've started, there's no going back, so disown completely
            spec.set_status(SpecStatus::Deleting);
            spec.disown_all();

            spec.start_destroy_op();
            spec.clone()
        };

        self.store_operation_log(registry, &spec_clone).await?;
        Ok(())
    }

    /// Completes a destroy operation by trying to delete the spec from the persistent store.
    /// If the persistent store operation fails then the spec is marked accordingly and the dirty
    /// spec reconciler will attempt to update the store when the store is back online.
    async fn complete_destroy<O, R: Send + Debug>(
        &mut self,
        result: Result<R, SvcError>,
        registry: &Registry,
    ) -> Result<R, SvcError>
    where
        Self::Inner: SpecTransaction<O>,
        Self::Inner: StorableObject,
    {
        let key = self.lock().key();
        match result {
            Ok(val) => {
                tracing::info!(?val, "complete_destroy");

                let mut spec_clone = self.lock().clone();
                spec_clone.commit_op();
                let deleted = registry.delete_kv(&key.key()).await;
                match deleted {
                    Ok(_) => {
                        self.remove_spec(registry);
                        self.complete_op();
                        Ok(val)
                    }
                    Err(error) => {
                        self.lock().set_op_result(true);
                        self.update();
                        Err(error)
                    }
                }
            }
            Err(error) => {
                let mut spec_clone = self.lock().clone();
                spec_clone.clear_op();
                let stored = registry.store_obj(&spec_clone).await;
                let mut spec = self.lock();
                match stored {
                    Ok(_) => {
                        spec.clear_op();
                        Err(error)
                    }
                    Err(error) => {
                        spec.set_op_result(false);
                        Err(error)
                    }
                }
            }
        }
    }

    /// Start an update operation and attempt to log the transaction to the store.
    /// In case of error, the log is undone and an error is returned.
    async fn start_update(
        &self,
        registry: &Registry,
        state: &Self::State,
        update_operation: Self::UpdateOp,
    ) -> Result<Self::Inner, SvcError>
    where
        Self::Inner: PartialEq<Self::State>,
        Self::Inner: SpecTransaction<Self::UpdateOp>,
        Self::Inner: StorableObject,
    {
        let (spec_clone, log_op) = {
            let mut spec = self.lock().clone();
            let log_op = spec.log_op(&update_operation);
            spec.start_update_inner(registry, state, update_operation)
                .await?;
            *self.lock() = spec.clone();
            (spec, log_op.0)
        };

        if log_op {
            self.store_operation_log(registry, &spec_clone).await?;
        }
        Ok(spec_clone)
    }

    /// Completes an update operation by trying to update the spec in the persistent store.
    /// If the persistent store operation fails then the spec is marked accordingly and the dirty
    /// spec reconciler will attempt to update the store when the store is back online.
    async fn complete_update<R: Send + Debug, O>(
        &mut self,
        registry: &Registry,
        result: Result<R, SvcError>,
        mut spec_clone: Self::Inner,
    ) -> Result<R, SvcError>
    where
        Self::Inner: SpecTransaction<O>,
        Self::Inner: StorableObject,
    {
        let store_obj = spec_clone.flush_pending_op();
        match result {
            Ok(val) => {
                tracing::info!(?val, "complete_update");

                if !store_obj {
                    self.complete_op();
                    return Ok(val);
                }
                spec_clone.commit_op();
                let stored = registry.store_obj(&spec_clone).await;
                match stored {
                    Ok(_) => {
                        self.complete_op();
                        Ok(val)
                    }
                    Err(error) => {
                        self.lock().set_op_result(true);
                        Err(error)
                    }
                }
            }
            Err(error) => {
                if !store_obj {
                    self.lock().clear_op();
                    return Err(error);
                }
                spec_clone.clear_op();
                let stored = registry.store_obj(&spec_clone).await;
                let mut spec = self.lock();
                match stored {
                    Ok(_) => {
                        spec.clear_op();
                        Err(error)
                    }
                    Err(error) => {
                        spec.set_op_result(false);
                        Err(error)
                    }
                }
            }
        }
    }

    /// Validates the outcome of an intermediate step, part of a transaction operation.
    /// In case of an error, it undoes the changes to the spec.
    /// If the persistent store is unavailable the spec is marked as dirty and the dirty
    /// spec reconciler will attempt to update the store when the store is back online.
    async fn validate_update_step<R: Send, O>(
        &self,
        registry: &Registry,
        result: Result<R, SvcError>,
        spec_clone: &Self::Inner,
    ) -> Result<R, SvcError>
    where
        Self::Inner: SpecTransaction<O>,
        Self::Inner: StorableObject,
    {
        match result {
            Ok(val) => Ok(val),
            Err(error) => {
                let mut spec_clone = spec_clone.clone();
                spec_clone.clear_op();
                let stored = registry.store_obj(&spec_clone).await;
                let mut spec = self.lock();
                match stored {
                    Ok(_) => {
                        spec.clear_op();
                        Err(error)
                    }
                    Err(error) => {
                        spec.set_op_result(false);
                        Err(error)
                    }
                }
            }
        }
    }
    /// Operations that have started but were not able to complete because access to the
    /// persistent store was lost.
    /// Returns whether the incomplete operation has now been handled.
    async fn handle_incomplete_ops<O>(&mut self, registry: &Registry) -> bool
    where
        Self::Inner: SpecTransaction<O>,
        Self::Inner: StorableObject,
    {
        self.handle_incomplete_ops_ext(registry, OnCreateFail::SetDeleting)
            .await
    }
    /// Operations that have started but were not able to complete because access to the
    /// persistent store was lost.
    /// Returns whether the incomplete operation has now been handled.
    async fn handle_incomplete_ops_ext<O>(
        &mut self,
        registry: &Registry,
        on_fail: OnCreateFail,
    ) -> bool
    where
        Self::Inner: SpecTransaction<O>,
        Self::Inner: StorableObject,
    {
        let spec_status = self.lock().status();
        match spec_status {
            SpecStatus::Creating => {
                if matches!(on_fail, OnCreateFail::SetDeleting) {
                    // Go to deleting stage to make sure we clean-up previously allocated resources.
                    self.lock().set_status(SpecStatus::Deleting);
                    true
                } else {
                    self.handle_incomplete_updates(registry).await
                }
            }
            SpecStatus::Deleted => {
                self.delete_spec(registry).await.ok();
                true
            }
            SpecStatus::Created(_) | SpecStatus::Deleting => {
                // A spec that was being updated is in the `Created` state.
                // Deleting is also a "temporary" update to the spec.
                self.handle_incomplete_updates(registry).await
            }
        }
    }
    /// Updates that have started but were not able to complete because access to the
    /// persistent store was lost.
    async fn handle_incomplete_updates<O>(&mut self, registry: &Registry) -> bool
    where
        Self::Inner: SpecTransaction<O>,
        Self::Inner: StorableObject,
    {
        let mut spec_clone = self.lock().clone();
        match spec_clone.operation_result() {
            Some(Some(true)) => {
                spec_clone.commit_op();
                let result = registry.store_obj(&spec_clone).await;
                if result.is_ok() {
                    self.complete_op();
                }
                result.is_ok()
            }
            Some(Some(false)) => {
                spec_clone.clear_op();
                let result = registry.store_obj(&spec_clone).await;
                if result.is_ok() {
                    self.lock().clear_op();
                }
                result.is_ok()
            }
            Some(None) => {
                // we must have crashed... we could check the node to see what the
                // current state is but for now assume failure
                spec_clone.clear_op();
                let result = registry.store_obj(&spec_clone).await;
                if result.is_ok() {
                    self.lock().clear_op();
                }
                result.is_ok()
            }
            None => true,
        }
    }

    /// Attempt to store a spec object with a logged SpecOperation to the persistent store
    /// In case of failure the operation cannot proceed so clear it and return an error
    async fn store_operation_log<O>(
        &self,
        registry: &Registry,
        spec_clone: &Self::Inner,
    ) -> Result<(), SvcError>
    where
        Self::Inner: SpecTransaction<O>,
        Self::Inner: StorableObject,
    {
        if let Err(error) = registry.store_obj(spec_clone).await {
            let mut spec = self.lock();
            spec.clear_op();
            Err(error)
        } else {
            Ok(())
        }
    }

    /// Used for resource specific validation rules
    fn validate_destroy(&self, _registry: &Registry) -> Result<(), SvcError> {
        Ok(())
    }

    /// Remove the object from the global Spec List
    fn remove_spec(&self, registry: &Registry);

    fn complete_op<O>(&mut self)
    where
        Self::Inner: SpecTransaction<O>,
    {
        self.lock().commit_op();
        self.update();
    }
}

#[async_trait::async_trait]
pub(crate) trait SpecOperationsHelper:
    Clone + Debug + StorableObject + AsOperationSequencer + PartialEq<Self::Create>
{
    type Create: Debug + PartialEq + Sync + Send;
    type Status: PartialEq + Sync + Send;
    type State: PartialEq + Sync + Send;
    type Owners: Default + Sync + Send;
    type UpdateOp: Sync + Send;

    /// When a create request is issued we need to validate by verifying that:
    /// 1. a previous create operation is no longer in progress
    /// 2. if it's a retry then it must have the same parameters as the original request
    fn start_create_inner(&mut self, request: &Self::Create) -> Result<(), SvcError>
    where
        Self: PartialEq<Self::Create>,
    {
        // we're busy with another request, try again later
        self.busy()?;
        if self.uuid_str() == Uuid::default().to_string() {
            return Err(SvcError::InvalidUuid {
                uuid: self.uuid_str(),
                kind: self.kind(),
            });
        }
        if self.status().creating() {
            if self != request {
                Err(SvcError::ReCreateMismatch {
                    id: self.uuid_str(),
                    kind: self.kind(),
                    resource: format!("{self:?}"),
                    request: format!("{request:?}"),
                })
            } else {
                self.start_create_op(request);
                Ok(())
            }
        } else if self.status().created() {
            Err(SvcError::AlreadyExists {
                kind: self.kind(),
                id: self.uuid_str(),
            })
        } else {
            Err(SvcError::Deleting { kind: self.kind() })
        }
    }

    /// Checks that the object ready to accept a new update operation
    async fn start_update_inner(
        &mut self,
        registry: &Registry,
        state: &Self::State,
        operation: Self::UpdateOp,
    ) -> Result<(), SvcError>
    where
        Self: PartialEq<Self::State> + SpecTransaction<Self::UpdateOp>,
    {
        // we're busy right now, try again later
        let _ = self.busy()?;

        match self.status() {
            SpecStatus::Creating if self.allow_op_creating(&operation) => Ok(()),
            SpecStatus::Creating => Err(SvcError::PendingCreation {
                id: self.uuid_str(),
                kind: self.kind(),
            }),
            SpecStatus::Deleted | SpecStatus::Deleting if self.allow_op_deleting(&operation) => {
                Ok(())
            }
            SpecStatus::Deleted | SpecStatus::Deleting => Err(SvcError::PendingDeletion {
                id: self.uuid_str(),
                kind: self.kind(),
            }),
            SpecStatus::Created(_) => Ok(()),
        }?;
        // start the requested operation (which also checks if it's a valid transition)
        self.start_update_op(registry, state, operation).await
    }

    /// Check if the object is free to be modified or if it's still busy
    fn busy(&self) -> Result<(), SvcError> {
        if self.dirty() {
            return Err(SvcError::StoreDirty {
                kind: self.kind(),
                id: self.uuid_str(),
            });
        }
        Ok(())
    }

    /// Start a create transaction
    fn start_create_op(&mut self, request: &Self::Create);
    /// Start a destroy transaction
    fn start_destroy_op(&mut self);
    /// Check if the object is dirty -> needs to be flushed to the persistent store
    fn dirty(&self) -> bool;
    /// Get the kind (for log messages)
    fn kind(&self) -> ResourceKind;
    /// Get the UUID as a string (for log messages)
    fn uuid_str(&self) -> String;
    /// Get the state of the object
    fn status(&self) -> SpecStatus<Self::Status>;
    /// Set the state of the object
    fn set_status(&mut self, state: SpecStatus<Self::Status>);
    /// When creating fails we might want to transition spec to deleting and clear the create op.
    fn fail_creating_to_deleting<O>(&mut self) -> Self
    where
        Self: SpecTransaction<O>,
    {
        self.set_status(SpecStatus::Deleting);
        self.clear_op();
        self.clone()
    }
    /// Check if the object is owned by another
    fn owned(&self) -> bool {
        false
    }
    /// Get a human readable list of owners
    fn owners(&self) -> Option<String> {
        None
    }
    /// Disown resource by owners
    fn disown(&mut self, _owner: &Self::Owners) {}
    /// Remove all owners from the resource
    fn disown_all(&mut self) {}
    /// Return the result of the pending operation, if any.
    fn operation_result(&self) -> Option<Option<bool>>;

    /// Start an update operation (not all resources support this currently)
    async fn start_update_op(
        &mut self,
        _registry: &Registry,
        _state: &Self::State,
        _operation: Self::UpdateOp,
    ) -> Result<(), SvcError> {
        unimplemented!();
    }
}

/// Operations are locked
#[async_trait::async_trait]
pub(crate) trait OperationSequenceGuard<
    T: AsOperationSequencer + Clone + Sync + Send + Debug + ResourceUid,
>
{
    /// Attempt to obtain a guard for the specified operation mode
    fn operation_guard_mode(&self, mode: OperationMode) -> Result<OperationGuardArc<T>, SvcError>;
    /// Attempt to obtain a guard for the specified operation mode
    fn operation_guard(&self) -> Result<OperationGuardArc<T>, SvcError> {
        self.operation_guard_mode(OperationMode::Exclusive)
    }
    /// Attempt to obtain a guard for the specified operation mode
    /// A few attempts are made with an async sleep in case something else is already running
    async fn operation_guard_mode_wait(
        &self,
        mode: OperationMode,
    ) -> Result<OperationGuardArc<T>, SvcError>;
    async fn operation_guard_wait(&self) -> Result<OperationGuardArc<T>, SvcError> {
        self.operation_guard_mode_wait(OperationMode::Exclusive)
            .await
    }
}

#[async_trait::async_trait]
impl<T: AsOperationSequencer + Clone + Sync + Send + Debug + ResourceUid> OperationSequenceGuard<T>
    for ResourceMutex<T>
{
    fn operation_guard_mode(&self, mode: OperationMode) -> Result<OperationGuardArc<T>, SvcError> {
        let get_value = |s: &Self| s.lock().clone();

        match OperationGuardArc::try_sequence(self, get_value, mode) {
            Ok(guard) => Ok(guard),
            Err((error, log)) => {
                if log {
                    tracing::trace!("Resource '{}' is busy: {}", self.lock().uid_str(), error);
                }
                Err(SvcError::Conflict {})
            }
        }
    }
    async fn operation_guard_mode_wait(
        &self,
        mode: OperationMode,
    ) -> Result<OperationGuardArc<T>, SvcError> {
        let mut tries = 5;
        loop {
            tries -= 1;
            match self.operation_guard_mode(mode) {
                Ok(guard) => return Ok(guard),
                Err(error) if tries == 0 => {
                    return Err(error);
                }
                Err(_) => {}
            };

            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }
}

/// Locked Resource Specs.
#[derive(Default, Clone, Debug)]
pub(crate) struct ResourceSpecsLocked(Arc<RwLock<ResourceSpecs>>);

impl Deref for ResourceSpecsLocked {
    type Target = Arc<RwLock<ResourceSpecs>>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Resource Specs.
#[derive(Default, Debug)]
pub(crate) struct ResourceSpecs {
    pub(crate) volumes: ResourceMutexMap<VolumeId, VolumeSpec>,
    pub(crate) nodes: ResourceMutexMap<NodeId, NodeSpec>,
    pub(crate) nexuses: ResourceMutexMap<NexusId, NexusSpec>,
    pub(crate) pools: ResourceMutexMap<PoolId, PoolSpec>,
    pub(crate) replicas: ResourceMutexMap<ReplicaId, ReplicaSpec>,
    pub(crate) affinity_groups: ResourceMutexMap<String, AffinityGroupSpec>,
    /// Top-level volume snapshots.
    pub(crate) volume_snapshots: ResourceMutexMap<SnapshotId, VolumeSnapshot>,
    pub(crate) app_nodes: ResourceMutexMap<AppNodeId, AppNodeSpec>,
}

impl ResourceSpecsLocked {
    pub(crate) fn new() -> Self {
        ResourceSpecsLocked::default()
    }

    /// Initialise the resource specs with the content from the persistent store.
    pub(crate) async fn init<S: Store>(
        &self,
        store: &mut S,
        legacy_prefix_present: bool,
        etcd_max_page_size: i64,
    ) -> Result<(), SvcError> {
        let spec_types = [
            StorableObjectType::VolumeSpec,
            StorableObjectType::NodeSpec,
            StorableObjectType::NexusSpec,
            StorableObjectType::PoolSpec,
            StorableObjectType::ReplicaSpec,
            StorableObjectType::VolumeSnapshot,
            StorableObjectType::AppNodeSpec,
        ];
        for spec in &spec_types {
            self.populate_specs(store, *spec, legacy_prefix_present, etcd_max_page_size)
                .await
                .map_err(|error| SvcError::Internal {
                    details: error.full_string(),
                })?;
        }

        // patch up the missing replica nexus owners
        for replica in self.read().replicas.values() {
            self.read()
                .nexuses
                .values()
                .filter(|n| n.lock().contains_replica(replica.uuid()))
                .for_each(|n| replica.lock().owners.add_owner(&n.lock().uuid));
        }
        // add runtime information for volume snapshots
        for snapshot in self.read().volume_snapshots.values() {
            let volume_id = snapshot.immutable_ref().spec().source_id();
            if let Some(volume) = self.read().volumes.get(volume_id) {
                volume.lock().insert_snapshot(snapshot.uuid());
            }
        }

        // add runtime information for volume restores
        for volume in self.read().volumes.values() {
            match volume.immutable_ref().content_source.as_ref() {
                None => continue,
                Some(VolumeContentSource::Snapshot(snap_uuid, _)) => {
                    if let Some(snapshot) = self.read().volume_snapshots.get(snap_uuid) {
                        snapshot.lock().insert_restore(volume.uuid())
                    }
                }
            }
        }

        // Remove all entries of v1 key prefix.
        store
            .delete_values_prefix(&product_v1_key_prefix())
            .await
            .map_err(|error| StoreError::Generic {
                source: Box::new(error),
                description: "Product v1 prefix cleanup failed".to_string(),
            })?;
        Ok(())
    }

    /// Deserialise a vector of serde_json values into specific spec types.
    /// If deserialisation fails for any object, return an error.
    fn deserialise_specs<T>(values: Vec<serde_json::Value>) -> Result<Vec<T>, serde_json::Error>
    where
        T: DeserializeOwned,
    {
        let specs: Vec<Result<T, serde_json::Error>> = values
            .iter()
            .map(|v| serde_json::from_value(v.clone()))
            .collect();

        let mut result = vec![];
        for spec in specs {
            match spec {
                Ok(s) => {
                    result.push(s);
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }
        Ok(result)
    }

    /// Populate the resource specs with data from the persistent store.
    async fn populate_specs<S: Store>(
        &self,
        store: &mut S,
        spec_type: StorableObjectType,
        legacy_prefix_present: bool,
        etcd_max_page_size: i64,
    ) -> Result<(), SpecError> {
        if legacy_prefix_present {
            migrate_product_v1_to_v2(store, spec_type, etcd_max_page_size)
                .await
                .map_err(|e| SpecError::StoreMigrate {
                    source: Box::new(e),
                })?;
        }
        let prefix = key_prefix_obj(spec_type, API_VERSION);
        let store_entries = store
            .get_values_paged_all(&prefix, etcd_max_page_size)
            .await
            .map_err(|e| SpecError::StoreGet {
                source: Box::new(e),
            })?;
        let store_values = store_entries.iter().map(|e| e.1.clone()).collect();

        let mut resource_specs = self.0.write();
        match spec_type {
            StorableObjectType::VolumeSpec => {
                let specs =
                    Self::deserialise_specs::<VolumeSpec>(store_values).context(Deserialise {
                        obj_type: StorableObjectType::VolumeSpec,
                    })?;
                let ag_specs = get_affinity_group_specs(&specs);
                resource_specs.volumes.populate(specs);
                // Load the ag specs in memory, ag specs are not persisted in memory so we don't
                // have a StorableObjectType for it.
                resource_specs.affinity_groups.populate(ag_specs);
            }
            StorableObjectType::NodeSpec => {
                let specs =
                    Self::deserialise_specs::<NodeSpec>(store_values).context(Deserialise {
                        obj_type: StorableObjectType::NodeSpec,
                    })?;
                resource_specs.nodes.populate(specs);
            }
            StorableObjectType::NexusSpec => {
                let specs =
                    Self::deserialise_specs::<NexusSpec>(store_values).context(Deserialise {
                        obj_type: StorableObjectType::NexusSpec,
                    })?;
                resource_specs.nexuses.populate(specs);
            }
            StorableObjectType::PoolSpec => {
                let specs =
                    Self::deserialise_specs::<PoolSpec>(store_values).context(Deserialise {
                        obj_type: StorableObjectType::PoolSpec,
                    })?;
                resource_specs.pools.populate(specs);
            }
            StorableObjectType::ReplicaSpec => {
                let specs =
                    Self::deserialise_specs::<ReplicaSpec>(store_values).context(Deserialise {
                        obj_type: StorableObjectType::ReplicaSpec,
                    })?;
                resource_specs.replicas.populate(specs);
            }
            StorableObjectType::VolumeSnapshot => {
                let specs = Self::deserialise_specs::<VolumeSnapshot>(store_values).context(
                    Deserialise {
                        obj_type: StorableObjectType::VolumeSnapshot,
                    },
                )?;
                resource_specs.volume_snapshots.populate(specs);
            }
            StorableObjectType::AppNodeSpec => {
                let specs =
                    Self::deserialise_specs::<AppNodeSpec>(store_values).context(Deserialise {
                        obj_type: StorableObjectType::AppNodeSpec,
                    })?;
                resource_specs.app_nodes.populate(specs);
            }
            _ => {
                // Not all spec types are persisted in the store.
                unimplemented!("{} not persisted in store", spec_type);
            }
        };
        Ok(())
    }
}

/// Helper function to extract the affinity groups from volumes on startup.
fn get_affinity_group_specs(volume_specs: &[VolumeSpec]) -> Vec<AffinityGroupSpec> {
    let mut affinity_group_specs: Vec<AffinityGroupSpec> = Vec::new();
    for volume_spec in volume_specs {
        if let Some(affinity_group) = &volume_spec.affinity_group {
            if let Some(existing_affinity_group_spec) = affinity_group_specs
                .iter_mut()
                .find(|spec| spec.id() == affinity_group.id())
            {
                existing_affinity_group_spec.append(volume_spec.uuid.clone());
            } else {
                let affinity_group_spec = AffinityGroupSpec::new(
                    affinity_group.id().clone(),
                    vec![volume_spec.uuid.clone()],
                );
                affinity_group_specs.push(affinity_group_spec);
            }
        }
    }
    affinity_group_specs
}
