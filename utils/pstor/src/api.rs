use crate::{common::ApiVersion, products::v2::generate_key, Error};
use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use tokio::sync::mpsc::Receiver;

/// Trait defining the operations that can be performed on a key-value store.
#[async_trait]
pub trait Store: StoreKv + StoreObj + Sync + Send + Clone {
    async fn online(&mut self) -> bool;
}

/// Trait defining the operations that can be performed on a key-value store.
/// This is strictly intended for a KV type access.
#[async_trait]
pub trait StoreKv: Sync + Send + Clone {
    /// Puts the given `V` value into the under the given `K` key.
    async fn put_kv<K: StoreKey, V: StoreValue>(&mut self, key: &K, value: &V)
        -> Result<(), Error>;
    /// Get the value from the given `K` key entry from the store.
    async fn get_kv<K: StoreKey>(&mut self, key: &K) -> Result<Value, Error>;
    /// Deletes the given `K` key entry from the store.
    async fn delete_kv<K: StoreKey>(&mut self, key: &K) -> Result<(), Error>;
    /// Watches for changes under the given `K` key entry.
    /// Returns a channel which is signalled when an event occurs.
    /// # Warning: Events may be lost if we are restarted.
    async fn watch_kv<K: StoreKey>(&mut self, key: &K) -> Result<StoreWatchReceiver, Error>;

    /// Returns a vector of tuples. Each tuple represents a key-value pair.
    async fn get_values_prefix(&mut self, key_prefix: &str) -> Result<Vec<(String, Value)>, Error>;
    /// Returns a vector of tuples. Each tuple represents a key-value pair.
    async fn get_values_paged(
        &mut self,
        key_prefix: &str,
        limit: i64,
        range_end: &str,
    ) -> Result<Vec<(String, Value)>, Error>;
    /// Returns a vector of tuples. Each tuple represents a key-value pair. It paginates through all
    /// the values for the prefix with limit.
    async fn get_values_paged_all(
        &mut self,
        key_prefix: &str,
        limit: i64,
    ) -> Result<Vec<(String, Value)>, Error>;
    /// Deletes all key values from a given prefix.
    async fn delete_values_prefix(&mut self, key_prefix: &str) -> Result<(), Error>;
    /// Get a StoreKv watcher.
    fn kv_watcher<Ctx: WatcherCtx, W: WatchCallback<Ctx>>(
        &self,
        ctx_cb: W,
    ) -> impl StoreKvWatcher<Ctx> + 'static;
}

/// Trait defining the operations that can be performed on a key-value store using object semantics.
/// It allows for abstracting the key component into the `StorableObject` itself.
#[async_trait]
pub trait StoreObj: StoreKv + Sync + Send + Clone {
    /// Puts the given `O` object into the store.
    async fn put_obj<O: StorableObject>(&mut self, object: &O) -> Result<(), Error>;
    /// Gets the object `O` through its `O::Key`.
    async fn get_obj<O: StorableObject>(&mut self, _key: &O::Key) -> Result<O, Error>;
    /// Watches for changes under the given `K` object key entry.
    /// Returns a channel which is signalled when an event occurs.
    /// # Warning: Events may be lost if we are restarted.
    async fn watch_obj<K: ObjectKey>(&mut self, key: &K) -> Result<StoreWatchReceiver, Error>;
}

/// Watch key used to register a watch for a given key prefix.
pub struct WatchKey {
    /// The key prefix to watch for.
    pub(crate) prefix: String,
    /// The revision number to watch from, if set.
    pub(crate) rev: Option<i64>,
}
impl WatchKey {
    /// Create a new `Self` with the given key prefix.
    pub fn new(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_string(),
            rev: None,
        }
    }
    /// Specify the revision to start the watch from.
    pub fn with_rev(self, rev: Option<i64>) -> Self {
        Self { rev, ..self }
    }
}
/// The watch context which can be used by the registrants for bookkeeping.
pub struct WatchCtx {
    /// A uuid v4 for bookkeeping.
    pub uuid: uuid::Uuid,
}

/// Callback may request for the respective registrant key to be unwatched.
pub enum WatchResult {
    /// Continue watching for further updates.
    Continue,
    /// Remove this key from the watch.
    Remove,
}
/// Callback arguments.
pub struct WatchCbArg<'a, Ctx> {
    /// The registered key prefix.
    pub key_prefix: &'a str,
    /// The registered per-callback context.
    pub cb_ctx: &'a Ctx,
    /// The actual key which was updated (which contains the prefix of `key_prefix`).
    pub updated_key: &'a str,
}
/// Watch callback function to be triggered on update events for `WatchKey`.
pub trait WatchCallback<Ctx: WatcherCtx>:
    Fn(WatchCbArg<Ctx>) -> WatchResult + Send + Sync + 'static
{
}
impl<T, Ctx: WatcherCtx> WatchCallback<Ctx> for T where
    T: Fn(WatchCbArg<Ctx>) -> WatchResult + Send + Sync + 'static
{
}

/// Trait defining the operations that can be performed on a key-value store using object semantics.
/// It allows for abstracting the key component into the `StorableObject` itself.
pub trait StoreKvWatcher<Ctx: WatcherCtx> {
    /// Watch for the key prefix specified in the [`WatchKey`].
    /// Callbacks are received by the global [`WatchCallback`].
    fn watch(&self, key: WatchKey, ctx: Ctx) -> Result<(), Error>;
    /// Stop watching the previously registered Watch.
    fn unwatch(&self, key: WatchKey) -> Result<(), Error>;
    /// Cancel all watches.
    fn abort(self);
}

/// Per watch context for [`StoreKvWatcher`] callbacks.
pub trait WatcherCtx: Send + Sync + 'static {}
impl<T> WatcherCtx for T where T: Send + Sync + 'static {}

/// Store keys type trait.
pub trait StoreKey: Sync + ToString {}
impl<T> StoreKey for T where T: Sync + ToString {}
/// Store value type trait.
pub trait StoreValue: Sync + serde::Serialize {}
impl<T> StoreValue for T where T: Sync + serde::Serialize {}
/// Representation of a watch event.
#[derive(Debug)]
pub enum WatchEvent {
    /// Put operation containing the key and value.
    Put(String, Value),
    /// Delete operation.
    Delete,
}
/// Channel used to receive events from a watch setup through `StoreKv::watch_kv`.
pub type StoreWatchReceiver = Receiver<Result<WatchEvent, Error>>;

/// Implemented by Keys of Storable Objects.
pub trait ObjectKey: Sync + Send {
    type Kind: AsRef<str>;

    fn key(&self) -> String {
        generate_key(self)
    }
    fn version(&self) -> ApiVersion;
    fn key_type(&self) -> Self::Kind;
    fn key_uuid(&self) -> String;
}
/// Implemented by objects which get stored in the store.
#[async_trait]
pub trait StorableObject: Serialize + Sync + Send + DeserializeOwned {
    type Key: ObjectKey;

    fn key(&self) -> Self::Key;
}
