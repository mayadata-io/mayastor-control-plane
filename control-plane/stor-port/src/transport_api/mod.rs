#![warn(missing_docs)]
//! All the different messages which can be sent/received to/from the control
//! plane services and io-engine.
//! We could split these out further into categories when they start to grow.

/// Send messages traits.
pub mod macros;
/// Version 0 of the messages.
pub mod v0;

use crate::types::v0::transport::{HostNqnParseError, MessageIdVs, VERSION};
use async_trait::async_trait;
use dyn_clonable::clonable;

use serde::{de::StdError, Deserialize, Serialize};

use std::{fmt::Debug, num::TryFromIntError, str::FromStr, time::Duration};
use strum_macros::{AsRefStr, Display};
use tokio::task::JoinError;
use tonic::Code;

/// Report error chain.
pub trait ErrorChain {
    /// Full error chain as a string separated by ':'.
    fn full_string(&self) -> String;
    /// Get the full error chain starting from the parent.
    fn parent_full_string(&self) -> String;
}

impl<T> ErrorChain for T
where
    T: std::error::Error,
{
    /// loops through the error chain and formats into a single string
    /// containing all the lower level errors.
    fn full_string(&self) -> String {
        let mut msg = format!("{self}");
        let mut opt_source = self.source();
        while let Some(source) = opt_source {
            msg = format!("{msg}: {source}");
            opt_source = source.source();
        }
        msg
    }

    fn parent_full_string(&self) -> String {
        match self.source() {
            Some(parent) => parent.full_string(),
            None => String::new(),
        }
    }
}

/// Message id which uniquely identifies every type of unsolicited message.
/// The solicited (replies) message do not currently carry an id as they
/// are sent to a specific requested channel.
#[derive(Debug, PartialEq, Clone)]
#[allow(non_camel_case_types)]
pub enum MessageId {
    /// Version 0.
    v0(MessageIdVs),
}

impl Serialize for MessageId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.to_string().as_str())
    }
}
impl<'de> Deserialize<'de> for MessageId {
    fn deserialize<D>(deserializer: D) -> Result<MessageId, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let string = String::deserialize(deserializer)?;
        match string.parse() {
            Ok(id) => Ok(id),
            Err(error) => {
                let error = format!("Failed to parse into MessageId, error: {error}");
                Err(serde::de::Error::custom(error))
            }
        }
    }
}

impl FromStr for MessageId {
    type Err = strum::ParseError;

    fn from_str(source: &str) -> Result<Self, Self::Err> {
        match source.split('/').next() {
            Some(VERSION) => {
                let id: MessageIdVs = source[VERSION.len() + 1 ..].parse()?;
                Ok(Self::v0(id))
            }
            _ => Err(strum::ParseError::VariantNotFound),
        }
    }
}
impl std::fmt::Display for MessageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::v0(id) => write!(f, "{VERSION}/{id}"),
        }
    }
}

/// This trait defines all Transport Messages which must:
/// 1 - be uniquely identifiable via MessageId.
#[async_trait]
pub trait Message {
    /// identification of this object according to the `MessageId`.
    fn id(&self) -> MessageId;
}

/// All the different variants of Resources.
#[derive(Serialize, Deserialize, Debug, Clone, AsRefStr, Display, Eq, PartialEq)]
pub enum ResourceKind {
    /// Unknown or unspecified resource.
    Unknown,
    /// Node resource.
    Node,
    /// Pool resource.
    Pool,
    /// Replica resource.
    Replica,
    /// Replica state.
    ReplicaState,
    /// Replica spec.
    ReplicaSpec,
    /// Replica snapshot.
    ReplicaSnapshot,
    /// Replica snapshot clone.
    ReplicaSnapshotClone,
    /// Nexus resource.
    Nexus,
    /// Child resource.
    Child,
    /// Volume resource.
    Volume,
    /// Volume snapshot.
    VolumeSnapshot,
    /// Volume snapshot clone.
    VolumeSnapshotClone,
    /// Json Grpc methods.
    JsonGrpc,
    /// Block devices.
    Block,
    /// Watch.
    Watch,
    /// Spec.
    Spec,
    /// State.
    State,
    /// Nvme Subsystem.
    NvmeSubsystem,
    /// Nvme Controller Path.
    NvmePath,
    /// Affinity Group.
    AffinityGroup,
    /// App Node.
    AppNode,
}

/// Error type which is returned over the transport for any operation.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReplyError {
    /// error kind.
    pub kind: ReplyErrorKind,
    /// resource kind.
    pub resource: ResourceKind,
    /// last source of this error.
    pub source: String,
    /// extra information.
    pub extra: String,
}

impl From<HostNqnParseError> for ReplyError {
    fn from(error: HostNqnParseError) -> Self {
        Self::invalid_argument(ResourceKind::Unknown, "allowed_host", format!("{error:?}"))
    }
}

impl From<tonic::Status> for ReplyError {
    fn from(status: tonic::Status) -> Self {
        Self::tonic_reply_error(status.code().into(), status.to_string(), String::new())
    }
}

impl From<ReplyError> for tonic::Status {
    fn from(error: ReplyError) -> Self {
        match error.kind {
            ReplyErrorKind::InvalidArgument => tonic::Status::invalid_argument(error.full_string()),
            ReplyErrorKind::DeadlineExceeded => {
                tonic::Status::deadline_exceeded(error.full_string())
            }
            ReplyErrorKind::FailedPrecondition => {
                tonic::Status::failed_precondition(error.full_string())
            }
            ReplyErrorKind::FailedPersist => {
                tonic::Status::failed_precondition(error.full_string())
            }
            ReplyErrorKind::AlreadyExists => tonic::Status::already_exists(error.full_string()),
            ReplyErrorKind::Aborted => tonic::Status::aborted(error.full_string()),
            ReplyErrorKind::NotFound => tonic::Status::not_found(error.full_string()),
            ReplyErrorKind::ResourceExhausted => {
                tonic::Status::resource_exhausted(error.full_string())
            }
            ReplyErrorKind::Unimplemented => tonic::Status::unimplemented(error.full_string()),
            _ => tonic::Status::internal(error.full_string()),
        }
    }
}

impl From<tonic::transport::Error> for ReplyError {
    fn from(e: tonic::transport::Error) -> Self {
        Self::tonic_reply_error(ReplyErrorKind::Aborted, e.to_string(), String::new())
    }
}

/// Error type for invalid integer type conversion
impl From<TryFromIntError> for ReplyError {
    fn from(e: TryFromIntError) -> Self {
        Self::invalid_argument(ResourceKind::Unknown, "", e.to_string())
    }
}

impl From<JoinError> for ReplyError {
    fn from(error: JoinError) -> Self {
        Self::aborted_error(error)
    }
}

impl StdError for ReplyError {}
impl ReplyError {
    /// Extend error with source.
    /// Useful when another error wraps around a `ReplyError` and we want to
    /// convert back to `ReplyError` so we can send it over the wire.
    pub fn extend(&mut self, source: &str, extra: &str) {
        self.source = format!("{}::{}", source, self.source);
        self.extra = format!("{}::{}", extra, self.extra);
    }
    /// Useful when the grpc server is dropped due to panic.
    pub fn aborted_error(error: JoinError) -> Self {
        Self {
            kind: ReplyErrorKind::Aborted,
            resource: ResourceKind::Unknown,
            source: error.to_string(),
            extra: "Failed to wait for thread".to_string(),
        }
    }
    /// Useful when the grpc server is dropped due to panic.
    pub fn tonic_reply_error(kind: ReplyErrorKind, source: String, extra: String) -> Self {
        Self {
            kind,
            resource: ResourceKind::Unknown,
            source,
            extra,
        }
    }
    /// Used only for testing, not used in code.
    pub fn invalid_reply_error(msg: String) -> Self {
        Self {
            kind: ReplyErrorKind::Aborted,
            resource: ResourceKind::Unknown,
            source: "Test Library".to_string(),
            extra: msg,
        }
    }
    /// Used when we get an empty response from the grpc server.
    pub fn invalid_response(resource: ResourceKind) -> Self {
        Self {
            kind: ReplyErrorKind::Aborted,
            resource,
            source: "Empty response reply received from grpc server".to_string(),
            extra: "".to_string(),
        }
    }
    /// Used when we get an invalid argument.
    pub fn invalid_argument(resource: ResourceKind, arg_name: &str, error: impl ToString) -> Self {
        Self {
            kind: ReplyErrorKind::InvalidArgument,
            resource,
            source: error.to_string(),
            extra: format!("Invalid {arg_name} was provided"),
        }
    }
    /// Used when we encounter a missing argument.
    pub fn missing_argument(resource: ResourceKind, arg_name: &str) -> Self {
        Self {
            kind: ReplyErrorKind::InvalidArgument,
            resource,
            source: arg_name.to_string(),
            extra: format!("Argument {arg_name} was not provided"),
        }
    }
    /// Failed to persist.
    pub fn failed_persist(resource: ResourceKind, source: String, extra: String) -> Self {
        Self {
            kind: ReplyErrorKind::FailedPersist,
            resource,
            source,
            extra,
        }
    }
    /// For errors that can occur when serializing or deserializing JSON data.
    pub fn serde_error(
        resource: ResourceKind,
        error_kind: ReplyErrorKind,
        error: serde_json::Error,
    ) -> Self {
        Self {
            kind: error_kind,
            resource,
            source: error.to_string(),
            extra: "".to_string(),
        }
    }
    /// For errors that represent unimplemented functionality.
    pub fn unimplemented(msg: String) -> Self {
        Self {
            kind: ReplyErrorKind::Unimplemented,
            resource: ResourceKind::Unknown,
            source: "Test Library".to_string(),
            extra: msg,
        }
    }
    /// For internal errors.
    pub fn internal_error(resource: ResourceKind, source: String, extra: String) -> Self {
        Self {
            kind: ReplyErrorKind::Internal,
            resource,
            source,
            extra,
        }
    }

    /// For deadline exceeded errors.
    pub fn deadline_exceeded(resource: ResourceKind, source: String, extra: String) -> Self {
        Self {
            kind: ReplyErrorKind::DeadlineExceeded,
            resource,
            source,
            extra,
        }
    }

    /// For failed precondition errors.
    pub fn failed_precondition(resource: ResourceKind, source: String, extra: String) -> Self {
        Self {
            kind: ReplyErrorKind::FailedPrecondition,
            resource,
            source,
            extra,
        }
    }

    /// For already exist errors.
    pub fn already_exist(resource: ResourceKind, source: String, extra: String) -> Self {
        Self {
            kind: ReplyErrorKind::AlreadyExists,
            resource,
            source,
            extra,
        }
    }

    /// For not found errors.
    pub fn not_found(resource: ResourceKind, source: String, extra: String) -> Self {
        Self {
            kind: ReplyErrorKind::NotFound,
            resource,
            source,
            extra,
        }
    }

    /// For resource exhausted errors.
    pub fn resource_exhausted(resource: ResourceKind, source: String, extra: String) -> Self {
        Self {
            kind: ReplyErrorKind::ResourceExhausted,
            resource,
            source,
            extra,
        }
    }
}

impl std::fmt::Display for ReplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{}: {}{}",
            self.kind.as_ref(),
            if matches!(self.resource, ResourceKind::Unknown) {
                String::new()
            } else {
                format!("/{}", self.resource.as_ref())
            },
            self.source,
            if !self.extra.is_empty() {
                format!(": {}", self.extra)
            } else {
                String::new()
            }
        )
    }
}

/// All the different variants of `ReplyError`.
#[derive(Serialize, Deserialize, Debug, Clone, strum_macros::AsRefStr, Eq, PartialEq)]
#[allow(missing_docs)]
pub enum ReplyErrorKind {
    WithMessage,
    DeserializeReq,
    Internal,
    Timeout,
    InvalidArgument,
    DeadlineExceeded,
    NotFound,
    AlreadyExists,
    PermissionDenied,
    ResourceExhausted,
    FailedPrecondition,
    Aborted,
    OutOfRange,
    Unimplemented,
    Unavailable,
    Unauthenticated,
    Unauthorized,
    Conflict,
    FailedPersist,
    NotShared,
    AlreadyShared,
    NotPublished,
    AlreadyPublished,
    Deleting,
    ReplicaCountAchieved,
    ReplicaChangeCount,
    ReplicaIncrease,
    ReplicaCreateNumber,
    VolumeNoReplicas,
    InUse,
    CapacityLimitExceeded,
    NotAcceptable,
    Cancelled,
}

impl From<tonic::Code> for ReplyErrorKind {
    fn from(code: tonic::Code) -> Self {
        match code {
            Code::Ok => Self::Internal,
            Code::Unknown => Self::Internal,
            Code::InvalidArgument => Self::InvalidArgument,
            Code::DeadlineExceeded => Self::DeadlineExceeded,
            Code::NotFound => Self::NotFound,
            Code::AlreadyExists => Self::AlreadyExists,
            Code::PermissionDenied => Self::PermissionDenied,
            Code::ResourceExhausted => Self::ResourceExhausted,
            Code::FailedPrecondition => Self::FailedPrecondition,
            Code::Aborted => Self::Aborted,
            Code::OutOfRange => Self::OutOfRange,
            Code::Unimplemented => Self::Unimplemented,
            Code::Internal => Self::Internal,
            Code::Unavailable => Self::Unavailable,
            Code::DataLoss => Self::FailedPersist,
            Code::Unauthenticated => Self::Unauthenticated,
            Code::Cancelled => Self::Cancelled,
        }
    }
}

/// Save on typing.
pub type DynClient = Box<dyn ClientOpts>;

/// Timeout for receiving a reply to a request message
/// Max number of retries until it gives up.
#[derive(Clone, Debug)]
pub struct TimeoutOptions {
    /// initial request message timeout.
    pub(crate) request_timeout: std::time::Duration,
    /// request message incremental timeout step.
    pub(crate) timeout_step: std::time::Duration,
    /// max number of retries following the initial attempt's timeout.
    pub(crate) max_retries: Option<u32>,
    /// Server tcp read timeout when no messages are received.
    /// Shen this timeout is triggered we attempt to send a Ping to the server. If a Pong is not
    /// received within the same timeout the nats client disconnects from the server.
    tcp_read_timeout: std::time::Duration,

    /// Request specific minimum timeouts.
    request_min_timeout: Option<RequestMinTimeout>,
    /// Connect timeout.
    pub connect_timeout: std::time::Duration,

    /// Http2 keep alive interval.
    keep_alive_interval: std::time::Duration,
    /// Http2 keep alive timeout.
    keep_alive_timeout: std::time::Duration,

    client: ClientId,
}

/// Request specific minimum timeouts.
/// zeroing replicas on create/destroy takes some time (observed up to 7seconds).
/// nexus creation by itself can take up to 4 seconds... it can take even longer if etcd is not up.
#[derive(Debug, Clone)]
pub struct RequestMinTimeout {
    replica: Duration,
    replica_snapshot: Duration,
    nexus: Duration,
    pool: Duration,
    nexus_shutdown: Duration,
    nexus_snapshot: Duration,
    nvme_reconnect: Duration,
}

impl Default for RequestMinTimeout {
    fn default() -> Self {
        Self {
            replica: Duration::from_secs(15),
            replica_snapshot: Duration::from_secs(10),
            nexus: Duration::from_secs(30),
            pool: Duration::from_secs(20),
            nexus_shutdown: Duration::from_secs(15),
            nexus_snapshot: Duration::from_secs(30),
            nvme_reconnect: Duration::from_secs(62),
        }
    }
}
impl RequestMinTimeout {
    /// Minimum timeout for a replica operation.
    pub fn replica(&self) -> Duration {
        self.replica
    }
    /// Minimum timeout for a replica snapshot operation.
    pub fn replica_snapshot(&self) -> Duration {
        self.replica_snapshot
    }
    /// Minimum timeout for a nexus operation.
    pub fn nexus(&self) -> Duration {
        self.nexus
    }
    /// Minimum timeout for a nexus snapshot operation.
    pub fn nexus_snapshot(&self) -> Duration {
        self.nexus_snapshot
    }
    /// Minimum timeout for a volume snapshot operation.
    pub fn volume_snapshot(&self) -> Duration {
        // not quite sure how much slack to give here, maybe this is enough?
        self.nexus_snapshot + self.replica_snapshot
    }
    /// Minimum timeout for a pool operation.
    pub fn pool(&self) -> Duration {
        self.pool
    }
    /// Minimum timeout for nexus shutdown.
    pub fn nexus_shutdown(&self) -> Duration {
        self.nexus_shutdown
    }
    /// Minimum timeout for nvme reconnect.
    pub fn nvme_reconnect(&self) -> Duration {
        self.nvme_reconnect
    }
}

impl TimeoutOptions {
    /// Default timeout waiting for a reply.
    pub(crate) fn default_timeout() -> Duration {
        Duration::from_secs(6)
    }
    /// Default time between retries when a timeout is hit.
    pub(crate) fn default_timeout_step() -> Duration {
        Duration::from_secs(1)
    }
    /// Default connect timeout.
    pub(crate) fn default_connect_timeout() -> Duration {
        Duration::from_secs(1)
    }
    /// Default max number of retries until the request is given up on.
    pub(crate) fn default_max_retries() -> u32 {
        0
    }
    /// Default `RequestMinTimeout` which specified timeouts for specific operations.
    pub(crate) fn default_min_request_timeouts() -> Option<RequestMinTimeout> {
        Some(RequestMinTimeout::default())
    }
    /// Default Server tcp read timeout when no messages are received.
    pub(crate) fn default_tcp_read_timeout() -> Duration {
        Duration::from_secs(6)
    }
    /// Get the tcp read timeout.
    pub fn tcp_read_timeout(&self) -> Duration {
        self.tcp_read_timeout
    }
    /// Get the base timeout.
    pub fn base_timeout(&self) -> Duration {
        self.request_timeout
    }
    /// Get the base timeout.
    pub fn backoff_timeout(&self) -> Duration {
        self.timeout_step
    }
    /// Default http2 Keep Alive interval.
    pub(crate) fn default_keep_alive_interval() -> std::time::Duration {
        Duration::from_secs(10)
    }
    /// Default http2 Keep Alive timeout.
    pub(crate) fn default_keep_alive_timeout() -> std::time::Duration {
        Duration::from_secs(20)
    }
}

impl Default for TimeoutOptions {
    fn default() -> Self {
        Self {
            request_timeout: Self::default_timeout(),
            timeout_step: Self::default_timeout_step(),
            max_retries: Some(Self::default_max_retries()),
            tcp_read_timeout: Self::default_tcp_read_timeout(),
            request_min_timeout: Self::default_min_request_timeouts(),
            keep_alive_timeout: Self::default_keep_alive_timeout(),
            keep_alive_interval: Self::default_keep_alive_interval(),
            client: ClientId::Unnamed,
            connect_timeout: Self::default_connect_timeout(),
        }
    }
}

impl TimeoutOptions {
    /// New options with default values.
    #[must_use]
    pub fn new() -> Self {
        Default::default()
    }

    /// New options with default values but with no retries.
    #[must_use]
    pub fn new_no_retries() -> Self {
        Self::new().with_max_retries(0)
    }

    /// Timeout after which we'll either fail the request or start retrying.
    /// if max_retries is greater than 0 or None.
    #[must_use]
    pub fn with_req_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Timeout multiplied at each iteration.
    #[must_use]
    pub fn with_timeout_backoff(mut self, timeout: Duration) -> Self {
        self.timeout_step = timeout;
        self
    }

    /// Specify a max number of retries before giving up.
    /// None for unlimited retries.
    #[must_use]
    pub fn with_max_retries<M: Into<Option<u32>>>(mut self, max_retries: M) -> Self {
        self.max_retries = max_retries.into();
        self
    }

    /// Minimum timeouts for specific requests.
    #[must_use]
    pub fn with_min_req_timeout(mut self, timeout: impl Into<Option<RequestMinTimeout>>) -> Self {
        self.request_min_timeout = timeout.into();
        self
    }

    /// Get the minimum request timeouts.
    pub fn request_min_timeout(&self) -> Option<&RequestMinTimeout> {
        self.request_min_timeout.as_ref()
    }

    /// Get the http2 Keep Alive interval.
    pub fn keep_alive_interval(&self) -> Duration {
        self.keep_alive_interval
    }
    /// Get the http2 Keep Alive timeout.
    pub fn keep_alive_timeout(&self) -> Duration {
        self.keep_alive_timeout
    }

    /// Get the max retries.
    pub fn max_retries(&self) -> Option<u32> {
        self.max_retries
    }

    /// Get the client.
    pub fn client(&self) -> &ClientId {
        &self.client
    }

    /// Set the connect timeout.
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }
    /// Get the connect timeout.
    pub fn connect_timeout(&self) -> Duration {
        self.connect_timeout
    }
}

/// Client Options trait.
#[async_trait]
#[clonable]
pub trait ClientOpts: Clone + Debug + Send + Sync {
    /// Get this client's identifier.
    fn client_name(&self) -> &ClientId;
    /// Get the configured timeout options.
    fn timeout_opts(&self) -> &TimeoutOptions;
}

/// Identifies which client it is.
#[derive(Debug, Clone)]
pub enum ClientId {
    /// The Rest Server.
    RestServer,
    /// The Core Agent.
    CoreAgent,
    /// The JsonGrpc Agent.
    JsonGrpcAgent,
    /// Not Specified.
    Unnamed,
}
