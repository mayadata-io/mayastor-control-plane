mod authentication;
mod v0;

use crate::v0::CORE_CLIENT;
use actix_service::ServiceFactory;
use actix_web::{
    body::MessageBody,
    dev::{ServiceRequest, ServiceResponse},
    middleware, App, HttpServer,
};
use rustls::{Certificate, PrivateKey, ServerConfig};
use rustls_pemfile::{certs, rsa_private_keys};

use std::{fs::File, io::BufReader};
use structopt::StructOpt;
use utils::DEFAULT_GRPC_CLIENT_ADDR;

#[derive(Debug, StructOpt)]
#[structopt(version = utils::package_info!())]
pub(crate) struct CliArgs {
    /// The bind address for the REST interface (with HTTPS)
    /// Default: 0.0.0.0:8080
    #[structopt(long, default_value = "0.0.0.0:8080")]
    https: String,
    /// The bind address for the REST interface (with HTTP)
    #[structopt(long)]
    http: Option<String>,
    /// The Nats Server URL or address to connect to
    #[structopt(long, short, default_value = "nats://0.0.0.0:4222")]
    nats: String,

    /// The CORE gRPC Server URL or address to connect to the services.
    #[structopt(long, short = "z", default_value = DEFAULT_GRPC_CLIENT_ADDR)]
    core_grpc: Uri,

    /// Path to the certificate file
    #[structopt(long, short, required_unless = "dummy-certificates")]
    cert_file: Option<String>,
    /// Path to the key file
    #[structopt(long, short, required_unless = "dummy-certificates")]
    key_file: Option<String>,

    /// Use dummy HTTPS certificates (for testing)
    #[structopt(long, short, required_unless = "cert-file")]
    dummy_certificates: bool,

    /// Trace rest requests to the Jaeger endpoint agent
    #[structopt(long, short)]
    jaeger: Option<String>,

    /// Path to JSON Web KEY file used for authenticating REST requests
    #[structopt(long, required_unless = "no-auth")]
    jwk: Option<String>,

    /// Don't authenticate REST requests
    #[structopt(long, required_unless = "jwk")]
    no_auth: bool,

    /// The default timeout for backend requests issued by the REST Server
    #[structopt(long, short, default_value = utils::DEFAULT_REQ_TIMEOUT)]
    request_timeout: humantime::Duration,

    /// Add process service tags to the traces
    #[structopt(short, long, env = "TRACING_TAGS", value_delimiter=",", parse(try_from_str = common_lib::opentelemetry::parse_key_value))]
    tracing_tags: Vec<KeyValue>,

    /// Don't use minimum timeouts for specific requests
    #[structopt(long)]
    no_min_timeouts: bool,
}
impl CliArgs {
    fn args() -> Self {
        CliArgs::from_args()
    }
}

/// default timeout options for every bus request
fn bus_timeout_opts() -> TimeoutOptions {
    let timeout_opts =
        TimeoutOptions::new_no_retries().with_timeout(CliArgs::args().request_timeout.into());

    if CliArgs::args().no_min_timeouts {
        timeout_opts.with_req_timeout(None)
    } else {
        timeout_opts.with_req_timeout(RequestMinTimeout::default())
    }
}

use actix_web_opentelemetry::RequestTracing;
use common_lib::{
    mbus_api,
    mbus_api::{BusClient, RequestMinTimeout, TimeoutOptions},
    opentelemetry::default_tracing_tags,
};
use grpc::client::CoreClient;
use http::Uri;
use opentelemetry::{
    global,
    sdk::{propagation::TraceContextPropagator, trace::Tracer},
    KeyValue,
};

fn init_tracing() -> Option<Tracer> {
    if let Ok(filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    } else {
        tracing_subscriber::fmt().with_env_filter("info").init();
    }
    if let Some(agent) = CliArgs::args().jaeger {
        let mut tracing_tags = CliArgs::args().tracing_tags;
        tracing_tags.append(&mut default_tracing_tags(
            utils::git_version(),
            env!("CARGO_PKG_VERSION"),
        ));
        tracing_tags.dedup();
        tracing::info!("Using the following tracing tags: {:?}", tracing_tags);
        tracing::info!("Starting jaeger trace pipeline at {}...", agent);
        // Start a new jaeger trace pipeline
        global::set_text_map_propagator(TraceContextPropagator::new());
        common_lib::opentelemetry::set_jaeger_env();
        let tracer = opentelemetry_jaeger::new_pipeline()
            .with_agent_endpoint(agent)
            .with_service_name("rest-server")
            .with_tags(tracing_tags)
            .install_batch(opentelemetry::runtime::TokioCurrentThread)
            .expect("Should be able to initialise the exporter");
        Some(tracer)
    } else {
        None
    }
}

/// Extension trait for actix-web applications.
pub trait OpenApiExt<T> {
    /// configures the App with this version's handlers and openapi generation
    fn configure_api(
        self,
        config: &dyn Fn(actix_web::App<T>) -> actix_web::App<T>,
    ) -> actix_web::App<T>;
}

impl<T, B> OpenApiExt<T> for actix_web::App<T>
where
    B: MessageBody,
    T: ServiceFactory<
        ServiceRequest,
        Config = (),
        Response = ServiceResponse<B>,
        Error = actix_web::Error,
        InitError = (),
    >,
{
    fn configure_api(
        self,
        config: &dyn Fn(actix_web::App<T>) -> actix_web::App<T>,
    ) -> actix_web::App<T> {
        config(self)
    }
}

fn get_certificates() -> anyhow::Result<ServerConfig> {
    if CliArgs::args().dummy_certificates {
        get_dummy_certificates()
    } else {
        // guaranteed to be `Some` by the require_unless attribute
        let cert_file = CliArgs::args().cert_file.expect("cert_file is required");
        let key_file = CliArgs::args().key_file.expect("key_file is required");
        let cert_file = &mut BufReader::new(File::open(cert_file)?);
        let key_file = &mut BufReader::new(File::open(key_file)?);
        load_certificates(cert_file, key_file)
    }
}

fn get_dummy_certificates() -> anyhow::Result<ServerConfig> {
    let cert_file = &mut BufReader::new(&std::include_bytes!("../../certs/rsa/user.chain")[..]);
    let key_file = &mut BufReader::new(&std::include_bytes!("../../certs/rsa/user.rsa")[..]);

    load_certificates(cert_file, key_file)
}

fn load_certificates<R: std::io::Read>(
    cert_file: &mut BufReader<R>,
    key_file: &mut BufReader<R>,
) -> anyhow::Result<ServerConfig> {
    let config = ServerConfig::builder().with_safe_defaults();
    let cert_chain = certs(cert_file).map_err(|_| {
        anyhow::anyhow!("Failed to retrieve certificates from the certificate file",)
    })?;
    let mut keys = rsa_private_keys(key_file).map_err(|_| {
        anyhow::anyhow!("Failed to retrieve the rsa private keys from the key file",)
    })?;
    if keys.is_empty() {
        anyhow::bail!("No keys found in the keys file");
    }
    let config = config.with_no_client_auth().with_single_cert(
        cert_chain.into_iter().map(Certificate).collect(),
        PrivateKey(keys.remove(0)),
    )?;
    Ok(config)
}

fn get_jwk_path() -> Option<String> {
    match CliArgs::args().jwk {
        Some(path) => Some(path),
        None => match CliArgs::args().no_auth {
            true => None,
            false => panic!("Cannot authenticate without a JWK file"),
        },
    }
}

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    // need to keep the jaeger pipeline tracer alive, if enabled
    let _tracer = init_tracing();
    utils::print_package_info!();

    let app = move || {
        App::new()
            .wrap(RequestTracing::new())
            .wrap(middleware::Logger::default())
            .app_data(authentication::init(get_jwk_path()))
            .configure_api(&v0::configure_api)
    };

    mbus_api::message_bus_init_options(
        BusClient::RestServer,
        CliArgs::args().nats,
        bus_timeout_opts(),
    )
    .await;

    // Initialise the core client to be used in rest
    CORE_CLIENT
        .set(CoreClient::new(CliArgs::args().core_grpc, None).await)
        .ok()
        .expect("Expect to be initialised only once");

    let server = HttpServer::new(app).bind_rustls(CliArgs::args().https, get_certificates()?)?;
    let result = if let Some(http) = CliArgs::args().http {
        server.bind(http).map_err(anyhow::Error::from)?
    } else {
        server
    }
    .run()
    .await
    .map_err(|e| e.into());

    global::shutdown_tracer_provider();

    result
}
