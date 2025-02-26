/// All errors that can be returned from the pstor.
#[derive(Debug, snafu::Snafu)]
#[snafu(visibility(pub), context(suffix(false)))]
pub enum Error {
    /// Failed to connect to the key-value store.
    #[snafu(display("Failed to connect to store. Error {}", source))]
    Connect { source: etcd_client::Error },
    /// Failed to 'put' an entry in the store.
    #[snafu(display(
        "Failed to 'put' entry with key {} and value {:?}. Error {}",
        key,
        value,
        source
    ))]
    Put {
        key: String,
        value: String,
        source: etcd_client::Error,
    },
    /// Failed to 'get' an entry from the store.
    #[snafu(display("Failed to 'get' entry with key {}. Error {}", key, source))]
    Get {
        key: String,
        source: etcd_client::Error,
    },
    /// Failed to 'get' an entry, with the given prefix, from the store.
    #[snafu(display("Failed to 'get' entry with prefix {}. Error {}", prefix, source))]
    GetPrefix {
        prefix: String,
        source: etcd_client::Error,
    },
    /// Failed to 'delete' entry, with the given prefix, from the store.
    #[snafu(display("Failed to 'delete' entry with prefix {}. Error {}", prefix, source))]
    DeletePrefix {
        prefix: String,
        source: etcd_client::Error,
    },
    /// Failed to find an entry with the given key.
    #[snafu(display("Entry with key {} not found.", key))]
    MissingEntry { key: String },
    /// The resource id is not present in the key.
    #[snafu(display("Invalid key {}, resource id not found.", key))]
    InvalidKey { key: String },
    /// Failed to 'delete' an entry from the store.
    #[snafu(display("Failed to 'delete' entry with key {}. Error {}", key, source))]
    Delete {
        key: String,
        source: etcd_client::Error,
    },
    /// Failed to 'watch' an entry in the store.
    #[snafu(display("Failed to 'watch' entry with key {}. Error {}", key, source))]
    Watch {
        key: String,
        source: etcd_client::Error,
    },
    /// Empty key.
    #[snafu(display("Failed to get key as string. Error {}", source))]
    KeyString { source: etcd_client::Error },
    /// Empty value.
    #[snafu(display("Failed to get value as string. Error {}", source))]
    ValueString { source: etcd_client::Error },
    /// Failed to deserialise value.
    #[snafu(display("Failed to deserialise value {}. Error {}", value, source))]
    DeserialiseValue {
        value: String,
        source: serde_json::Error,
    },
    /// Failed to serialise value.
    #[snafu(display("Failed to serialise value. Error {}", source))]
    SerialiseValue { source: serde_json::Error },
    /// Failed to run operation within a timeout.
    #[snafu(display("Timed out during {} operation after {:?}", operation, timeout))]
    Timeout {
        operation: String,
        timeout: std::time::Duration,
    },
    #[snafu(display("Failed to grab the lease lock, reason: '{}'", reason))]
    FailedLock { reason: String },
    #[snafu(display("Failed to unlock the lease. Error {}", source))]
    FailedUnlock { source: etcd_client::Error },
    #[snafu(display("Etcd is not ready, reason: '{}'", reason))]
    NotReady { reason: String },
    #[snafu(display("Minimum paged value is 2"))]
    PagedMinimum,
    Generic {
        source: Box<Error>,
        description: String,
    },
    #[snafu(display("Failed to parse range end for start key: '{}'", start_key))]
    RangeEnd { start_key: String },
    #[snafu(display("The Watcher has already been closed: this is a logical bug."))]
    WatcherClosed {},
}
