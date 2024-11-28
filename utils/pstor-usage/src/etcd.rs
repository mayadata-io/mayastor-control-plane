use crate::{
    resources::{FormatSamples, ResourceMgr, ResourceSample, ResourceSamples, Sampler},
    ResourceUpdates,
};
use etcd_client::StatusResponse;
use openapi::{apis::Url, clients::tower::direct::ApiClient};

/// New type over an etcd client that exposed some helper methods.
#[derive(Clone)]
pub(crate) struct Etcd(etcd_client::Client);
impl Etcd {
    /// Return new `Self` which exposes the etcd database usage.
    pub(crate) async fn new(url: Url) -> anyhow::Result<Self> {
        let client = etcd_client::Client::connect([url], None).await?;
        Ok(Self(client))
    }
    async fn status(&self) -> anyhow::Result<StatusResponse> {
        let status = self.0.clone().status().await;
        status.map_err(|e| anyhow::anyhow!("Etcd connection error: {}", e))
    }
    /// dbSize is the size of the backend database physically allocated, in
    /// bytes, of the corresponding member.
    pub(crate) async fn db_size(&self) -> anyhow::Result<u64> {
        Ok(self.status().await?.db_size() as u64)
    }
    /// Convert from bytes to a nicer format with the units.
    pub(crate) fn bytes_to_units(bytes: u64) -> String {
        let byte = 1;
        let kilo = byte * 1024;
        let mega = kilo * 1024;
        if bytes > 10 * mega {
            let mut megas = bytes / mega;
            let kilos = (bytes - (megas * mega)) / kilo;
            if kilos > 100 {
                megas += 1;
            }
            format!("{megas} MiB")
        } else if bytes > mega {
            let megas = bytes / mega;
            let kilos = (bytes - (megas * mega)) / kilo;
            format!("{megas} MiB {kilos} KiB")
        } else if bytes > kilo {
            format!("{} KiB", bytes / kilo)
        } else {
            format!("{bytes} B")
        }
    }
}

/// Used to sample resources and capture etcd resource usage as it goes.
pub(crate) struct EtcdSampler {
    etcd: Etcd,
    steps: u32,
}
impl EtcdSampler {
    /// Returns a new `Sampler` taking a fixed number specified steps.
    /// This means `steps` samples are taken, one step at a time.
    pub(crate) fn new_sampler(etcd: Etcd, steps: u32) -> impl Sampler {
        Self { etcd, steps }
    }
}

#[async_trait::async_trait]
impl Sampler for EtcdSampler {
    async fn sample<T: ResourceMgr>(
        &self,
        client: &ApiClient,
        count: u32,
        resource_mgr: &T,
    ) -> anyhow::Result<(Vec<T::Output>, ResourceSamples)> {
        if count == 0 {
            return Ok((vec![], ResourceSamples::new(vec![])));
        }

        let mut created = Vec::with_capacity((self.steps * count) as usize);
        let mut count_vec = Vec::with_capacity((self.steps * count) as usize);
        let mut usage_vec = Vec::with_capacity((self.steps * count) as usize);

        let base = self.etcd.db_size().await?;
        let mut acc = base;
        for _ in 1..=self.steps {
            created.push(resource_mgr.create(client, count).await?);
            let usage = self.etcd.db_size().await? - acc;

            count_vec.push(count as u64);
            usage_vec.push(usage);

            acc += usage;
        }

        let count_r = resource_mgr.format(count_vec);
        let usage_r = Box::new(DiskUsage { points: usage_vec });

        Ok((created, ResourceSamples::new(vec![count_r, usage_r])))
    }

    async fn sample_mods<T: ResourceUpdates>(
        &self,
        client: &ApiClient,
        count: u32,
        resources: &[T],
    ) -> anyhow::Result<ResourceSamples> {
        if count == 0 {
            return Ok(ResourceSamples::new(vec![]));
        }

        let mut count_vec = Vec::with_capacity((self.steps * count) as usize);
        let mut usage_vec = Vec::with_capacity((self.steps * count) as usize);

        let base = self.etcd.db_size().await?;
        let mut acc = base;

        for each in resources {
            each.modify(client, count).await?;

            let usage = self.etcd.db_size().await? - acc;

            count_vec.push(count as u64);
            usage_vec.push(usage);

            acc += usage;
        }

        let count_r = resources.format(count_vec);
        let usage_r = Box::new(DiskUsage { points: usage_vec });

        Ok(ResourceSamples::new(vec![count_r, usage_r]))
    }
}

/// ResourceSample for etcd disk usage.
pub(crate) struct DiskUsage {
    points: Vec<u64>,
}
impl DiskUsage {
    /// Get new `Self` with the provided data.
    pub(crate) fn new(points: impl Iterator<Item = u64>) -> Self {
        Self {
            points: points.collect(),
        }
    }
}
impl ResourceSample for DiskUsage {
    fn points(&self) -> &Vec<u64> {
        &self.points
    }
    fn points_mut(&mut self) -> &mut Vec<u64> {
        &mut self.points
    }

    fn name(&self) -> String {
        "Disk Usage".to_string()
    }

    fn format_point(&self, point: u64) -> String {
        Etcd::bytes_to_units(point)
    }
}
