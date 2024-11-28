use openapi::clients::tower::direct::ApiClient;

/// Resource Manager that handles creating and deleting resources.
#[async_trait::async_trait]
pub(crate) trait ResourceMgr: Send + Sync + FormatSamples {
    type Output: ResourceDelete + ResourceUpdates;
    /// Create `count` resources.
    async fn create(&self, client: &ApiClient, count: u32) -> anyhow::Result<Self::Output>;
    /// Delete the created resources.
    #[allow(dead_code)]
    async fn delete(&self, client: &ApiClient, created: Self::Output) -> anyhow::Result<()>;
}

/// Formats collected points into a ResourceSample.
pub(crate) trait FormatSamples: Send + Sync {
    /// Format a sample from the given data points.
    fn format(&self, points: Vec<u64>) -> Box<dyn ResourceSample>;
}

/// Resources that are deletable and should be deleted.
#[async_trait::async_trait]
pub(crate) trait ResourceDelete: Send + Sync + Clone {
    async fn delete(&self, client: &ApiClient) -> anyhow::Result<()>;
}

/// Resource updates/modifications that can be performed by a user.
#[async_trait::async_trait]
pub(crate) trait ResourceUpdates: Send + Sync + FormatSamples + Default {
    /// Update/Modify a resource `count` times
    async fn modify(&self, client: &ApiClient, count: u32) -> anyhow::Result<()>;
}

#[async_trait::async_trait]
impl<T: ResourceDelete> ResourceDelete for Vec<T> {
    async fn delete(&self, client: &ApiClient) -> anyhow::Result<()> {
        for e in self.iter() {
            e.delete(client).await?;
        }
        Ok(())
    }
}

impl<T: FormatSamples + Default> FormatSamples for [T] {
    fn format(&self, points: Vec<u64>) -> Box<dyn ResourceSample> {
        match self.first() {
            None => T::default().format(points),
            Some(first) => first.format(points),
        }
    }
}

/// Sample `count` data points from a given `ResourceMgr`.
#[async_trait::async_trait]
pub(crate) trait Sampler {
    /// Samples `count` resource allocations returning both the allocated resources and the samples.
    async fn sample<T: ResourceMgr>(
        &self,
        client: &ApiClient,
        count: u32,
        resource_mgr: &T,
    ) -> anyhow::Result<(Vec<T::Output>, ResourceSamples)>;
    /// Samples `count` resource modifications returning the collected samples.
    async fn sample_mods<T: ResourceUpdates>(
        &self,
        client: &ApiClient,
        count: u32,
        resources: &[T],
    ) -> anyhow::Result<ResourceSamples>;
}

/// A sample of data points, if you will.
pub(crate) trait ResourceSample {
    /// Get a reference to the data points.
    fn points(&self) -> &Vec<u64>;
    /// Get a mutable reference to the data points.
    fn points_mut(&mut self) -> &mut Vec<u64>;
    /// Accrue the data points with the given ones.
    fn accrue_points(&mut self, add: &[u64]) {
        for (i, p) in self.points_mut().iter_mut().enumerate() {
            if let Some(p2) = add.get(i) {
                *p += *p2;
            }
        }
        if add.len() > self.points().len() {
            let add = &add[self.points().len()..add.len()];
            self.points_mut().extend(add);
        }
    }
    /// Get the name of the data points.
    fn name(&self) -> String;
    /// Format a data point into a String.
    fn format_point(&self, point: u64) -> String;
}

/// A helper collection of samples
#[derive(Default)]
pub(crate) struct ResourceSamples {
    inner: Vec<Box<dyn ResourceSample>>,
}

impl ResourceSamples {
    /// Return a new `Self` with the starting samples.
    pub(crate) fn new(samples: Vec<Box<dyn ResourceSample>>) -> Self {
        Self { inner: samples }
    }
    /// Move out the samples.
    pub(crate) fn into_inner(self) -> Vec<Box<dyn ResourceSample>> {
        self.inner
    }
    /// Get reference to the samples.
    pub(crate) fn inner(&self) -> &Vec<Box<dyn ResourceSample>> {
        &self.inner
    }
    /// Get length of the samples.
    pub(crate) fn len(&self) -> usize {
        self.inner.len()
    }
    /// Extend the current samples with more samples.
    /// If any of the samples name match then they are accrued rather than kept as duplicate.
    pub(crate) fn extend(&mut self, add: Vec<Box<dyn ResourceSample>>) {
        // Reverse now and at the end so we keep the same order when folding
        self.inner.reverse();

        for i in add.into_iter() {
            if let Some(m) = self.inner.iter_mut().find(|r| i.name() == r.name()) {
                m.accrue_points(i.points());
            } else {
                self.inner.push(i);
            }
        }

        self.inner.reverse();
    }
    /// Check if we have no samples or if no samples have data points.
    pub(crate) fn is_empty(&self) -> bool {
        if self.inner().is_empty() {
            return true;
        }
        if self.inner().iter().all(|r| r.points().is_empty()) {
            return true;
        }
        false
    }
}

/// New Generic sample of data points with a name.
pub(crate) struct GenericSample {
    name: String,
    points: Vec<u64>,
}
impl GenericSample {
    /// Get new `Self` with a name.
    pub(crate) fn new(name: &str, points: impl Iterator<Item = u64>) -> Self {
        Self {
            name: name.to_string(),
            points: points.collect(),
        }
    }
}
impl ResourceSample for GenericSample {
    fn points(&self) -> &Vec<u64> {
        &self.points
    }
    fn points_mut(&mut self) -> &mut Vec<u64> {
        &mut self.points
    }
    fn name(&self) -> String {
        self.name.clone()
    }
    fn format_point(&self, point: u64) -> String {
        point.to_string()
    }
}
