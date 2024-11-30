//! This allows us to send futures from within csi-driver to the tokio
//! runtime to do whatever it needs to do.

use once_cell::sync::Lazy;
use tokio::task::JoinHandle;
use tracing::trace;

/// Spawn a future that might block on a separate worker thread the
/// number of threads available is determined by max_blocking_threads.
pub(crate) fn spawn_blocking<F, R>(f: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    RUNTIME.spawn_blocking(f)
}

/// Runtime Instance.
pub(crate) struct Runtime {
    rt: tokio::runtime::Runtime,
}

static RUNTIME: Lazy<Runtime> = Lazy::new(|| {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .max_blocking_threads(128)
        .build()
        .unwrap();

    Runtime::new(rt)
});

impl Runtime {
    fn new(rt: tokio::runtime::Runtime) -> Self {
        Self { rt }
    }
    fn spawn_blocking<F, R>(&self, f: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let handle = self.rt.handle().clone();
        handle.spawn_blocking(|| {
            trace!("Spawned a blocking thread..");
            f()
        })
    }
}
