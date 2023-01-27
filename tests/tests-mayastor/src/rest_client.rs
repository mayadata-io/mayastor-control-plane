use openapi::tower::{client, client::Url};

/// Tower Rest Client
#[derive(Clone)]
pub(crate) struct RestClient {
    openapi_client_v0: client::direct::ApiClient,
    trace: bool,
}

impl RestClient {
    /// Get Autogenerated Openapi client v0
    pub fn v0(&self) -> client::direct::ApiClient {
        self.openapi_client_v0.clone()
    }
    /// creates a new client which uses the specified `url` and specified timeout
    /// uses the rustls connector if the url has the https scheme
    pub(crate) fn new_timeout(
        url: &str,
        trace: bool,
        bearer_token: Option<String>,
        timeout: std::time::Duration,
    ) -> anyhow::Result<Self> {
        let url: Url = url.parse()?;

        match url.scheme() {
            "https" => Self::new_https(url, timeout, bearer_token, trace),
            "http" => Self::new_http(url, timeout, bearer_token, trace),
            invalid => {
                let msg = format!("Invalid url scheme: {}", invalid);
                Err(anyhow::Error::msg(msg))
            }
        }
    }
    /// creates a new secure client
    fn new_https(
        url: Url,
        timeout: std::time::Duration,
        bearer_token: Option<String>,
        trace: bool,
    ) -> anyhow::Result<Self> {
        let cert_file = &std::include_bytes!("../../../control-plane/rest/certs/rsa/ca.cert")[..];

        let openapi_client_config =
            client::Configuration::new(url, timeout, bearer_token, Some(cert_file), trace, None)
                .map_err(|e| anyhow::anyhow!("Failed to create rest client config: '{:?}'", e))?;
        let openapi_client = client::direct::ApiClient::new(openapi_client_config);

        Ok(Self {
            openapi_client_v0: openapi_client,
            trace,
        })
    }
    /// creates a new client
    fn new_http(
        url: Url,
        timeout: std::time::Duration,
        bearer_token: Option<String>,
        trace: bool,
    ) -> anyhow::Result<Self> {
        let openapi_client_config =
            client::Configuration::new(url, timeout, bearer_token, None, trace, None)
                .map_err(|e| anyhow::anyhow!("Failed to create rest client config: '{:?}'", e))?;
        let openapi_client = client::direct::ApiClient::new(openapi_client_config);
        Ok(Self {
            openapi_client_v0: openapi_client,
            trace,
        })
    }
}
