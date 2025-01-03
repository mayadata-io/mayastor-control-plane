use clap::Parser;
use openapi::tower::client::Url;

use plugin::{operations::Operations, rest_wrapper::RestClient, ExecuteOperation};
use snafu::ResultExt;
use std::env;

#[derive(clap::Parser, Debug)]
#[clap(name = utils::package_description!(), version = utils::version_info_str!())]
#[group(skip)]
struct CliArgs {
    /// The rest endpoint to connect to.
    #[clap(global = true, long, short, default_value = "http://localhost:8081")]
    rest: Url,

    /// The operation to be performed.
    #[clap(subcommand)]
    operation: Operations,

    #[clap(flatten)]
    args: plugin::CliArgs,
}

#[tokio::main]
async fn main() {
    let cli_args = CliArgs::args();
    let _trace_flush = cli_args.args.init_tracing();
    if let Err(error) = cli_args.execute().await {
        eprintln!("{error}");
        std::process::exit(-1);
    }
}

#[derive(Debug, snafu::Snafu)]
enum Error {
    #[snafu(display("Failed to initialise the REST client. Error {source}"))]
    RestClient { source: anyhow::Error },
    #[snafu(display("{source}"))]
    Resources { source: plugin::resources::Error },
}

impl CliArgs {
    fn args() -> Self {
        CliArgs::parse()
    }

    async fn execute(&self) -> Result<(), Error> {
        // todo: client connection is lazy, we should do sanity connection test here.
        //  Example, we can use use rest liveness probe.
        RestClient::init(
            self.rest.clone(),
            self.args.jaeger.is_some(),
            *self.args.timeout,
        )
        .context(RestClientSnafu)?;
        self.operation
            .execute(CliArgs::args().args, Some(&CliArgs::args().args.output))
            .await
            .context(ResourcesSnafu)
    }
}
