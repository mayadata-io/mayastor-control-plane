extern crate utils as external_utils;
use crate::{
    operations::{GetWithArgs, Label, ListWithArgs, PluginResult},
    resources::{
        error::{Error, LabelAssignSnafu, OpError, TopologyError},
        utils,
        utils::{
            optional_cell, print_table, validate_topology_key, validate_topology_value, CreateRow,
            CreateRows, GetHeaderRow, OutputFormat,
        },
        NodeId, PoolId,
    },
    rest_wrapper::RestClient,
};

use async_trait::async_trait;
use openapi::apis::StatusCode;
use prettytable::{Cell, Row};
use serde::Serialize;
use snafu::ResultExt;
use std::collections::HashMap;

use super::VolumeId;

/// Pools resource.
#[derive(clap::Args, Debug)]
pub struct Pools {}

impl CreateRow for openapi::models::Pool {
    fn row(&self) -> Row {
        // The spec would be empty if it was not created using
        // control plane.
        let managed = self.spec.is_some();
        let spec = self.spec.clone().unwrap_or_default();
        // In case the state is not coming as filled, either due to pool, node lost, fill in
        // spec data and mark the status as Unknown.
        let state = self.state.clone().unwrap_or(openapi::models::PoolState {
            capacity: 0,
            disks: spec.disks,
            id: spec.id,
            node: spec.node,
            status: openapi::models::PoolStatus::Unknown,
            used: 0,
            committed: None,
        });
        let free = if state.capacity > state.used {
            state.capacity - state.used
        } else {
            0
        };
        let disks = state.disks.join(", ");
        row![
            self.id,
            disks,
            managed,
            state.node,
            state.status,
            ::utils::bytes::into_human(state.capacity),
            ::utils::bytes::into_human(state.used),
            ::utils::bytes::into_human(free),
            optional_cell(state.committed.map(::utils::bytes::into_human)),
        ]
    }
}

// GetHeaderRow being trait for Pool would return the Header Row for
// Pool.
impl GetHeaderRow for openapi::models::Pool {
    fn get_header_row(&self) -> Row {
        (*utils::POOLS_HEADERS).clone()
    }
}

/// Arguments used when getting a pool.
#[derive(Debug, Clone, clap::Args)]
pub struct GetPoolArgs {
    /// Id of the pool.
    pool_id: PoolId,
    /// Show the labels of the pool.
    #[clap(long, default_value = "false")]
    show_labels: bool,
}

impl GetPoolArgs {
    /// Return the pool ID.
    pub fn pool_id(&self) -> PoolId {
        self.pool_id.clone()
    }
    /// Return whether to show the labels of the pool.
    pub fn show_labels(&self) -> bool {
        self.show_labels
    }
}

/// Arguments used when getting pools.
#[derive(Debug, Clone, clap::Args)]
pub struct GetPoolsArgs {
    /// Gets Pools from this node only.
    #[clap(long)]
    node: Option<NodeId>,

    /// Gets Pools for the given volume.
    #[clap(long)]
    volume: Option<VolumeId>,

    /// Selector (label query) to filter on, supports '=' only.
    /// (e.g. -l key1=value1,key2=value2).
    /// Pools must satisfy all of the specified label constraints.
    #[clap(short = 'l', long)]
    selector: Option<String>,

    /// Show the labels of the pool.
    #[clap(long, default_value = "false")]
    show_labels: bool,
}

impl GetPoolsArgs {
    /// Return the node ID.
    pub fn node(&self) -> &Option<NodeId> {
        &self.node
    }

    /// Return the volume ID.
    pub fn volume(&self) -> &Option<VolumeId> {
        &self.volume
    }

    /// Select the pools based on labels.
    pub fn selector(&self) -> &Option<String> {
        &self.selector
    }

    /// Return whether to show the labels of the pool.
    pub fn show_labels(&self) -> bool {
        self.show_labels
    }
}

#[async_trait(?Send)]
impl ListWithArgs for Pools {
    type Args = GetPoolsArgs;
    async fn list(args: &Self::Args, output: &utils::OutputFormat) -> PluginResult {
        let mut pools = match args.node() {
            Some(node_id) => RestClient::client()
                .pools_api()
                .get_node_pools(node_id)
                .await
                .map(|pools| pools.into_body())
                .map_err(|e| Error::ListPoolsError { source: e }),
            None => RestClient::client()
                .pools_api()
                .get_pools(args.volume().as_ref())
                .await
                .map(|pools| pools.into_body())
                .map_err(|e| Error::ListPoolsError { source: e }),
        }?;

        pools.retain(|pool| match &pool.spec {
            Some(spec) => match &spec.labels {
                Some(pool_labels) => {
                    let pool_label_match =
                        labels_matched(pool_labels, args.selector()).unwrap_or(false);
                    pool_label_match
                }
                None => true,
            },
            None => true,
        });

        let pools_display = PoolDisplay::new_pools(pools.clone(), args.show_labels());
        match output {
            OutputFormat::Yaml | OutputFormat::Json => {
                print_table(output, pools_display.inner);
            }
            OutputFormat::None => {
                print_table(output, pools_display);
            }
        }

        Ok(())
    }
}

/// Pool resource.
#[derive(clap::Args, Debug)]
pub struct Pool {}

#[async_trait(?Send)]
impl GetWithArgs for Pool {
    type ID = PoolId;
    type Args = GetPoolArgs;
    async fn get(id: &Self::ID, args: &Self::Args, output: &utils::OutputFormat) -> PluginResult {
        match RestClient::client().pools_api().get_pool(id).await {
            Ok(pool) => match output {
                OutputFormat::Yaml | OutputFormat::Json => {
                    print_table(output, pool.clone().into_body());
                }
                OutputFormat::None => {
                    print_table(
                        output,
                        PoolDisplay::new(pool.into_body(), args.show_labels()),
                    );
                }
            },
            Err(e) => {
                return Err(Error::GetPoolError {
                    id: id.to_string(),
                    source: e,
                });
            }
        }
        Ok(())
    }
}

/// Check if the labels match the pool labels.
pub(crate) fn labels_matched(
    pool_labels: &HashMap<String, String>,
    labels: &Option<String>,
) -> Result<bool, Error> {
    match labels {
        Some(filter_labels) => {
            for label in filter_labels.split(',') {
                let [key, value] = label.split('=').collect::<Vec<_>>()[..] else {
                    return Err(Error::LabelNodeFilter {
                        labels: filter_labels.to_string(),
                    });
                };
                if pool_labels.get(key) != Some(&value.to_string()) {
                    return Ok(false);
                }
            }
        }
        None => return Ok(true),
    }
    Ok(true)
}

#[async_trait(?Send)]
impl Label for Pool {
    type ID = PoolId;
    async fn label(
        id: &Self::ID,
        label: String,
        overwrite: bool,
        output: &utils::OutputFormat,
    ) -> PluginResult {
        let result = if label.contains('=') {
            let [key, value] = label.split('=').collect::<Vec<_>>()[..] else {
                return Err(TopologyError::LabelMultiAssign {}.into());
            };

            validate_topology_key(key).context(super::error::PoolLabelFormatSnafu)?;
            validate_topology_value(value).context(super::error::PoolLabelFormatSnafu)?;
            match RestClient::client()
                .pools_api()
                .put_pool_label(id, key, value, Some(overwrite))
                .await
            {
                Err(source) => match source.status() {
                    Some(StatusCode::UNPROCESSABLE_ENTITY) if output.none() => {
                        Err(OpError::LabelExists {
                            resource: "Pool".to_string(),
                            id: id.to_string(),
                        })
                    }
                    Some(StatusCode::PRECONDITION_FAILED) if output.none() => {
                        Err(OpError::LabelConflict {
                            resource: "Pool".to_string(),
                            id: id.to_string(),
                        })
                    }
                    Some(StatusCode::NOT_FOUND) if output.none() => {
                        Err(OpError::ResourceNotFound {
                            resource: "Pool".to_string(),
                            id: id.to_string(),
                        })
                    }
                    _ => Err(OpError::Generic {
                        resource: "Pool".to_string(),
                        id: id.to_string(),
                        source,
                    }),
                },
                Ok(pool) => Ok(pool),
            }
        } else {
            snafu::ensure!(label.len() >= 2 && label.ends_with('-'), LabelAssignSnafu);
            let key = &label[.. label.len() - 1];
            validate_topology_key(key)?;
            match RestClient::client()
                .pools_api()
                .del_pool_label(id, key)
                .await
            {
                Err(source) => match source.status() {
                    Some(StatusCode::PRECONDITION_FAILED) if output.none() => {
                        Err(OpError::LabelNotFound {
                            resource: "Pool".to_string(),
                            id: id.to_string(),
                        })
                    }
                    Some(StatusCode::NOT_FOUND) if output.none() => {
                        Err(OpError::ResourceNotFound {
                            resource: "Pool".to_string(),
                            id: id.to_string(),
                        })
                    }
                    _ => Err(OpError::Generic {
                        resource: "Pool".to_string(),
                        id: id.to_string(),
                        source,
                    }),
                },
                Ok(pool) => Ok(pool),
            }
        }?;
        let pool = result.into_body();
        match output {
            OutputFormat::Yaml | OutputFormat::Json => {
                // Print json or yaml based on output format.
                print_table(output, pool);
            }
            OutputFormat::None => {
                // In case the output format is not specified, show a success message.
                let mut labels = pool.spec.unwrap().labels.unwrap_or_default();
                let internal_label = external_utils::dsp_created_by_key();
                labels.remove(&internal_label);
                println!("Pool {id} labelled successfully. Current labels: {labels:?}");
            }
        }
        Ok(())
    }
}

/// The PoolDisplay structure is responsible for controlling the display formatting of Pool
/// objects. `#[serde(flatten)]` and `#[serde(skip)]` attributes are used to ensure that when the
/// object is serialised, only the `inner` object is represented.
#[derive(Serialize, Debug)]
pub struct PoolDisplay {
    #[serde(flatten)]
    pub inner: Vec<openapi::models::Pool>,
    #[serde(skip)]
    show_labels: bool,
}

impl PoolDisplay {
    /// Create a new `PoolDisplay` instance.
    pub(crate) fn new(pool: openapi::models::Pool, show_labels: bool) -> Self {
        let vec: Vec<openapi::models::Pool> = vec![pool];
        Self {
            inner: vec,
            show_labels,
        }
    }
    /// Create a new `PoolDisplay` instance from a vector of pools.
    pub(crate) fn new_pools(pools: Vec<openapi::models::Pool>, show_labels: bool) -> Self {
        Self {
            inner: pools,
            show_labels,
        }
    }

    /// Get a list of pool labels.
    pub(crate) fn pool_label_list(pool: &openapi::models::Pool) -> Vec<String> {
        let mut pools_labels: Vec<String> = vec![];
        let internal_label = external_utils::dsp_created_by_key();

        match &pool.spec {
            Some(spec) => match &spec.labels {
                Some(ds) => {
                    pools_labels = ds
                        .iter()
                        // Dont return the created_by_dsp label for the gets
                        .filter(|(key, _)| *key != &internal_label)
                        .map(|(key, value)| format!("{}={}", key, value))
                        .collect();
                }
                None => {}
            },
            None => {}
        }
        pools_labels
    }
}

// Create the header for a `PoolDisplay` object.
impl GetHeaderRow for PoolDisplay {
    fn get_header_row(&self) -> Row {
        let mut header = (*utils::POOLS_HEADERS).clone();
        if self.show_labels {
            header.extend(vec!["LABELS"]);
        }
        header
    }
}

impl CreateRows for PoolDisplay {
    fn create_rows(&self) -> Vec<Row> {
        let mut rows = vec![];
        for pool in self.inner.iter() {
            let mut row = pool.row();
            if self.show_labels {
                let labelstring = PoolDisplay::pool_label_list(pool).join(", ");
                // Add the pool labels to each row.
                row.add_cell(Cell::new(&labelstring));
            }
            rows.push(row);
        }
        rows
    }
}
