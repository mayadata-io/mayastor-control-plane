use crate::{
    operations::{Get, List},
    resources::{
        utils,
        utils::{CreateRows, GetHeaderRow},
        PoolId,
    },
    rest_wrapper::RestClient,
};
use async_trait::async_trait;
use prettytable::Row;
use structopt::StructOpt;

/// Pools resource.
#[derive(StructOpt, Debug)]
pub struct Pools {}

// CreateRows being trait for Vec<Pool> would create the rows from the list of
// Pools returned from REST call.
impl CreateRows for Vec<openapi::models::Pool> {
    fn create_rows(&self) -> Vec<Row> {
        let mut rows: Vec<Row> = Vec::new();
        for pool in self {
            let state = pool.state.as_ref().unwrap();
            // The disks are joined by a comma and shown in the table, ex /dev/vda, /dev/vdb
            let disks = state.disks.join(", ");
            rows.push(row![
                state.id,
                state.capacity,
                state.used,
                disks,
                state.node,
                state.status
            ]);
        }
        rows
    }
}

// GetHeaderRow being trait for Pool would return the Header Row for
// Pool.
impl GetHeaderRow for Vec<openapi::models::Pool> {
    fn get_header_row(&self) -> Row {
        (&*utils::POOLS_HEADERS).clone()
    }
}

#[async_trait(?Send)]
impl List for Pools {
    async fn list(output: &utils::OutputFormat) {
        match RestClient::client().pools_api().get_pools().await {
            Ok(pools) => {
                // Print table, json or yaml based on output format.
                utils::print_table::<openapi::models::Pool>(output, pools);
            }
            Err(e) => {
                println!("Failed to list pools. Error {}", e)
            }
        }
    }
}

/// Pool resource.
#[derive(StructOpt, Debug)]
pub(crate) struct Pool {
    /// ID of the pool.
    id: PoolId,
}

#[async_trait(?Send)]
impl Get for Pool {
    type ID = PoolId;
    async fn get(id: &Self::ID, output: &utils::OutputFormat) {
        match RestClient::client().pools_api().get_pool(id).await {
            Ok(pool) => {
                // Print table, json or yaml based on output format.
                utils::print_table::<openapi::models::Pool>(output, vec![pool]);
            }
            Err(e) => {
                println!("Failed to get pool {}. Error {}", id, e)
            }
        }
    }
}
