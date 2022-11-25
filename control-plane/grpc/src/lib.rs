pub mod client;
pub mod context;
pub mod misc;
/// All server, client implementations and the traits
pub mod operations;
pub mod tracing;

/// Common module for all the misc operations
pub(crate) mod common {
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("v1.common");
}

/// Pool GRPC module for the autogenerated pool code
pub(crate) mod pool {
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("v1.pool");
}

/// Replica GRPC module for the autogenerated replica code
pub(crate) mod replica {
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("v1.replica");
}

/// Nexus GRPC module for the autogenerated pool code
/// The allow rule is to allow clippy to let the enum member names
/// to have same suffix and prefix
#[allow(clippy::enum_variant_names)]
pub(crate) mod nexus {
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("v1.nexus");
}

/// Volume GRPC module for the autogenerated replica code
#[allow(clippy::large_enum_variant)]
pub(crate) mod volume {
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("v1.volume");
}

/// Node GRPC module for the autogenerated node code
pub(crate) mod node {
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("v1.node");
}

/// Blockdevice GRPC module for the autogenerated blockdevice code
pub(crate) mod blockdevice {
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("v1.blockdevice");
}

/// Registry GRPC module for the autogenerated registry code
pub(crate) mod registry {
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("v1.registry");
}

/// JsonGrpc GRPC module for the autogenerated jsongrpc code
pub(crate) mod jsongrpc {
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("v1.jsongrpc");
}

/// Watch GRPC module for the autogenerated watch code
pub(crate) mod watch {
    #![allow(clippy::derive_partial_eq_without_eq)]
    #![allow(clippy::enum_variant_names)]
    tonic::include_proto!("v1.watch");
}

/// Cluster agent GRPC module for the autogenerated cluster-agent code
pub(crate) mod ha_cluster_agent {
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("v1.ha_cluster_agent");
}

/// Cluster agent GRPC module for the autogenerated node-agent code
pub(crate) mod ha_node_agent {
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("v1.ha_node_agent");
}

/// Cluster agent GRPC module for the autogenerated csi-node nvme code
pub mod csi_node_nvme {
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("v1.csi_node_nvme");
}
