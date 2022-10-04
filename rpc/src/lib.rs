extern crate bytes;
extern crate prost;
extern crate prost_derive;
extern crate serde;
extern crate serde_derive;
extern crate serde_json;
extern crate tonic;
#[allow(dead_code)]
#[allow(clippy::type_complexity)]
#[allow(clippy::unit_arg)]
#[allow(clippy::redundant_closure)]
#[allow(clippy::upper_case_acronyms)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub mod io_engine {
    use std::{
        net::{SocketAddr, TcpStream},
        str::FromStr,
        time::Duration,
    };
    use strum_macros::{EnumString, ToString};
    use tonic::transport::Channel;

    /// AutoGenerated Io Engine Client V0.
    pub use mayastor_client::MayastorClient as IoEngineClientV0;
    /// Io Engine Client V1, with its components.
    #[derive(Clone)]
    struct IoEngineClientV1<Channel> {
        /// AutoGenerated Io Engine V1 Nexus Client.
        nexus: super::v1::nexus::nexus_rpc_client::NexusRpcClient<Channel>,
    }
    impl IoEngineClientV1<Channel> {
        fn new(channel: Channel) -> Self {
            Self {
                nexus: super::v1::nexus::nexus_rpc_client::NexusRpcClient::new(channel),
            }
        }
    }

    /// Nvme ANA Parse Error.
    #[derive(Debug)]
    pub enum Error {
        ParseError,
    }

    impl From<()> for Null {
        fn from(_: ()) -> Self {
            Self {}
        }
    }

    impl FromStr for NvmeAnaState {
        type Err = Error;
        fn from_str(state: &str) -> Result<Self, Self::Err> {
            match state {
                "optimized" => Ok(Self::NvmeAnaOptimizedState),
                "non_optimized" => Ok(Self::NvmeAnaNonOptimizedState),
                "inaccessible" => Ok(Self::NvmeAnaInaccessibleState),
                _ => Err(Error::ParseError),
            }
        }
    }

    include!(concat!(env!("OUT_DIR"), "/mayastor.rs"));

    /// The IoEngine grpc api versions.
    #[derive(Default, Debug, EnumString, ToString, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
    #[strum(ascii_case_insensitive)]
    pub enum IoEngineApiVersion {
        #[default]
        V0,
        V1,
    }
    impl IoEngineApiVersion {
        /// Convert a list of `Self` to a `String` argument.
        pub fn vec_to_str(vec: Vec<Self>) -> String {
            vec.into_iter().fold(String::new(), |mut acc, n| {
                acc.push_str(&n.to_string());
                acc.push(',');
                acc
            })
        }
    }

    /// A versioned IoEngine client
    #[derive(Clone)]
    enum IoEngineClient {
        V0(IoEngineClientV0<Channel>),
        V1(IoEngineClientV1<Channel>),
    }

    /// Test Rpc Handle to connect to an io-engine instance via an endpoint.
    /// Gives access to the io-engine client and the bdev client.
    #[derive(Clone)]
    pub struct RpcHandle {
        name: String,
        endpoint: SocketAddr,
        io_engine: IoEngineClient,
    }

    trait Rpc: RpcHealth + RpcNexus {}
    #[tonic::async_trait]
    trait RpcHealth {
        /// Ping the Io Engine for responsiveness.
        async fn ping(&mut self) -> Result<(), tonic::Status>;
    }
    #[tonic::async_trait]
    trait RpcNexus {
        /// List all Nexuses.
        async fn list_nexuses(&mut self) -> Result<String, tonic::Status>;
        /// Fault a Nexus Child.
        async fn fault_child(&mut self, uuid: &str, uri: &str) -> Result<(), tonic::Status>;
        /// Add a Nexus Child.
        async fn add_child(
            &mut self,
            uuid: &str,
            uri: &str,
            norebuild: bool,
        ) -> Result<String, tonic::Status>;
        /// Remove a Nexus Child.
        async fn remove_child(&mut self, uuid: &str, uri: &str) -> Result<(), tonic::Status>;
    }
    #[tonic::async_trait]
    impl RpcHealth for IoEngineClientV0<Channel> {
        async fn ping(&mut self) -> Result<(), tonic::Status> {
            self.list_nexus_v2(Null {}).await?;
            Ok(())
        }
    }
    #[tonic::async_trait]
    impl RpcNexus for IoEngineClientV0<Channel> {
        async fn list_nexuses(&mut self) -> Result<String, tonic::Status> {
            let nexuses = self.list_nexus_v2(Null {}).await?;
            Ok(format!("{:?}", nexuses.into_inner().nexus_list))
        }

        async fn fault_child(&mut self, uuid: &str, uri: &str) -> Result<(), tonic::Status> {
            self.fault_nexus_child(FaultNexusChildRequest {
                uuid: uuid.to_string(),
                uri: uri.to_string(),
            })
            .await?;
            Ok(())
        }

        async fn add_child(
            &mut self,
            uuid: &str,
            uri: &str,
            norebuild: bool,
        ) -> Result<String, tonic::Status> {
            let child = self
                .add_child_nexus(AddChildNexusRequest {
                    uuid: uuid.to_string(),
                    uri: uri.to_string(),
                    norebuild,
                })
                .await?;
            Ok(format!("{:?}", child.into_inner()))
        }

        async fn remove_child(&mut self, uuid: &str, uri: &str) -> Result<(), tonic::Status> {
            self.remove_child_nexus(RemoveChildNexusRequest {
                uuid: uuid.to_string(),
                uri: uri.to_string(),
            })
            .await?;
            Ok(())
        }
    }
    #[tonic::async_trait]
    impl RpcHealth for IoEngineClientV1<Channel> {
        async fn ping(&mut self) -> Result<(), tonic::Status> {
            self.nexus
                .list_nexus(super::v1::nexus::ListNexusOptions::default())
                .await?;
            Ok(())
        }
    }
    #[tonic::async_trait]
    impl RpcNexus for IoEngineClientV1<Channel> {
        async fn list_nexuses(&mut self) -> Result<String, tonic::Status> {
            let nexuses = self
                .nexus
                .list_nexus(super::v1::nexus::ListNexusOptions::default())
                .await?;
            Ok(format!("{:?}", nexuses.into_inner().nexus_list))
        }

        async fn fault_child(&mut self, uuid: &str, uri: &str) -> Result<(), tonic::Status> {
            self.nexus
                .fault_nexus_child(super::v1::nexus::FaultNexusChildRequest {
                    uuid: uuid.to_string(),
                    uri: uri.to_string(),
                })
                .await?;
            Ok(())
        }

        async fn add_child(
            &mut self,
            uuid: &str,
            uri: &str,
            norebuild: bool,
        ) -> Result<String, tonic::Status> {
            let child = self
                .nexus
                .add_child_nexus(super::v1::nexus::AddChildNexusRequest {
                    uuid: uuid.to_string(),
                    uri: uri.to_string(),
                    norebuild,
                })
                .await?;
            Ok(format!("{:?}", child.into_inner()))
        }

        async fn remove_child(&mut self, uuid: &str, uri: &str) -> Result<(), tonic::Status> {
            self.nexus
                .remove_child_nexus(super::v1::nexus::RemoveChildNexusRequest {
                    uuid: uuid.to_string(),
                    uri: uri.to_string(),
                })
                .await?;
            Ok(())
        }
    }

    impl RpcHandle {
        /// Ping the Io Engine for responsiveness.
        pub async fn ping(&mut self) -> Result<(), tonic::Status> {
            match &mut self.io_engine {
                IoEngineClient::V0(cli) => cli.ping().await,
                IoEngineClient::V1(cli) => cli.ping().await,
            }
        }
        /// List all Nexuses.
        pub async fn list_nexuses(&mut self) -> Result<String, tonic::Status> {
            match &mut self.io_engine {
                IoEngineClient::V0(cli) => cli.list_nexuses().await,
                IoEngineClient::V1(cli) => cli.list_nexuses().await,
            }
        }
        /// Fault a Nexus Child.
        pub async fn fault_child(&mut self, uuid: &str, uri: &str) -> Result<(), tonic::Status> {
            match &mut self.io_engine {
                IoEngineClient::V0(cli) => cli.fault_child(uuid, uri).await,
                IoEngineClient::V1(cli) => cli.fault_child(uuid, uri).await,
            }
        }
        /// Add a Nexus Child.
        pub async fn add_child(
            &mut self,
            uuid: &str,
            uri: &str,
            norebuild: bool,
        ) -> Result<String, tonic::Status> {
            match &mut self.io_engine {
                IoEngineClient::V0(cli) => cli.add_child(uuid, uri, norebuild).await,
                IoEngineClient::V1(cli) => cli.add_child(uuid, uri, norebuild).await,
            }
        }
        /// Remove a Nexus Child.
        pub async fn remove_child(&mut self, uuid: &str, uri: &str) -> Result<(), tonic::Status> {
            match &mut self.io_engine {
                IoEngineClient::V0(cli) => cli.remove_child(uuid, uri).await,
                IoEngineClient::V1(cli) => cli.remove_child(uuid, uri).await,
            }
        }

        /// Connect to the container and return a handle to `Self`
        /// Note: The initial connection with a timeout is using blocking calls
        pub async fn connect(
            version: IoEngineApiVersion,
            name: &str,
            endpoint: SocketAddr,
        ) -> Result<Self, String> {
            let mut attempts = 40;
            loop {
                if TcpStream::connect_timeout(&endpoint, Duration::from_millis(100)).is_ok() {
                    break;
                } else {
                    std::thread::sleep(Duration::from_millis(100));
                }
                attempts -= 1;
                if attempts == 0 {
                    return Err(format!("Failed to connect to {}/{}", name, endpoint));
                }
            }

            let endpoint_str = format!("http://{}", endpoint);
            let channel = tonic::transport::Endpoint::new(endpoint_str)
                .map_err(|e| e.to_string())?
                .connect()
                .await
                .map_err(|e| e.to_string())?;

            let client = match version {
                IoEngineApiVersion::V0 => IoEngineClient::V0(IoEngineClientV0::new(channel)),
                IoEngineApiVersion::V1 => IoEngineClient::V1(IoEngineClientV1::new(channel)),
            };

            Ok(Self {
                name: name.to_string(),
                io_engine: client,
                endpoint,
            })
        }
    }
}

pub mod csi {
    #![allow(clippy::derive_partial_eq_without_eq)]
    include!(concat!(env!("OUT_DIR"), "/csi.v1.rs"));
}

pub mod v1 {
    /// The raw protobuf types.
    pub mod pb {
        #![allow(clippy::derive_partial_eq_without_eq)]
        include!(concat!(env!("OUT_DIR"), "/mayastor.v1.rs"));
    }

    /// V1 Registration autogenerated grpc code.
    pub mod registration {
        pub use super::pb::{
            registration_client, registration_server, ApiVersion, DeregisterRequest,
            RegisterRequest,
        };
    }

    /// V1 Host autogenerated grpc code.
    pub mod host {
        pub use super::pb::{
            block_device::{Filesystem, Partition},
            host_rpc_client, BlockDevice, ListBlockDevicesRequest,
        };
    }

    /// V1 Replica autogenerated grpc code.
    pub mod replica {
        pub use super::pb::{
            replica_rpc_client, CreateReplicaRequest, DestroyReplicaRequest, ListReplicaOptions,
            ListReplicasResponse, Replica, ShareReplicaRequest, UnshareReplicaRequest,
        };
    }

    /// V1 Nexus autogenerated grpc code.
    pub mod nexus {
        pub use super::pb::{
            nexus_rpc_client, AddChildNexusRequest, AddChildNexusResponse, Child, ChildState,
            ChildStateReason, CreateNexusRequest, CreateNexusResponse, DestroyNexusRequest,
            FaultNexusChildRequest, ListNexusOptions, ListNexusResponse, Nexus,
            NexusNvmePreemption, NexusState, NvmeAnaState, NvmeReservation, PublishNexusRequest,
            PublishNexusResponse, RemoveChildNexusRequest, RemoveChildNexusResponse,
            ShutdownNexusRequest, UnpublishNexusRequest, UnpublishNexusResponse,
        };
    }

    /// V1 Pool autogenerated grpc code.
    pub mod pool {
        pub use super::pb::{
            pool_rpc_client, CreatePoolRequest, DestroyPoolRequest, ListPoolOptions,
            ListPoolsResponse, Pool, PoolType,
        };
    }

    /// V1 JsonRpc autogenerated grpc code.
    pub mod json {
        pub use super::pb::{
            json_rpc_client, json_rpc_client::JsonRpcClient, JsonRpcRequest, JsonRpcResponse,
        };
    }
}

/// V1 Alpha api version.
pub mod v1_alpha {
    /// V1 alpha registration autogenerated grpc code.
    pub mod registration {
        include!(concat!(env!("OUT_DIR"), "/v1.registration.rs"));
    }
}
