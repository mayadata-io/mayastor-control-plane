//! The persistent stor is an interface to a datastore which suits the projects needs.
//! We may have various implementations, depending on the platform we're running on.

/// Error exposed by the pstor.
pub mod error;
/// Export error module.
pub use error::Error;

/// The stor interface.
mod api;

/// A particular implementation of the persistent store, using ETCd.
pub mod etcd;
mod etcd_keep_alive;

/// Definition for the StorableObjectType.
mod common;

mod etcd_watcher;
/// The product specific modules.
mod products;

/// Export pstor module.
pub use api::{
    ObjectKey, StorableObject, Store, StoreKey, StoreKv, StoreKvWatcher, StoreObj,
    StoreWatchReceiver, WatchCtx, WatchEvent, WatchKey, WatchResult,
};
pub use common::{ApiVersion, StorableObjectType};
pub use products::{
    v1::{
        detect_product_v1_prefix, key_prefix as product_v1_key_prefix,
        key_prefix_obj as product_v1_key_prefix_obj,
    },
    v2::{build_key_prefix, key_prefix, key_prefix_obj, API_VERSION},
};
