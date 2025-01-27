use k8s_openapi::api::core::v1::Node as K8sNode;
use kube::{
    api::{Patch, PatchParams},
    Api,
};
use serde_json::Value;
use tracing::trace;

/// Patch a k8s node with the given patch.
pub(crate) async fn patch_k8s_node(
    client: &kube::client::Client,
    node_name: &str,
    node_patch: &Value,
) -> anyhow::Result<()> {
    let nodes: Api<K8sNode> = Api::all(client.clone());
    match nodes
        .patch(
            node_name,
            &PatchParams::default(),
            &Patch::Merge(node_patch),
        )
        .await
    {
        Ok(_) => trace!("Patched node: {} with patch: {}", node_name, node_patch),
        Err(error) => anyhow::bail!(
            "Failed to patch node: {} with patch: {}. {}",
            node_name,
            node_patch,
            error
        ),
    }
    Ok(())
}
