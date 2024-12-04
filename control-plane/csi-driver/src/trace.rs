/// A csi request which can be traced.
pub struct CsiRequest<'a> {
    request: &'a str,
    start: std::time::Instant,
}

impl<'a> CsiRequest<'a> {
    /// Add new request and info trace it.
    pub fn new_info(request: &'a str) -> Self {
        tracing::info!("[ CSI ] {request} Request started");

        Self::new(request)
    }
    /// Add new request and debug trace it.
    pub fn new_dbg(request: &'a str) -> Self {
        tracing::debug!("[ CSI ] {request} Request started");

        Self::new(request)
    }
    /// Add new request and trace trace it.
    pub fn new_trace(request: &'a str) -> Self {
        tracing::trace!("[ CSI ] {request} Request started");

        Self::new(request)
    }

    fn new(request: &'a str) -> Self {
        Self {
            request,
            start: std::time::Instant::now(),
        }
    }

    /// Get completion log message.
    pub fn log_str(self) -> String {
        format!(
            "[ CSI ] {} Request completed successfully after {:?}",
            self.request,
            self.start.elapsed()
        )
    }

    /// Log completion info.
    pub fn info_ok(self) {
        tracing::info!("{}", self.log_str())
    }
}
