use crate::{
    api::{StoreKvWatcher, WatchCallback, WatchCbArg, WatchKey, WatchResult, WatcherCtx},
    Error,
};
use etcd_client::{ResponseHeader, WatchResponse};
use std::{collections::BTreeMap, time::Duration};
use tokio::sync::mpsc::error::TryRecvError;

/// We expect the initial watch setup to complete within this timeframe.
const SETUP_TMO: Duration = Duration::from_secs(10);
/// There's very little benefit in watching a subset... so we set a reasonably high
/// watch setup duration, whilst allowing us to timeout and retry if the connection
/// is blocked for some reason.
const WATCH_ALL_TMO: Duration = Duration::from_secs(15);
/// When a watch is requested, we expect etcd to accept it within this timeframe, and return
/// a created event.
const WATCH_ACCEPT_TMO: Duration = Duration::from_secs(10);
/// Timeout for response when checking the client status.
const HEALTH_CHECK_TMO: Duration = Duration::from_secs(5);
/// We should be receiving progress reports every 10 minutes or so.
const NO_PROGRESS_TMO: Duration = Duration::from_secs(60 * 20);

enum WatchRequest<Ctx: WatcherCtx> {
    Register(WatchRegister<Ctx>),
    Unregister { prefix: String },
}
struct WatchRegister<Ctx: WatcherCtx> {
    prefix: String,
    rev: Option<i64>,
    ctx: Ctx,
}

struct WatchConfigData<Ctx: WatcherCtx> {
    rev: i64,
    ctx: Ctx,
}

struct WatchConfig<Ctx: WatcherCtx> {
    // The keys which we're tasked with watching over and the last revision we've received.
    keys: BTreeMap<String, WatchConfigData<Ctx>>,
    // "global" watcher callback.
    ctx_cb: Box<dyn WatchCallback<Ctx>>,
    // The revision reported by the last watch event.
    rev: Option<i64>,
    // Timestamp of the last watch event.
    updated: Option<std::time::Instant>,
    closed: bool,
}
impl<Ctx: WatcherCtx> WatchConfig<Ctx> {
    fn register(&mut self, register: WatchRegister<Ctx>) {
        tracing::info!("Adding watch key: {}", register.prefix);
        let rev = register.rev.unwrap_or_default();
        let data = WatchConfigData {
            rev,
            ctx: register.ctx,
        };
        self.keys.insert(register.prefix, data);
    }
    fn unregister(&mut self, key: &str) {
        tracing::info!("Removing watch key: {key}");
        self.keys.remove(key);
    }
    fn update(&mut self, key: &str, rev: i64, updated_key: &str) -> WatchResult {
        let Some(value) = self.keys.get_mut(key) else {
            // todo: if we can't find the key, should we request watch removal?
            return WatchResult::Continue;
        };

        let old_rev = value.rev;
        tracing::trace!("Updating {key} from revision {old_rev} to {rev} due to {updated_key}");
        value.rev = rev;

        // callback and update the data, ex: volume update callback!
        (self.ctx_cb)(WatchCbArg {
            updated_key,
            key_prefix: key,
            cb_ctx: &value.ctx,
        })
    }
    fn update_rev(&mut self, header: Option<&ResponseHeader>) {
        if let Some(header) = header {
            self.rev = Some(header.revision());
        }
    }
    fn update_response(&mut self, response: &WatchResponse) {
        self.update_rev(response.header());
        self.updated = Some(std::time::Instant::now());
    }
}

#[derive(Debug)]
enum WatchState {
    // Waiting to be given something to watch!
    Idle,
    // Watching the state
    Watching(StreamedWatcher),
    // Attempting to establish initial connection
    Connecting,
    // Attempting to re-establish connection
    Reconnecting(u32),
    // Closing
    Closed,
}

pub(crate) struct EtcdWatchRunner<Ctx: WatcherCtx> {
    client: etcd_client::Client,
    config: WatchConfig<Ctx>,
}
#[derive(Debug)]
struct StreamedWatcher {
    watcher: etcd_client::Watcher,
    stream: etcd_client::WatchStream,
    // Next watch ID to use.
    watched_next: std::ops::Range<i64>,
    // Watch IDs which we've issued.
    watch_ids: BTreeMap<i64, String>,
    // Watch IDs which we are waiting for etcd to accept.
    waiting_ids: BTreeMap<i64, std::time::Instant>,
    // Mapping of key to watch id.
    watch_keys: BTreeMap<String, i64>,
}
impl StreamedWatcher {
    async fn new(client: &mut etcd_client::Client) -> Result<Self, etcd_client::Error> {
        let (mut watcher, stream) = client.watch("/watch-setup", None).await?;
        watcher.cancel_by_id(watcher.watch_id()).await?;
        Ok(Self {
            watcher,
            stream,
            watched_next: 1..i64::MAX,
            watch_ids: Default::default(),
            waiting_ids: Default::default(),
            watch_keys: Default::default(),
        })
    }
    async fn watch_key_prefixes<Ctx: WatcherCtx>(
        &mut self,
        data: &WatchConfig<Ctx>,
    ) -> Result<(), etcd_client::Error> {
        let rev = data.rev;
        for (key, cfg) in &data.keys {
            self.watch_key_prefix(key.clone(), rev.or(Some(cfg.rev)))
                .await?;
        }
        Ok(())
    }
    async fn watch_key_prefix(
        &mut self,
        key: String,
        rev: Option<i64>,
    ) -> Result<(), etcd_client::Error> {
        let rev = rev.unwrap_or_default();
        let watch_id = self.watched_next.next().unwrap();
        tracing::trace!(
            key,
            rev,
            watch_id,
            "Adding watched key prefix to the stream"
        );
        let options = etcd_client::WatchOptions::new()
            .with_prefix()
            .with_start_revision(rev)
            .with_progress_notify()
            .with_watch_id(watch_id);
        self.watcher.watch(key.clone(), Some(options)).await?;
        self.watch_ids.insert(watch_id, key.clone());
        self.watch_keys.insert(key, watch_id);
        self.waiting_ids.insert(watch_id, std::time::Instant::now());
        Ok(())
    }
    async fn del_watch_key_prefix(&mut self, key: &str) {
        tracing::trace!(key, "Removing key prefix watch");
        if let Some(watch_id) = self.watch_keys.remove(key) {
            self.watcher.cancel_by_id(watch_id).await.unwrap();
            self.watch_ids.remove(&watch_id);
            self.waiting_ids.remove(&watch_id);
        }
    }
    async fn unregister_key<Ctx: WatcherCtx>(
        &mut self,
        key: String,
        config: &mut WatchConfig<Ctx>,
    ) {
        config.unregister(&key);
        self.del_watch_key_prefix(&key).await;
    }

    async fn notify_events<Ctx: WatcherCtx>(
        mut self,
        config: &mut WatchConfig<Ctx>,
        response: WatchResponse,
    ) -> WatchState {
        let watch_id = response.watch_id();
        let registrant_key = self.watch_ids.get(&watch_id);
        tracing::trace!(?response, "Watch Event");

        if response.created() && response.canceled() {
            // A watch failed to be setup, it's better to error out and start the watch again...
            tracing::error!("A watch setup likely failed. We're reconnecting again...");
            tokio::time::sleep(Duration::from_secs(2)).await;
            return WatchState::Reconnecting(0);
        }
        if response.created() {
            self.waiting_ids.remove(&response.watch_id());
        }
        config.update_response(&response);

        let mut unregister: Option<String> = None;
        for event in response.events() {
            let Some(kv) = event.kv() else {
                continue;
            };
            let (Ok(k), Ok(v)) = (kv.key_str(), kv.value_str()) else {
                continue;
            };

            tracing::trace!(
                "k: {k}, v: {v}, rev: {}, wid: {watch_id}",
                kv.mod_revision()
            );
            let Some(registrant_key) = registrant_key else {
                continue;
            };

            if matches!(
                config.update(registrant_key, kv.mod_revision(), k),
                WatchResult::Remove
            ) && unregister.is_none()
            {
                unregister = Some(registrant_key.clone());
            }
        }

        if let Some(registrant_key) = unregister {
            // the callback may request for the watch to be unregistered
            self.unregister_key(registrant_key, config).await;
        }

        WatchState::Watching(self)
    }
}

impl<Ctx: WatcherCtx> EtcdWatchRunner<Ctx> {
    fn new<W: WatchCallback<Ctx>>(client: etcd_client::Client, ctx_cb: W) -> Self {
        Self {
            client,
            config: WatchConfig {
                keys: BTreeMap::new(),
                closed: false,
                ctx_cb: Box::new(ctx_cb),
                rev: None,
                updated: None,
            },
        }
    }

    fn watch(mut self) -> EtcdWatcher<Ctx> {
        let (send, mut request) = tokio::sync::mpsc::unbounded_channel::<WatchRequest<Ctx>>();
        tokio::spawn(async move {
            let mut state = WatchState::Idle;

            loop {
                state = match state {
                    WatchState::Idle => self.handle_request_idle(request.recv().await),
                    WatchState::Connecting => self.handle_connecting().await,

                    WatchState::Watching(mut watcher) => {
                        let interval = tokio::time::sleep(WATCH_ACCEPT_TMO);
                        tokio::select! {
                            cmd = request.recv() => self.handle_request(watcher, cmd).await,
                            rsp = watcher.stream.message() => self.handle_watch_event(watcher, rsp).await,
                            _ = interval => self.health_check(watcher).await
                        }
                    }

                    WatchState::Reconnecting(attempts) => {
                        self.handle_reconnecting(attempts, &mut request).await
                    }

                    WatchState::Closed => {
                        tracing::warn!("Request channel has been closed, the watch is stopped...");
                        break;
                    }
                }
            }
        });
        EtcdWatcher { watcher: send }
    }

    /// Handles a new [`WatchRequest`] when we are in the [`WatchState::Idle`] state, awaiting the very
    /// first request.
    fn handle_request_idle(&mut self, op: Option<WatchRequest<Ctx>>) -> WatchState {
        match op {
            None => WatchState::Closed,
            Some(WatchRequest::Register(register)) => {
                self.config.register(register);
                WatchState::Connecting
            }
            Some(WatchRequest::Unregister { prefix }) => {
                self.config.unregister(&prefix);
                WatchState::Idle
            }
        }
    }

    /// Handles a new [`WatchRequest`] when we are already in the [`WatchState::Watching`] state.
    /// In this state, we may receive new requests, and we also process watch events.
    async fn handle_request(
        &mut self,
        mut watcher: StreamedWatcher,
        request: Option<WatchRequest<Ctx>>,
    ) -> WatchState {
        match request {
            None => {
                watcher.watcher.cancel().await.ok();
                WatchState::Closed
            }
            Some(WatchRequest::Register(register)) => {
                let prefix = register.prefix.clone();
                let rev = register.rev;
                self.config.register(register);
                match watcher.watch_key_prefix(prefix, rev).await {
                    Ok(_) => WatchState::Watching(watcher),
                    Err(error) => self.handle_error(&error, "Failed to watch the given key"),
                }
            }
            Some(WatchRequest::Unregister { prefix }) => {
                self.config.unregister(&prefix);
                watcher.del_watch_key_prefix(&prefix).await;
                WatchState::Watching(watcher)
            }
        }
    }

    /// When an error is encountered we go into the [`WatchState::Reconnecting`] state where we
    /// attempt to re-establish the watch connection stream.
    async fn handle_reconnecting(
        &mut self,
        mut attempts: u32,
        request: &mut tokio::sync::mpsc::UnboundedReceiver<WatchRequest<Ctx>>,
    ) -> WatchState {
        if attempts % 10 == 0 {
            tracing::warn!("Starting etcd watch stream reconnection loop, attempt: {attempts}");
        }

        // ensure we keep processing incoming requests, even during signal loss
        self.drip_pending_requests(request);

        let delay = (Duration::from_secs(1) * attempts).min(Duration::from_secs(5));
        attempts += 1;
        tokio::time::sleep(delay).await;
        match self.handle_connecting().await {
            WatchState::Reconnecting(_) => WatchState::Reconnecting(attempts),
            WatchState::Watching(w) => {
                tracing::info!("Successfully re-connected the watch stream");
                WatchState::Watching(w)
            }
            state => state,
        }
    }

    /// When the connection is lost and we're attempting to reconnect, ensure the requests keep
    /// getting processed.
    fn drip_pending_requests(
        &mut self,
        request: &mut tokio::sync::mpsc::UnboundedReceiver<WatchRequest<Ctx>>,
    ) {
        for _ in 0..100 {
            match request.try_recv() {
                Ok(WatchRequest::Register(register)) => {
                    self.config.register(register);
                }
                Ok(WatchRequest::Unregister { prefix }) => {
                    self.config.unregister(&prefix);
                }
                Err(TryRecvError::Disconnected) => {
                    self.config.closed = true;
                    break;
                }
                Err(TryRecvError::Empty) => {
                    break;
                }
            }
        }
    }

    /// When in the [`WatchState::Connecting`] state we attempt the first connection, and we connect
    /// all the registered keys in the [`WatchConfig].
    async fn handle_connecting(&mut self) -> WatchState {
        if self.config.closed {
            return WatchState::Closed;
        }
        match tokio::time::timeout(SETUP_TMO, StreamedWatcher::new(&mut self.client)).await {
            Ok(Ok(mut w)) => {
                match tokio::time::timeout(WATCH_ALL_TMO, w.watch_key_prefixes(&self.config)).await
                {
                    Ok(Ok(_)) => WatchState::Watching(w),
                    Err(error) => self.handle_error(&error, "Watch setup on all keys"),
                    Ok(Err(error)) => self.handle_error(&error, "Watch setup on all keys"),
                }
            }
            Err(error) => self.handle_error(&error, "Setting up watch stream"),
            Ok(Err(error)) => self.handle_error(&error, "Setting up watch stream"),
        }
    }

    /// When an error is encountered we log it and transition into the [`WatchState::Reconnecting`]
    /// state in hopes of re-establishing the watch stream.
    fn handle_error(&self, error: &dyn std::error::Error, message: &'static str) -> WatchState {
        tracing::error!(%error, "{message}");
        WatchState::Reconnecting(0)
    }
    /// When an error is encountered we log it and transition into the [`WatchState::Reconnecting`]
    /// state in hopes of re-establishing the watch stream.
    fn handle_error_msg(&self, error: impl Into<String>) -> WatchState {
        tracing::error!("{}", error.into());
        WatchState::Reconnecting(0)
    }

    /// Watch events are received as [`WatchResponse`].
    async fn handle_watch_event(
        &mut self,
        watcher: StreamedWatcher,
        rsp: Result<Option<WatchResponse>, etcd_client::Error>,
    ) -> WatchState {
        match rsp {
            Ok(Some(resp)) => watcher.notify_events(&mut self.config, resp).await,
            Ok(None) => {
                // todo: can this happen? What do do here?
                self.handle_error_msg("Unexpected EOF for the watch stream")
            }
            Err(error) => self.handle_error(&error, "Polling watch stream error"),
        }
    }

    async fn health_check(&mut self, watcher: StreamedWatcher) -> WatchState {
        for (id, registered) in &watcher.waiting_ids {
            if registered.elapsed() > WATCH_ACCEPT_TMO {
                return self.handle_error_msg(format!(
                    "Watch id {id} has not been accepted by etcd after {:?}",
                    registered.elapsed()
                ));
            }
        }
        match tokio::time::timeout(HEALTH_CHECK_TMO, self.client.status()).await {
            Ok(Ok(status)) => {
                self.config.update_rev(status.header());
                if let Some(updated) = self.config.updated {
                    if updated.elapsed() > NO_PROGRESS_TMO {
                        return self.handle_error_msg(format!(
                            "No progress notification within {NO_PROGRESS_TMO:?}"
                        ));
                    }
                }
                WatchState::Watching(watcher)
            }
            Ok(Err(error)) => self.handle_error(&error, "Fetching client status"),
            Err(error) => self.handle_error(&error, "Fetching client status"),
        }
    }
}

/// An etcd watcher stream frontend which can be updated by adding or removing keys to be
/// watched by the stream.
/// The stream lifecycle is maintained by us, recreating the stream is necessary.
pub struct EtcdWatcher<Ctx: WatcherCtx> {
    watcher: tokio::sync::mpsc::UnboundedSender<WatchRequest<Ctx>>,
}
impl<Ctx: WatcherCtx> EtcdWatcher<Ctx> {
    /// Create a new instance using the given client.
    pub(crate) fn new<W: WatchCallback<Ctx>>(client: etcd_client::Client, ctx_cb: W) -> Self {
        let runner = EtcdWatchRunner::new(client, ctx_cb);
        runner.watch()
    }
}

impl<Ctx: WatcherCtx> StoreKvWatcher<Ctx> for EtcdWatcher<Ctx> {
    fn watch(&self, key: WatchKey, ctx: Ctx) -> Result<(), Error> {
        self.watcher
            .send(WatchRequest::Register(WatchRegister {
                prefix: key.prefix,
                rev: key.rev,
                ctx,
            }))
            .map_err(|_| Error::WatcherClosed {})
    }

    fn unwatch(&self, key: WatchKey) -> Result<(), Error> {
        self.watcher
            .send(WatchRequest::Unregister { prefix: key.prefix })
            .map_err(|_| Error::WatcherClosed {})
    }

    fn abort(self) {
        // the only strong count channel is dropped, so channel must be closed
        assert_eq!(self.watcher.strong_count(), 1);
    }
}
