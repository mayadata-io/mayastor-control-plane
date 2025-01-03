use snafu::Snafu;

/// All errors returned when resources command fails.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
#[allow(clippy::enum_variant_names)]
pub enum Error {
    /// Error when listing block devices fails.
    #[snafu(display("Failed to list blockdevices for node {id}. Error {source}"))]
    GetBlockDevicesError {
        id: String,
        source: openapi::tower::client::Error<openapi::models::RestJsonError>,
    },
    /// Error when get node request fails.
    #[snafu(display("Failed to get node {id}. Error {source}"))]
    GetNodeError {
        id: String,
        source: openapi::tower::client::Error<openapi::models::RestJsonError>,
    },
    /// Error when node cordon request fails.
    #[snafu(display("Failed to get node {id}. Error {source}"))]
    NodeCordonError {
        id: String,
        source: openapi::tower::client::Error<openapi::models::RestJsonError>,
    },
    #[snafu(display("Invalid label format: {source}"))]
    NodeLabelFormat { source: TopologyError },
    #[snafu(display("{source}"))]
    NodeLabel { source: OpError },
    #[snafu(display("Invalid label format: {source}"))]
    PoolLabelFormat { source: TopologyError },
    #[snafu(display("{source}"))]
    PoolLabel { source: OpError },

    /// Error when node uncordon request fails.
    #[snafu(display("Failed to uncordon node {id}. Error {source}"))]
    NodeUncordonError {
        id: String,
        source: openapi::tower::client::Error<openapi::models::RestJsonError>,
    },
    /// Error when put node drain request fails.
    #[snafu(display("Failed to put node drain {id}. Error {source}"))]
    PutNodeDrainError {
        id: String,
        source: openapi::tower::client::Error<openapi::models::RestJsonError>,
    },
    /// Error when list nodes request fails.
    #[snafu(display("Failed to list nodes. Error {source}"))]
    ListNodesError {
        source: openapi::tower::client::Error<openapi::models::RestJsonError>,
    },
    /// Error when get pool request fails.
    #[snafu(display("Failed to get pool {id}. Error {source}"))]
    GetPoolError {
        id: String,
        source: openapi::tower::client::Error<openapi::models::RestJsonError>,
    },
    /// Error when list pools request fails.
    #[snafu(display("Failed to list pools. Error {source}"))]
    ListPoolsError {
        source: openapi::tower::client::Error<openapi::models::RestJsonError>,
    },
    /// Error when get volume request fails.
    #[snafu(display("Failed to get volume {id}. Error {source}"))]
    GetVolumeError {
        id: String,
        source: openapi::tower::client::Error<openapi::models::RestJsonError>,
    },
    /// Error when get rebuild history for volume request fails.
    #[snafu(display("Failed to get rebuild history for volume {id}. Error {source}"))]
    GetRebuildHistory {
        id: String,
        source: openapi::tower::client::Error<openapi::models::RestJsonError>,
    },
    /// Error when scale volume request fails.
    #[snafu(display("Failed to scale volume {id}. Error {source}"))]
    ScaleVolumeError {
        id: String,
        source: openapi::tower::client::Error<openapi::models::RestJsonError>,
    },
    /// Error when resize volume request fails.
    #[snafu(display("Failed to resize volume {id}. Error {source}"))]
    ResizeVolumeError {
        id: String,
        source: openapi::tower::client::Error<openapi::models::RestJsonError>,
    },
    /// Error when set volume property request fails.
    #[snafu(display("Failed to set volume {id} property, Error {source}"))]
    SetVolumePropertyError {
        id: String,
        source: openapi::tower::client::Error<openapi::models::RestJsonError>,
    },
    /// Error when list snapshots request fails.
    #[snafu(display("Failed to list volume snapshots. Error {source}"))]
    ListSnapshotsError {
        source: openapi::tower::client::Error<openapi::models::RestJsonError>,
    },
    /// Error when get pool request fails.
    #[snafu(display(
        "Error while parsing labels `{labels}`. \
        The supported formats for labels is: \
        key1=value1,key2=value2"
    ))]
    LabelNodeFilter { labels: String },

    #[snafu(display("Expected output to be a Some"))]
    OutputFormatAbsent,
}

/// Errors related to label topology formats.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum TopologyError {
    #[snafu(display("key must not be an empty string"))]
    KeyIsEmpty {},
    #[snafu(display("value must not be an empty string"))]
    ValueIsEmpty {},
    #[snafu(display("key part must not be more than 63 characters"))]
    KeyTooLong {},
    #[snafu(display("value part must not be more than 63 characters"))]
    ValueTooLong {},
    #[snafu(display("both key and value parts must start with an ascii alphanumeric character"))]
    EdgesNotAlphaNum {},
    #[snafu(display("key can contain at most one `/` character"))]
    KeySlashCount {},
    #[snafu(display(
        "only ascii alphanumeric characters and (`/`,` - `, `_`,`.`) are allowed for the key part"
    ))]
    KeyIsNotAlphaNumericPlus {},
    #[snafu(display(
        "only ascii alphanumeric characters and (`-`,` _ `, `.`) are allowed for the label part"
    ))]
    ValueIsNotAlphaNumericPlus {},
    #[snafu(display("only a single assignment key=value is allowed"))]
    LabelMultiAssign {},
    #[snafu(display(
        "the supported formats are: \
        key=value for adding (example: group=a) \
        and key- for removing (example: group-)"
    ))]
    LabelAssign {},
}

/// Errors related to node label topology operation execution.
#[derive(Debug, snafu::Snafu)]
#[snafu(visibility(pub))]
pub enum OpError {
    #[snafu(display("{resource} {id} not unlabelled as it did not contain the label"))]
    LabelNotFound { resource: String, id: String },
    #[snafu(display("{resource} {id} not labelled as the same label already exists"))]
    LabelExists { resource: String, id: String },
    #[snafu(display("{resource} {id} not found"))]
    ResourceNotFound { resource: String, id: String },
    #[snafu(display(
        "{resource} {id} not labelled as the label key already exists, but with a different value and --overwrite is false"
    ))]
    LabelConflict { resource: String, id: String },
    #[snafu(display("Failed to label {resource} {id}. Error {source}"))]
    Generic {
        resource: String,
        id: String,
        source: openapi::tower::client::Error<openapi::models::RestJsonError>,
    },
}

impl From<TopologyError> for Error {
    fn from(source: TopologyError) -> Self {
        Self::NodeLabelFormat { source }
    }
}
impl From<OpError> for Error {
    fn from(source: OpError) -> Self {
        Self::NodeLabel { source }
    }
}
