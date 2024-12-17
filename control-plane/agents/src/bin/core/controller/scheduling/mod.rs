pub(crate) mod affinity_group;
pub(crate) mod nexus;
pub(crate) mod pool;
pub(crate) mod resources;
pub(crate) mod volume;
mod volume_policy;

use crate::controller::scheduling::{
    nexus::GetPersistedNexusChildrenCtx,
    resources::{ChildItem, PoolItem, ReplicaItem},
    volume::{ReplicaResizePoolsContext, VolumeReplicasForNexusCtx},
};
use std::{cmp::Ordering, collections::HashMap, future::Future};
use weighted_scoring::{Criteria, Value, ValueGrading, WeightedScore};

#[async_trait::async_trait(?Send)]
pub(crate) trait ResourcePolicy<Request: ResourceFilter>: Sized {
    fn apply(self, to: Request) -> Request;
    #[allow(dead_code)]
    fn apply_async(self, to: Request) -> Request {
        self.apply(to)
    }
}

/// Default container of context and a list of items which must be filtered down and sorted.
#[derive(Clone)]
pub(crate) struct ResourceData<C, I: std::fmt::Debug> {
    context: C,
    list: Vec<I>,
}
impl<C, I: std::fmt::Debug> ResourceData<C, I> {
    /// Create a new `Self`.
    pub(crate) fn new(request: C, list: Vec<I>) -> Self {
        Self {
            context: request,
            list,
        }
    }
    pub(crate) fn context(&self) -> &C {
        &self.context
    }
}

#[async_trait::async_trait(?Send)]
pub(crate) trait ResourceFilter: Sized {
    type Request;
    type Item: std::fmt::Debug;

    fn data(&mut self) -> &mut ResourceData<Self::Request, Self::Item>;

    fn policy<P: ResourcePolicy<Self>>(self, policy: P) -> Self {
        policy.apply(self)
    }
    #[allow(dead_code)]
    fn policy_async<P: ResourcePolicy<Self>>(self, policy: P) -> Self {
        policy.apply_async(self)
    }
    fn filter_param<P, F>(mut self, param: &P, filter: F) -> Self
    where
        F: Fn(&P, &Self::Request, &Self::Item) -> bool,
    {
        let data = self.data();
        data.list.retain(|v| filter(param, &data.context, v));
        self
    }
    #[allow(dead_code)]
    fn filter_iter(self, filter: fn(Self) -> Self) -> Self {
        filter(self)
    }
    #[allow(dead_code)]
    async fn filter_iter_async<F, Fut>(self, filter: F) -> Self
    where
        F: Fn(Self) -> Fut,
        Fut: Future<Output = Self>,
    {
        filter(self).await
    }
    fn filter<F: FnMut(&Self::Request, &Self::Item) -> bool>(mut self, mut filter: F) -> Self {
        let data = self.data();
        data.list.retain(|v| filter(&data.context, v));
        self
    }
    fn sort<F: FnMut(&Self::Item, &Self::Item) -> std::cmp::Ordering>(mut self, sort: F) -> Self {
        let data = self.data();
        data.list.sort_by(sort);
        self
    }
    fn sort_ctx<F: FnMut(&Self::Request, &Self::Item, &Self::Item) -> std::cmp::Ordering>(
        mut self,
        mut sort: F,
    ) -> Self {
        let data = self.data();
        data.list.sort_by(|a, b| sort(&data.context, a, b));
        self
    }
    fn collect(self) -> Vec<Self::Item>;
    #[allow(dead_code)]
    fn group_by<K, V, F: Fn(&Self::Request, &Vec<Self::Item>) -> HashMap<K, V>>(
        mut self,
        group: F,
    ) -> HashMap<K, V> {
        let data = self.data();
        group(&data.context, &data.list)
    }
}

/// Represents a sort criteria to be passed to a sort builder.
pub(crate) struct SortCriteria {
    criteria: Criteria,
    grading: ValueGrading,
    value_fn: Box<dyn Fn(&PoolItem) -> Value>,
}

impl SortCriteria {
    /// Create a new sort criteria.
    pub(crate) fn new(
        criteria: Criteria,
        grading: ValueGrading,
        value_fn: impl Fn(&PoolItem) -> Value + 'static,
    ) -> Self {
        SortCriteria {
            criteria,
            grading,
            value_fn: Box::new(value_fn),
        }
    }
}

/// Builds a weighted sorting comparator, with the various sort criterias being added to it.
pub(crate) struct SortBuilder {
    sort_criterias: Vec<SortCriteria>,
}

impl SortBuilder {
    /// Create a new sort builder.
    pub(crate) fn new() -> Self {
        SortBuilder {
            sort_criterias: Vec::new(),
        }
    }

    /// Add sort criteria to the builder.
    pub(crate) fn with_criteria(mut self, sort_criteria: fn() -> SortCriteria) -> Self {
        self.sort_criterias.push(sort_criteria());
        self
    }

    /// Build the comparator based on the weights of sort criteria.
    pub(crate) fn compare(&self, a: &PoolItem, b: &PoolItem) -> std::cmp::Ordering {
        let mut weighted_score = WeightedScore::dual_values();
        for criteria in &self.sort_criterias {
            let value_a = (criteria.value_fn)(a);
            let value_b = (criteria.value_fn)(b);
            weighted_score =
                weighted_score.weigh(criteria.criteria, criteria.grading, value_a, value_b);
        }
        let (score_a, score_b) = weighted_score.score().unwrap();
        score_b.cmp(&score_a)
    }
}

/// Sort the nexus children for removal when decreasing a volume's replica count
pub(crate) struct ChildSorters {}
impl ChildSorters {
    /// Sort replicas by their nexus child (state and rebuild progress)
    /// todo: should we use weights instead (like moac)?
    pub(crate) fn sort(a: &ReplicaItem, b: &ReplicaItem) -> std::cmp::Ordering {
        match Self::sort_by_health(a, b) {
            Ordering::Equal => match Self::sort_by_child(a, b) {
                Ordering::Equal => {
                    // remove mismatched topology replicas first
                    if let (Some(a), Some(b)) = (a.valid_node_topology(), b.valid_node_topology()) {
                        match a.cmp(b) {
                            Ordering::Equal => {}
                            // todo: what if the pool and node topology are at odds with each other?
                            _else => return _else,
                        }
                    }
                    if let (Some(a), Some(b)) = (a.valid_pool_topology(), b.valid_pool_topology()) {
                        match a.cmp(b) {
                            Ordering::Equal => {}
                            _else => return _else,
                        }
                    }

                    let childa_is_local = !a.spec().share.shared();
                    let childb_is_local = !b.spec().share.shared();
                    if childa_is_local == childb_is_local {
                        match a.ag_replicas_on_pool().cmp(&b.ag_replicas_on_pool()) {
                            Ordering::Less => Ordering::Greater,
                            Ordering::Equal => Ordering::Equal,
                            Ordering::Greater => Ordering::Less,
                        }
                    } else if childa_is_local {
                        std::cmp::Ordering::Greater
                    } else {
                        std::cmp::Ordering::Less
                    }
                }
                ord => ord,
            },
            ord => ord,
        }
    }
    // sort replicas by their health: prefer healthy replicas over unhealthy
    fn sort_by_health(a: &ReplicaItem, b: &ReplicaItem) -> std::cmp::Ordering {
        match a.child_info() {
            None => {
                match b.child_info() {
                    Some(b_info) if b_info.healthy => {
                        // sort replicas by their health: prefer healthy replicas over unhealthy
                        std::cmp::Ordering::Less
                    }
                    _ => std::cmp::Ordering::Equal,
                }
            }
            Some(a_info) => match b.child_info() {
                Some(b_info) if a_info.healthy && !b_info.healthy => std::cmp::Ordering::Greater,
                Some(b_info) if !a_info.healthy && b_info.healthy => std::cmp::Ordering::Less,
                _ => std::cmp::Ordering::Equal,
            },
        }
    }
    // remove unused replicas first
    fn sort_by_child(a: &ReplicaItem, b: &ReplicaItem) -> std::cmp::Ordering {
        match a.child_spec() {
            None => {
                match b.child_spec() {
                    None => std::cmp::Ordering::Equal,
                    Some(_) => {
                        // prefer the replica that is not part of a nexus
                        std::cmp::Ordering::Greater
                    }
                }
            }
            Some(_) => {
                match b.child_spec() {
                    // prefer the replica that is not part of a nexus
                    None => std::cmp::Ordering::Less,
                    // compare the child states, and then the rebuild progress
                    Some(_) => match (a.child_state(), b.child_state()) {
                        (Some(a_state), Some(b_state)) => {
                            match a_state.state.partial_cmp(&b_state.state) {
                                None => a_state.rebuild_progress.cmp(&b_state.rebuild_progress),
                                Some(ord) => ord,
                            }
                        }
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => std::cmp::Ordering::Equal,
                    },
                }
            }
        }
    }
}

/// Filter the nexus children/replica candidates when creating a nexus
pub(crate) struct ChildInfoFilters {}
impl ChildInfoFilters {
    /// Should only allow healthy children
    pub(crate) fn healthy(request: &GetPersistedNexusChildrenCtx, item: &ChildItem) -> bool {
        // on first creation there is no nexus_info/child_info so all children are deemed healthy
        let first_create = request.nexus_info().is_none();
        first_create || item.info().as_ref().map(|i| i.healthy).unwrap_or(false)
    }
}

/// Filter the nexus children/replica candidates when creating a nexus
pub(crate) struct ReplicaFilters {}
impl ReplicaFilters {
    /// Should only allow children with corresponding online replicas
    pub(crate) fn online(_request: &GetPersistedNexusChildrenCtx, item: &ChildItem) -> bool {
        item.state().online()
    }

    /// Should only try to resize online replicas
    pub(crate) fn online_for_resize(
        _request: &ReplicaResizePoolsContext,
        item: &ChildItem,
    ) -> bool {
        item.state().online()
    }

    /// Should only allow children with corresponding replicas with enough size
    pub(crate) fn size(request: &GetPersistedNexusChildrenCtx, item: &ChildItem) -> bool {
        match request.vol_spec() {
            Some(volume) => item.state().size >= volume.size,
            None => true,
        }
    }

    /// Should only allow children which are reservable.
    pub(crate) fn reservable(request: &GetPersistedNexusChildrenCtx, item: &ChildItem) -> bool {
        !request.shutdown_failed_nexuses().iter().any(|p| {
            let nexus = p.lock();
            nexus.node == item.pool().node && nexus.contains_replica(&item.spec().uuid)
        })
    }
}

/// Sort the nexus replicas/children by preference when creating a nexus
pub(crate) struct ChildItemSorters {}
impl ChildItemSorters {
    /// Sort ChildItem's for volume nexus creation
    /// Prefer children local to where the nexus will be created
    pub(crate) fn sort_by_locality(
        request: &GetPersistedNexusChildrenCtx,
        a: &ChildItem,
        b: &ChildItem,
    ) -> std::cmp::Ordering {
        let a_is_local = Some(&a.state().node) == request.target_node();
        let b_is_local = Some(&b.state().node) == request.target_node();
        match (a_is_local, b_is_local) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (_, _) => std::cmp::Ordering::Equal,
        }
    }
}

/// Filter replicas when selecting the best candidates to add to a nexus
pub(crate) struct AddReplicaFilters {}
impl AddReplicaFilters {
    /// Should only allow children with corresponding online replicas
    pub(crate) fn online(_request: &VolumeReplicasForNexusCtx, item: &ChildItem) -> bool {
        item.state().online()
    }

    /// Should only allow children with corresponding replicas with enough size
    pub(crate) fn size(request: &VolumeReplicasForNexusCtx, item: &ChildItem) -> bool {
        item.state().size >= request.vol_spec().size
    }

    /// Should only allow children which are reservable.
    pub(crate) fn reservable(request: &VolumeReplicasForNexusCtx, item: &ChildItem) -> bool {
        !request.shutdown_failed_nexuses().iter().any(|p| {
            let nexus = p.lock();
            nexus.node == item.pool().node && nexus.contains_replica(&item.spec().uuid)
        })
    }
}

/// Sort replicas to pick the best choice to add to a given nexus
pub(crate) struct AddReplicaSorters {}
impl AddReplicaSorters {
    /// Sorted by:
    /// 1. replicas local to the nexus
    /// 2. replicas which have not been marked as faulted by the io-engine
    /// 3. replicas from pools with more free space
    pub(crate) fn sort(
        request: &VolumeReplicasForNexusCtx,
        a: &ChildItem,
        b: &ChildItem,
    ) -> std::cmp::Ordering {
        let a_is_local = a.state().node == request.nexus_spec().node;
        let b_is_local = b.state().node == request.nexus_spec().node;
        match (a_is_local, b_is_local) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (_, _) => {
                let a_healthy = a.info().as_ref().map(|i| i.healthy).unwrap_or(false);
                let b_healthy = b.info().as_ref().map(|i| i.healthy).unwrap_or(false);
                match (a_healthy, b_healthy) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    (_, _) => a.pool().free_space().cmp(&b.pool().free_space()),
                }
            }
        }
    }
}
