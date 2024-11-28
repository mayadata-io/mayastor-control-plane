use crate::{
    etcd::{Etcd, EtcdSampler},
    pools::PoolMgr,
    printer::{PrettyPrinter, Printer, TabledData},
    resources::{ResourceDelete, ResourceMgr, ResourceSamples, Sampler},
    volumes::VolMgr,
};
use openapi::{
    apis::Url,
    clients::tower::{direct::ApiClient, Configuration},
};

use anyhow::anyhow;
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::time::Duration;

fn parse_size_compat(src: &str) -> Result<u64, parse_size::Error> {
    parse_size::parse_size(src)
}

/// Simulate how much storage a cluster would require based on some parameters.
#[derive(Parser, Debug, Clone, Hash, Eq, PartialEq)]
pub(crate) struct Simulation {
    #[clap(flatten)]
    opts: SimulationOpts,

    /// Skip the output of the total storage usage after allocation of all resources and also after
    /// those resources have been deleted.
    #[clap(long = "no-total-stats", action = clap::ArgAction::SetFalse)]
    total_stats: bool,

    #[clap(skip)]
    no_print_samples: bool,
}

#[derive(Parser, Serialize, Deserialize, Debug, Clone, Hash, Eq, PartialEq)]
#[serde(default)]
pub(crate) struct SimulationOpts {
    /// Size of the pools.
    #[clap(long, value_parser = parse_size_compat, default_value = "20MiB")]
    pool_size: u64,

    /// Size of the volumes.
    #[clap(long, value_parser = parse_size_compat, default_value = "5MiB")]
    volume_size: u64,

    /// Number of volume replicas.
    #[clap(long, default_value = "3")]
    volume_replicas: u8,

    /// Number of pools per sample.
    #[clap(short, long, default_value = "10")]
    pools: u32,

    /// Number of pool samples.
    #[clap(long, default_value = "5")]
    pool_samples: u32,

    /// Number of volumes per sample.
    #[clap(short, long, default_value = "20")]
    volumes: u32,

    /// Number of volume samples.
    #[clap(long, default_value = "10")]
    volume_samples: u32,

    /// Attaches and detaches `N` volumes from each volume sample.
    /// In other words, we will publish/unpublish each `N` volumes from each list of samples.
    /// Please note that this can take quite some time; it's very slow to create volume targets
    /// with remote replicas.
    #[clap(long, default_value = "2")]
    volume_attach_cycles: u32,

    /// Use ram based pools instead of files (useful for debugging with small pool allocation).
    /// When using files the /tmp/pool directory will be used.
    #[clap(long)]
    pool_use_malloc: bool,
}

impl Default for SimulationOpts {
    fn default() -> Self {
        SimulationOpts::parse_from(Vec::<String>::new())
    }
}

impl From<SimulationOpts> for Simulation {
    fn from(opts: SimulationOpts) -> Self {
        Self {
            total_stats: true,
            opts,
            no_print_samples: true,
        }
    }
}

impl SimulationOpts {
    /// Set number of volume replicas.
    pub(crate) fn with_replicas(mut self, replicas: u8) -> Self {
        self.volume_replicas = replicas;
        self
    }
    /// Number of volume replicas.
    pub(crate) fn replicas(&self) -> u8 {
        self.volume_replicas
    }
    /// Attach/detach volumes from each cycle.
    pub(crate) fn volume_attach_cycles(&self) -> u64 {
        self.volume_attach_cycles as u64
    }
    /// Volume turnover.
    pub(crate) fn volume_turnover(&self) -> u64 {
        self.volumes as u64
    }
}

impl Simulation {
    fn print<P, T>(&self, printer: &P, printable: &T)
    where
        P: Printer,
        T: TabledData<Row = P::Row>,
    {
        if !self.no_print_samples {
            printer.print(printable);
        }
    }

    /// Sets whether the simulation prints its samples as it's simulating.
    pub(crate) fn set_print_samples(&mut self, print: bool) {
        self.no_print_samples = !print;
    }

    /// Simulate with the current parameters.
    pub(crate) async fn simulate(
        &self,
        external_cluster: &Option<Url>,
    ) -> anyhow::Result<RunStats> {
        let args = &self.opts;

        let (client, cluster) = match external_cluster {
            None => {
                // cluster will be terminated on drop
                let cluster = deployer_cluster::ClusterBuilder::builder()
                    .with_silence_test_traces()
                    .with_build(false)
                    .with_build_all(false)
                    .with_jaeger(false)
                    .with_io_engines(args.volume_replicas.into())
                    .build()
                    .await
                    .map_err(|e| anyhow!("Failed to build cluster: {}", e))?;

                (cluster.rest_v00(), Some(cluster))
            }
            Some(rest_url) => {
                let openapi_client_config = Configuration::builder()
                    .with_timeout(Duration::from_secs(5))
                    .build_url(rest_url.clone())
                    .map_err(|e| {
                        anyhow::anyhow!("Failed to create rest client config: '{:?}'", e)
                    })?;
                let client = ApiClient::new(openapi_client_config);
                (client, None)
            }
        };
        let cleanup = cluster.is_none() || self.total_stats;

        let etcd = Etcd::new(Url::parse("http://[::]:2379")?).await?;

        // create some pools as backing for the volumes
        let four_mb = 4096 * 1024;
        let mut volume_size = args.volume_size;
        if (volume_size % four_mb) != 0 {
            volume_size += four_mb - (volume_size % four_mb);
        }
        let pools = if args.volumes > 0 && args.volume_samples > 0 {
            args.volume_replicas as u32
        } else {
            0
        };
        let pool_size = args.volume_samples as u64 * args.volumes as u64 * (volume_size + four_mb); // 4M for the metadata
        let pool_mgr = PoolMgr::new_mgr(&client, pool_size, args.pool_use_malloc).await?;
        let vol_pools = pool_mgr.create(&client, pools).await?;

        // capture the initial database size
        let initial = etcd.db_size().await?;
        let printer = PrettyPrinter::new();

        // sample how much space the volumes take up
        let vol_mgr = VolMgr::new_mgr(args.volume_replicas, args.volume_size).await?;
        let (volumes, vol_results) = EtcdSampler::new_sampler(etcd.clone(), args.volume_samples)
            .sample(&client, args.volumes, &vol_mgr)
            .await?;
        self.print(&printer, &vol_results);

        // sample how much space the pools take up
        let pool_mgr = PoolMgr::new_mgr(&client, args.pool_size, args.pool_use_malloc).await?;
        let (pools, mut pool_results) = EtcdSampler::new_sampler(etcd.clone(), args.pool_samples)
            .sample(&client, args.pools, &pool_mgr)
            .await?;
        self.print(&printer, &pool_results);

        // this is the combined pool and volume usage
        if args.pools > 0 && args.volumes > 0 {
            pool_results.extend(vol_results.into_inner());
            self.print(&printer, &pool_results);
        }

        // capture the database size after we've completed allocating new resources
        let after_alloc = etcd.db_size().await?;

        if args.volume_attach_cycles > 0 {
            let mod_results = EtcdSampler::new_sampler(etcd.clone(), args.volume_samples)
                .sample_mods(&client, args.volume_attach_cycles, &volumes)
                .await?;
            self.print(&printer, &mod_results);
        }

        // capture the database size after we've churned the volumes
        let after_mod = etcd.db_size().await?;

        // clean up created resources
        if cleanup {
            volumes.delete(&client).await?;
            pools.delete(&client).await?;
            vol_pools.delete(&client).await?;
        }

        // capture the database size after we've deleted the resources
        let after_cleanup = etcd.db_size().await?;

        let stats = RunStats::new(initial, after_alloc, after_mod, after_cleanup);

        if self.total_stats {
            self.print(&printer, &stats);
        }

        Ok(stats)
    }
    /// Total number of volumes allocated.
    pub(crate) fn volumes_allocated(&self) -> u64 {
        (self.opts.volumes * self.opts.volume_samples) as u64
    }
    /// Total number of volumes modified.
    pub(crate) fn volumes_modified(&self) -> u64 {
        (self.opts.volume_attach_cycles * self.opts.volume_samples) as u64
    }
}

/// Stats collected after running a simulation.
#[derive(Debug, Clone, Default)]
pub(crate) struct RunStats {
    allocation: u64,
    modification: u64,
    cleanup: u64,
    last: u64,
}
impl RunStats {
    fn new(initial: u64, after_alloc: u64, after_mod: u64, after_cleanup: u64) -> Self {
        Self {
            allocation: after_alloc - initial,
            modification: after_mod - after_alloc,
            cleanup: after_cleanup - after_mod,
            last: after_cleanup,
        }
    }
    /// How many bytes were used as a result of resource allocation
    pub(crate) fn allocation(&self) -> u64 {
        self.allocation
    }
    /// How many bytes were used as a result of resource modification
    pub(crate) fn modification(&self) -> u64 {
        self.modification
    }
    /// How many bytes were used as a result of resource deletion
    pub(crate) fn cleanup(&self) -> u64 {
        self.cleanup
    }
    /// Total number of bytes
    fn total(&self) -> u64 {
        self.allocation + self.modification + self.cleanup
    }
}
impl TabledData for RunStats {
    type Row = prettytable::Row;

    fn titles(&self) -> Self::Row {
        prettytable::Row::new(vec![
            crate::new_cell("Creation"),
            crate::new_cell("Modification"),
            crate::new_cell("Cleanup"),
            crate::new_cell("Total"),
            crate::new_cell("Current"),
        ])
    }

    fn rows(&self) -> Vec<Self::Row> {
        vec![prettytable::Row::new(vec![
            crate::new_cell(&Etcd::bytes_to_units(self.allocation)),
            crate::new_cell(&Etcd::bytes_to_units(self.modification)),
            crate::new_cell(&Etcd::bytes_to_units(self.cleanup)),
            crate::new_cell(&Etcd::bytes_to_units(self.total())),
            crate::new_cell(&Etcd::bytes_to_units(self.last)),
        ])]
    }
}

impl TabledData for ResourceSamples {
    type Row = prettytable::Row;

    fn titles(&self) -> Self::Row {
        prettytable::Row::new(
            self.inner()
                .iter()
                .map(|r| crate::new_cell(&r.name()))
                .collect::<Vec<_>>(),
        )
    }

    fn rows(&self) -> Vec<Self::Row> {
        if self.is_empty() {
            return vec![];
        }
        let max_row = self
            .inner()
            .iter()
            .map(|estimation| estimation.points().len())
            .max()
            .unwrap_or(0);

        let mut rows = vec![];
        let mut acc_cache = vec![0; self.len()];
        for row_index in 0..max_row {
            let mut row = prettytable::Row::default();
            for (index, estimation) in self.inner().iter().enumerate() {
                let acc = acc_cache.get_mut(index).expect("already validated");

                let point = estimation.points().get(row_index).cloned().unwrap_or(0);
                *acc += point;
                let formatted_point = estimation.format_point(*acc);

                let cell = crate::new_cell(&formatted_point);
                row.add_cell(cell);
            }
            rows.push(row);
        }
        rows
    }
}
