//! K8S pool operator watches for pool CRs and creates the pool on the given node.
//! There is a maximum retry limit that will put the pool into a steady error state.
//!
//! Successfully created pools are recreated by the control plane.

mod crd;

use chrono::Utc;
use clap::{App, Arg, ArgMatches};
use crd::{DiskPool, DiskPoolStatus, PoolState};
use futures::StreamExt;
use k8s_openapi::{api::core::v1::Event, apimachinery::pkg::apis::meta::v1::MicroTime};
use kube::{
    api::{Api, ListParams, ObjectMeta, Patch, PatchParams, PostParams},
    runtime::{
        controller::{Action, Controller},
        finalizer,
    },
    Client, CustomResourceExt, Resource, ResourceExt,
};
use openapi::{
    apis::StatusCode,
    clients::{self, tower::Url},
    models::{CreatePoolBody, Pool, RestJsonError},
};
use opentelemetry::global;

use crate::crd::CrPoolState;
use serde_json::json;
use snafu::Snafu;
use std::{
    collections::HashMap,
    ops::Deref,
    sync::{Arc, Mutex},
    time::Duration,
};
use tracing::{debug, error, info, trace, warn};

const WHO_AM_I: &str = "DiskPool Operator";
const WHO_AM_I_SHORT: &str = "dsp-operator";

/// Errors generated during the reconciliation loop
#[derive(Debug, Snafu)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum Error {
    #[snafu(display(
        "Failed to reconcile '{}' CRD within set limits, aborting operation",
        name
    ))]
    /// Error generated when the loop stops processing
    ReconcileError {
        name: String,
    },
    /// Generated when we have a duplicate resource version for a given resource
    Duplicate {
        timeout: u32,
    },
    /// Spec error
    SpecError {
        value: String,
        timeout: u32,
    },
    #[snafu(display("Kubernetes client error: {}", source))]
    /// k8s client error
    Kube {
        source: kube::Error,
    },
    #[snafu(display("HTTP request error: {}", source))]
    Request {
        source: clients::tower::RequestError,
    },
    #[snafu(display("HTTP response error: {}", source))]
    Response {
        source: clients::tower::ResponseError<RestJsonError>,
    },
    Noun {},
}

impl From<clients::tower::Error<RestJsonError>> for Error {
    fn from(source: clients::tower::Error<RestJsonError>) -> Self {
        match source {
            clients::tower::Error::Request(source) => Error::Request { source },
            clients::tower::Error::Response(source) => Self::Response { source },
        }
    }
}

/// Additional per resource context during the runtime; it is volatile
#[derive(Clone)]
pub(crate) struct ResourceContext {
    /// The latest CRD known to us
    inner: Arc<DiskPool>,
    /// Counter that keeps track of how many times the reconcile loop has run
    /// within the current state
    num_retries: u32,
    /// Reference to the operator context
    ctx: Arc<OperatorContext>,
    event_info: Arc<Mutex<Vec<String>>>,
}

impl Deref for ResourceContext {
    type Target = DiskPool;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// Data we want access to in error/reconcile calls
pub(crate) struct OperatorContext {
    /// Reference to our k8s client
    k8s: Client,
    /// Hashtable of name and the full last seen CRD
    inventory: tokio::sync::RwLock<HashMap<String, ResourceContext>>,
    /// HTTP client
    http: clients::tower::ApiClient,
    /// Interval
    interval: u64,
}

impl OperatorContext {
    /// Upsert the potential new CRD into the operator context. If an existing
    /// resource with the same name is present, the old resource is
    /// returned.
    pub(crate) async fn upsert(
        &self,
        ctx: Arc<OperatorContext>,
        dsp: Arc<DiskPool>,
    ) -> ResourceContext {
        let resource = ResourceContext {
            inner: dsp,
            num_retries: 0,
            event_info: Default::default(),
            ctx,
        };

        let mut i = self.inventory.write().await;
        debug!(count = ?i.keys().count(), "current number of CRDS");

        match i.get_mut(&resource.name_any()) {
            Some(p) => {
                if p.resource_version() == resource.resource_version() {
                    if matches!(
                        resource.status,
                        Some(DiskPoolStatus {
                            state: PoolState::Created,
                            ..
                        })
                    ) {
                        return p.clone();
                    }

                    debug!(status =? resource.status, "duplicate event or long running operation");

                    // The status should be the same here as well
                    assert_eq!(&p.status, &resource.status);
                    p.num_retries += 1;
                    return p.clone();
                }

                // Its a new resource version which means we will swap it out
                // to reset the counter.
                let p = i
                    .insert(resource.name_any(), resource.clone())
                    .expect("existing resource should be present");
                info!(name = ?p.name_any(), "new resource_version inserted");
                resource
            }

            None => {
                let p = i.insert(resource.name_any(), resource.clone());
                assert!(p.is_none());
                resource
            }
        }
    }
    /// Remove the resource from the operator
    pub(crate) async fn remove(&self, name: String) -> Option<ResourceContext> {
        let mut i = self.inventory.write().await;
        let removed = i.remove(&name);
        if let Some(removed) = removed {
            info!(name =? removed.name_any(), "removed from inventory");
            return Some(removed);
        }
        None
    }
}

impl ResourceContext {
    /// Called when putting our finalizer on top of the resource.
    #[tracing::instrument(fields(name = ?dsp.name_any()))]
    pub(crate) async fn put_finalizer(dsp: Arc<DiskPool>) -> Result<Action, Error> {
        Ok(Action::await_change())
    }

    /// Remove pool from control plane if exist, Then delete it from map.
    #[tracing::instrument(fields(name = ?resource.name_any()) skip(resource))]
    pub(crate) async fn delete_finalizer(
        resource: ResourceContext,
        attempt_delete: bool,
    ) -> Result<Action, Error> {
        let ctx = resource.ctx.clone();
        if attempt_delete {
            resource.delete_pool().await?;
        }
        ctx.remove(resource.name_any()).await;
        Ok(Action::await_change())
    }

    /// Clone the inner value of this resource
    fn inner(&self) -> Arc<DiskPool> {
        self.inner.clone()
    }

    /// Construct an API handle for the resource
    fn api(&self) -> Api<DiskPool> {
        Api::namespaced(self.ctx.k8s.clone(), &self.namespace().unwrap())
    }

    /// Control plane pool handler.
    fn pools_api(&self) -> &dyn openapi::apis::pools_api::tower::client::Pools {
        self.ctx.http.pools_api()
    }

    /// Control plane block device handler.
    fn block_devices_api(
        &self,
    ) -> &dyn openapi::apis::block_devices_api::tower::client::BlockDevices {
        self.ctx.http.block_devices_api()
    }

    /// Patch the given dsp status to the state provided.
    async fn patch_status(&self, status: DiskPoolStatus) -> Result<DiskPool, Error> {
        let status = json!({ "status": status });

        let ps = PatchParams::apply(WHO_AM_I);

        let o = self
            .api()
            .patch_status(&self.name_any(), &ps, &Patch::Merge(&status))
            .await
            .map_err(|source| Error::Kube { source })?;

        debug!(name = ?o.name_any(), old = ?self.status, new =?o.status, "status changed");
        Ok(o)
    }

    /// Create a pool when there is no status found. When no status is found for
    /// this resource it implies that it does not exist yet and so we create
    /// it. We set the state of the of the object to Creating, such that we
    /// can track the its progress
    async fn init_cr(&self) -> Result<Action, Error> {
        let _ = self.patch_status(DiskPoolStatus::default()).await?;
        Ok(Action::await_change())
    }

    /// Mark Pool state as None as couldnt find already provisioned pool in control plane.
    async fn mark_pool_not_found(&self) -> Result<Action, Error> {
        self.patch_status(DiskPoolStatus::not_found()).await?;
        error!(name = ?self.name_any(), "Pool not found, clearing status");
        Ok(Action::requeue(Duration::from_secs(30)))
    }

    /// Patch the resource state to creating.
    async fn is_missing(&self) -> Result<Action, Error> {
        self.patch_status(DiskPoolStatus::default()).await?;
        Ok(Action::await_change())
    }

    /// Patch the resource state to terminating.
    async fn mark_terminating_when_unknown(&self) -> Result<Action, Error> {
        self.patch_status(DiskPoolStatus::terminating_when_unknown())
            .await?;
        Ok(Action::requeue(Duration::from_secs(self.ctx.interval)))
    }

    /// Used to patch control plane state as Unknown.
    async fn mark_unknown(&self) -> Result<Action, Error> {
        self.patch_status(DiskPoolStatus::mark_unknown()).await?;
        Ok(Action::requeue(Duration::from_secs(self.ctx.interval)))
    }

    /// Create or import the pool, on failure try again.
    #[tracing::instrument(fields(name = ?self.name_any(), status = ?self.status) skip(self))]
    pub(crate) async fn create_or_import(self) -> Result<Action, Error> {
        let mut labels: HashMap<String, String> = HashMap::new();
        labels.insert(
            String::from(utils::CREATED_BY_KEY),
            String::from(utils::DSP_OPERATOR),
        );

        let body = CreatePoolBody::new_all(self.spec.disks(), labels);
        match self
            .pools_api()
            .put_node_pool(&self.spec.node(), &self.name_any(), body)
            .await
        {
            Ok(_) => {}
            Err(clients::tower::Error::Response(response))
                if response.status() == clients::tower::StatusCode::UNPROCESSABLE_ENTITY =>
            {
                // UNPROCESSABLE_ENTITY indicates that the pool spec already exists in the
                // control plane. Keeping it idempotent.
            }
            Err(error) => {
                return match self
                    .block_devices_api()
                    .get_node_block_devices(&self.spec.node(), Some(true))
                    .await
                {
                    Ok(response) => {
                        if !response.into_body().into_iter().any(|b| {
                            b.devname == normalize_disk(&self.spec.disks()[0])
                                || b.devlinks
                                    .iter()
                                    .any(|d| *d == normalize_disk(&self.spec.disks()[0]))
                        }) {
                            self.k8s_notify(
                                "Create or import",
                                "Missing",
                                &format!(
                                    "The block device(s): {} can not be found",
                                    &self.spec.disks()[0]
                                ),
                                "Warn",
                            )
                            .await;
                            error!(
                                "The block device(s): {} can not be found",
                                &self.spec.disks()[0]
                            );
                            Err(error.into())
                        } else {
                            self.k8s_notify(
                                "Create or Import Failure",
                                "Failure",
                                format!("Unable to create or import pool {error}").as_str(),
                                "Critical",
                            )
                            .await;
                            error!("Unable to create or import pool {}", error);
                            Err(error.into())
                        }
                    }
                    Err(clients::tower::Error::Response(response))
                        if response.status() == StatusCode::NOT_FOUND =>
                    {
                        self.k8s_notify(
                            "Create or Import Failure",
                            "Failure",
                            format!("Unable to find io-engine node {}", &self.spec.node()).as_str(),
                            "Critical",
                        )
                        .await;
                        error!("Unable to find io-engine node {}", &self.spec.node());
                        Err(error.into())
                    }
                    _ => {
                        self.k8s_notify(
                            "Create or Import Failure",
                            "Failure",
                            format!("Unable to create or import pool {error}").as_str(),
                            "Critical",
                        )
                        .await;
                        error!("Unable to create or import pool {}", error);
                        Err(error.into())
                    }
                };
            }
        }

        self.k8s_notify(
            "Create or Import",
            "Created",
            "Created or imported pool",
            "Normal",
        )
        .await;

        self.pool_created().await
    }

    /// Delete the pool from the io-engine instance
    #[tracing::instrument(fields(name = ?self.name_any(), status = ?self.status) skip(self))]
    async fn delete_pool(&self) -> Result<Action, Error> {
        let res = self
            .pools_api()
            .del_node_pool(&self.spec.node(), &self.name_any())
            .await;

        match res {
            Ok(_) => {
                self.k8s_notify(
                    "Destroyed pool",
                    "Destroy",
                    "The pool has been destroyed",
                    "Normal",
                )
                .await;
                Ok(Action::await_change())
            }
            Err(clients::tower::Error::Response(response))
                if response.status() == StatusCode::NOT_FOUND =>
            {
                self.k8s_notify(
                    "Destroyed pool",
                    "Destroy",
                    "The pool was already destroyed",
                    "Normal",
                )
                .await;
                Ok(Action::await_change())
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Gets pool from control plane and sets state as applicable.
    #[tracing::instrument(fields(name = ?self.name_any(), status = ?self.status) skip(self))]
    async fn pool_created(self) -> Result<Action, Error> {
        let pool = self
            .pools_api()
            .get_node_pool(&self.spec.node(), &self.name_any())
            .await?
            .into_body();

        if pool.state.is_some() {
            let _ = self.patch_status(DiskPoolStatus::from(pool)).await?;

            self.k8s_notify(
                "Online pool",
                "Online",
                "Pool online and ready to roll!",
                "Normal",
            )
            .await;

            Ok(Action::await_change())
        } else {
            // the pool does not have a status yet reschedule the operation
            Ok(Action::requeue(Duration::from_secs(3)))
        }
    }

    /// Check the state of the pool.
    ///
    /// Get the pool information from the control plane and use this to set the state of the CRD
    /// accordingly. If the control plane returns a pool state, set the CRD to 'Online'. If the
    /// control plane does not return a pool state (occurs when a node is missing), set the CRD to
    /// 'Unknown' and let the reconciler retry later.
    #[tracing::instrument(fields(name = ?self.name_any(), status = ?self.status) skip(self))]
    async fn pool_check(&self) -> Result<Action, Error> {
        let pool = match self
            .pools_api()
            .get_node_pool(&self.spec.node(), &self.name_any())
            .await
        {
            Ok(response) => response,
            Err(clients::tower::Error::Response(response)) => {
                return if response.status() == clients::tower::StatusCode::NOT_FOUND {
                    if self.metadata.deletion_timestamp.is_some() {
                        tracing::debug!(name = ?self.name_any(), "deleted stopping checker");
                        Ok(Action::await_change())
                    } else {
                        tracing::warn!(pool = ?self.name_any(), "deleted by external event NOT recreating");
                        self.k8s_notify(
                            "Notfound",
                            "Check",
                            "The pool has been deleted through an external API request",
                            "Warning",
                        )
                            .await;

                        // We expected the control plane to have a spec for this pool. It didn't so
                        // set the pool_status in CRD to None.
                        self.mark_pool_not_found().await
                    }
                } else if response.status() == clients::tower::StatusCode::SERVICE_UNAVAILABLE || response.status() == clients::tower::StatusCode::REQUEST_TIMEOUT {
                    // Probably grpc server is not yet up
                    self.k8s_notify(
                        "Unreachable",
                        "Check",
                        "Could not reach Rest API service. Please check control plane health",
                        "Warning",
                    )
                        .await;
                    self.mark_pool_not_found().await
                } else {
                    self.k8s_notify(
                        "Missing",
                        "Check",
                        &format!("The pool information is not available: {response}"),
                        "Warning",
                    )
                        .await;
                    self.is_missing().await
                }
            }
            Err(clients::tower::Error::Request(_)) => {
                // Probably grpc server is not yet up
                return self.mark_pool_not_found().await
            }
        }.into_body();
        // As pool exists, set the status based on the presence of pool state.
        self.set_status_or_unknown(pool).await
    }

    /// If the pool, has a state we set that status to the CR and if it does not have a state
    /// we set the status as unknown so that we can try again later.
    async fn set_status_or_unknown(&self, pool: Pool) -> Result<Action, Error> {
        if pool.state.is_some() {
            if let Some(status) = &self.status {
                let mut new_status = DiskPoolStatus::from(pool);
                if self.metadata.deletion_timestamp.is_some() {
                    new_status.cr_state = CrPoolState::Terminating;
                }
                if status != &new_status {
                    // update the usage state such that users can see the values changes
                    // as replica's are added and/or removed.
                    let _ = self.patch_status(new_status).await;
                }
            }
        } else {
            return if self.metadata.deletion_timestamp.is_some() {
                self.mark_terminating_when_unknown().await
            } else {
                self.mark_unknown().await
            };
        }

        // always reschedule though
        Ok(Action::requeue(Duration::from_secs(self.ctx.interval)))
    }

    /// Post an event, typically these events are used to indicate that
    /// something happened. They should not be used to "log" generic
    /// information. Events are GC-ed by k8s automatically.
    ///
    /// action:
    ///     What action was taken/failed regarding to the Regarding object.
    /// reason:
    ///     This should be a short, machine understandable string that gives the
    ///     reason for the transition into the object's current status.
    /// message:
    ///     A human-readable description of the status of this operation.
    /// type_:
    ///     Type of this event (Normal, Warning), new types could be added in
    ///     the  future

    async fn k8s_notify(&self, action: &str, reason: &str, message: &str, type_: &str) {
        let client = self.ctx.k8s.clone();
        let ns = self.namespace().expect("must be namespaced");
        let e: Api<Event> = Api::namespaced(client.clone(), &ns);
        let pp = PostParams::default();
        let time = Utc::now();
        let contains = {
            self.event_info
                .lock()
                .unwrap()
                .contains(&message.to_string())
        };
        if !contains {
            self.event_info.lock().unwrap().push(message.to_string());
            let metadata = ObjectMeta {
                // the name must be unique for all events we post
                generate_name: Some(format!("{}.{:x}", self.name_any(), time.timestamp())),
                namespace: Some(ns),
                ..Default::default()
            };

            let _ = e
                .create(
                    &pp,
                    &Event {
                        event_time: Some(MicroTime(time)),
                        involved_object: self.object_ref(&()),
                        action: Some(action.into()),
                        reason: Some(reason.into()),
                        type_: Some(type_.into()),
                        metadata,
                        reporting_component: Some(WHO_AM_I_SHORT.into()),
                        reporting_instance: Some(
                            std::env::var("MY_POD_NAME")
                                .ok()
                                .unwrap_or_else(|| WHO_AM_I_SHORT.into()),
                        ),
                        message: Some(message.into()),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|error| error!(?error));
        }
    }

    /// Callback hooks for the finalizers
    async fn finalizer(&self) -> Result<Action, Error> {
        let _ = finalizer(
            &self.api(),
            "openebs.io/diskpool-protection",
            self.inner(),
            |event| async move {
                match event {
                    finalizer::Event::Apply(dsp) => Self::put_finalizer(dsp).await,
                    finalizer::Event::Cleanup(dsp) => {
                        match self
                            .pools_api()
                            .get_node_pool(&self.spec.node(), &self.name_any())
                            .await
                        {
                            Ok(pool) => {
                                if dsp.status.as_ref().unwrap().cr_state != CrPoolState::Terminating
                                {
                                    let new_status = DiskPoolStatus::terminating(pool.into_body());
                                    let _ = self.patch_status(new_status).await?;
                                }
                                Self::delete_finalizer(self.clone(), true).await
                            }
                            Err(clients::tower::Error::Response(response))
                                if response.status() == StatusCode::NOT_FOUND =>
                            {
                                Self::delete_finalizer(self.clone(), false).await
                            }
                            Err(error) => Err(error.into()),
                        }
                    }
                }
            },
        )
        .await
        .map_err(|e| error!(?e));
        Ok(Action::await_change())
    }
}

/// ensure the CRD is installed. This creates a chicken and egg problem. When the CRD is removed,
/// the operator will fail to list the CRD going into a error loop.
///
/// To prevent that, we will simply panic, and hope we can make progress after restart. Keep
/// running is not an option as the operator would be "running" and the only way to know something
/// is wrong would be to consult the logs.
async fn ensure_crd(k8s: Client) {
    let dsp: Api<k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition> = Api::all(k8s);

    // Check if there is an existing dsp crd. If yes, Replace it with new generated one.
    // Create new if it doesnt exist.
    let mut crd = DiskPool::crd();
    let crd_name = crd.metadata.name.as_ref().unwrap().as_str();
    let result = if let Ok(existing) = dsp.get(crd_name).await {
        crd.metadata.resource_version = existing.resource_version();
        info!(
            "Replacing CRD: {}",
            serde_json::to_string_pretty(&crd).unwrap()
        );

        let pp = PostParams::default();
        dsp.replace(crd_name, &pp, &crd).await
    } else {
        info!(
            "Creating CRD: {}",
            serde_json::to_string_pretty(&crd).unwrap()
        );

        let pp = PostParams::default();
        dsp.create(&pp, &crd).await
    };
    match result {
        Ok(o) => {
            info!(crd = ?o.name_any(), "created");
            // let the CRD settle this purely to avoid errors messages in the console
            // that are harmless but can cause some confusion maybe.
            tokio::time::sleep(Duration::from_secs(5)).await;
        }

        Err(e) => {
            error!("failed to create CRD error {}", e);
            tokio::time::sleep(Duration::from_secs(1)).await;
            std::process::exit(1);
        }
    }
}

const BACKOFF_PERIOD: u64 = 20;

/// Determine what we want to do when dealing with errors from the
/// reconciliation loop
fn error_policy(error: &Error, _ctx: Arc<OperatorContext>) -> Action {
    let duration = Duration::from_secs(match error {
        Error::Duplicate { timeout } | Error::SpecError { timeout, .. } => (*timeout).into(),

        Error::ReconcileError { .. } => {
            return Action::await_change();
        }
        _ => BACKOFF_PERIOD,
    });

    let when = Utc::now()
        .checked_add_signed(chrono::Duration::from_std(duration).unwrap())
        .unwrap();
    warn!(
        "{}, retry scheduled @{} ({} seconds from now)",
        error,
        when.to_rfc2822(),
        duration.as_secs()
    );
    info!("requeue after :{:?}", duration);
    Action::requeue(duration)
}

/// The main work horse
#[tracing::instrument(fields(name = %dsp.spec.node(), status = ?dsp.status) skip(dsp, ctx))]
async fn reconcile(dsp: Arc<DiskPool>, ctx: Arc<OperatorContext>) -> Result<Action, Error> {
    let dsp = ctx.upsert(ctx.clone(), dsp).await;
    let _ = dsp.finalizer().await;

    match dsp.status {
        Some(DiskPoolStatus {
            cr_state: CrPoolState::Creating,
            ..
        }) => dsp.create_or_import().await,

        Some(DiskPoolStatus {
            cr_state: CrPoolState::Created,
            ..
        })
        | Some(DiskPoolStatus {
            cr_state: CrPoolState::Terminating,
            ..
        }) => dsp.pool_check().await,
        // We use this state to indicate its a new CRD however, we could (and
        // perhaps should) use the finalizer callback.
        None => dsp.init_cr().await,
    }
}

async fn pool_controller(args: ArgMatches<'_>) -> anyhow::Result<()> {
    let k8s = Client::try_default().await?;
    let namespace = args.value_of("namespace").unwrap();
    ensure_crd(k8s.clone()).await;

    let dsp: Api<DiskPool> = Api::namespaced(k8s.clone(), namespace);
    let lp = ListParams::default();
    let url = Url::parse(args.value_of("endpoint").unwrap()).expect("endpoint is not a valid URL");

    let timeout: Duration = args
        .value_of("request-timeout")
        .unwrap()
        .parse::<humantime::Duration>()
        .expect("timeout value is invalid")
        .into();

    let cfg = clients::tower::Configuration::new(url, timeout, None, None, true, None).map_err(
        |error| {
            anyhow::anyhow!(
                "Failed to create openapi configuration, Error: '{:?}'",
                error
            )
        },
    )?;

    let context = OperatorContext {
        k8s,
        inventory: tokio::sync::RwLock::new(HashMap::new()),
        http: clients::tower::ApiClient::new(cfg),
        interval: args
            .value_of("interval")
            .unwrap()
            .parse::<humantime::Duration>()
            .expect("interval value is invalid")
            .as_secs(),
    };

    info!(
        "Starting DiskPool Operator (dsp) in namespace {}",
        namespace
    );

    Controller::new(dsp, lp)
        .run(reconcile, error_policy, Arc::new(context))
        .for_each(|res| async move {
            match res {
                Ok(o) => {
                    trace!(?o);
                }
                Err(e) => {
                    trace!(?e);
                }
            }
        })
        .await;

    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let matches = App::new(utils::package_description!())
        .author(clap::crate_authors!())
        .version(utils::version_info_str!())
        .settings(&[
            clap::AppSettings::ColoredHelp,
            clap::AppSettings::ColorAlways,
        ])
        .arg(
            Arg::with_name("interval")
                .short("i")
                .long("interval")
                .env("INTERVAL")
                .default_value(utils::CACHE_POLL_PERIOD)
                .help("specify timer based reconciliation loop"),
        )
        .arg(
            Arg::with_name("request-timeout")
                .short("t")
                .long("request-timeout")
                .env("REQUEST_TIMEOUT")
                .default_value(utils::DEFAULT_REQ_TIMEOUT)
                .help("the timeout for remote requests"),
        )
        .arg(
            Arg::with_name("retries")
                .short("r")
                .env("RETRIES")
                .default_value("10")
                .help("the number of retries before we set the resource into the error state"),
        )
        .arg(
            Arg::with_name("endpoint")
                .long("endpoint")
                .short("e")
                .env("ENDPOINT")
                .default_value("http://ksnode-1:30011")
                .help("an URL endpoint to the control plane's rest endpoint"),
        )
        .arg(
            Arg::with_name("namespace")
                .long("namespace")
                .short("-n")
                .env("NAMESPACE")
                .default_value("mayastor")
                .help("the default namespace we are supposed to operate in"),
        )
        .arg(
            Arg::with_name("jaeger")
                .short("-j")
                .long("jaeger")
                .env("JAEGER_ENDPOINT")
                .help("enable open telemetry and forward to jaeger"),
        )
        .arg(
            Arg::with_name("disable_device_validation")
                .long("disable-device-validation")
                .takes_value(false)
                .help("do not attempt to validate the block device prior to pool creation"),
        )
        .get_matches();

    utils::print_package_info!();

    let tags = utils::tracing_telemetry::default_tracing_tags(
        utils::raw_version_str(),
        env!("CARGO_PKG_VERSION"),
    );
    utils::tracing_telemetry::init_tracing(
        "dsp-operator",
        tags,
        matches.value_of("jaeger").map(|s| s.to_string()),
    );

    pool_controller(matches).await?;
    global::shutdown_tracer_provider();
    Ok(())
}

/// Normalize the disks if they have a schema, we dont want to change anything
/// or do any error checking -- the loop will converge to the error state eventually
fn normalize_disk(disk: &str) -> String {
    Url::parse(disk).map_or(disk.to_string(), |u| {
        u.to_file_path()
            .unwrap_or_else(|_| disk.into())
            .as_path()
            .display()
            .to_string()
    })
}

#[cfg(test)]
mod test {

    #[test]
    fn normalize_disk() {
        use super::*;
        let disks = vec![
            "aio:///dev/null",
            "uring:///dev/null",
            "uring://dev/null", // this URL is invalid
        ];

        assert_eq!(normalize_disk(disks[0]), "/dev/null");
        assert_eq!(normalize_disk(disks[1]), "/dev/null");
        assert_eq!(normalize_disk(disks[2]), "uring://dev/null");
    }
}
