/// The CSI plugin's name.
pub use utils::csi_plugin_name;

/// The topology label used to identify a node as a csi-node.
pub fn csi_node_selector() -> String {
    format!("{}={}", csi_node_topology_key(), csi_node_topology_val())
}

type Selector = std::collections::HashMap<String, String>;
/// Parses node selector labels from clap args into its KeyValue pair.
pub fn csi_node_selector_parse<'a, I: Iterator<Item = &'a str>>(
    values: Option<I>,
) -> anyhow::Result<Selector> {
    match values {
        Some(values) => {
            let values = values.collect::<Vec<_>>();
            let selector = values
                .iter()
                .map(|source| match source.split_once('=') {
                    None => Err(anyhow::anyhow!(
                        "node-selector labels must be in the format: 'Key=Value'"
                    )),
                    Some((key, value)) => Ok((key.to_string(), value.to_string())),
                })
                .collect::<anyhow::Result<Selector>>()?;
            anyhow::ensure!(
                values.len() == selector.len(),
                "Node selector labels must be unique"
            );
            Ok(selector)
        }
        None => Ok(Selector::from([(
            csi_node_topology_key().to_string(),
            csi_node_topology_val().to_string(),
        )])),
    }
}
/// The topology key added by the csi-node plugin.
pub fn csi_node_topology_key() -> String {
    use utils::constants::PRODUCT_DOMAIN_NAME;
    format!("{PRODUCT_DOMAIN_NAME}/csi-node")
}
/// The topology value added by the csi-node plugin.
pub fn csi_node_topology_val() -> &'static str {
    utils::constants::PRODUCT_NAME
}

/// The topology label used to uniquely identify a node.
/// The csi-driver (node,controller) and the io-engine must pick the same value.
/// The nodename key assigned to each node.
pub fn node_name_topology_key() -> String {
    use utils::constants::PRODUCT_DOMAIN_NAME;
    format!("{PRODUCT_DOMAIN_NAME}/nodename")
}

/// Volume Parameters parsed from context.
pub use context::{CreateParams, Parameters, PublishParams};

/// The node plugin exported components.
pub mod node;

/// The csi driver components.
pub use rpc::csi;

/// The volume contexts.
pub mod context;
/// Filesystem Operations.
pub mod filesystem;
/// Volume concurrency limiter.
pub mod limiter;
/// Contains tools to advertise the same set of capabilities across different
/// CSI microservices.
pub mod plugin_capabilities;
/// Request Tracing.
pub mod trace;
