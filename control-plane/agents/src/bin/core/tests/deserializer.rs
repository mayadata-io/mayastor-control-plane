use std::collections::HashMap;
use stor_port::types::v0::{
    store::{
        nexus::{NexusSpec, NexusSpecStatus, ReplicaUri},
        nexus_child::NexusChild,
        nexus_persistence::{ChildInfo, NexusInfo},
        node::NodeSpec,
        pool::{PoolSpec, PoolSpecStatus},
        replica::{PoolRef, ReplicaSpec, ReplicaSpecStatus},
        volume::{TargetConfig, VolumeSpec, VolumeSpecStatus, VolumeTarget},
    },
    transport::{
        ChildUri, ExplicitNodeTopology, LabelledTopology, NexusId, NexusStatus, NodeTopology,
        PoolStatus, PoolTopology, Protocol, ReplicaId, ReplicaOwners, ReplicaStatus, Topology,
        VolumeId, VolumeLabels, VolumePolicy, VolumeShareProtocol, VolumeStatus,
    },
};

#[allow(clippy::large_enum_variant)]
enum Expected {
    VolumeSpec(VolumeSpec),
    PoolSpec(PoolSpec),
    NexusSpec(NexusSpec),
    ReplicaSpec(ReplicaSpec),
    NodeSpec(NodeSpec),
    NexusInfo(NexusInfo),
}

struct TestEntry {
    json_str: &'static str,
    expected: Expected,
}

fn validate_deserialization(entries: &[TestEntry]) {
    for entry in entries {
        let json_str = entry.json_str;
        match &entry.expected {
            Expected::VolumeSpec(value) => {
                let deserialized: VolumeSpec = serde_json::from_str(json_str).unwrap();
                assert_eq!(deserialized, *value);
            }
            Expected::PoolSpec(value) => {
                let deserialized: PoolSpec = serde_json::from_str(json_str).unwrap();
                assert_eq!(deserialized, *value);
            }
            Expected::NexusSpec(value) => {
                let deserialized: NexusSpec = serde_json::from_str(json_str).unwrap();
                assert_eq!(deserialized, *value);
            }
            Expected::ReplicaSpec(value) => {
                let deserialized: ReplicaSpec = serde_json::from_str(json_str).unwrap();
                assert_eq!(deserialized, *value);
            }
            Expected::NodeSpec(value) => {
                let deserialized: NodeSpec = serde_json::from_str(json_str).unwrap();
                assert_eq!(deserialized, *value);
            }
            Expected::NexusInfo(value) => {
                let deserialized: NexusInfo = serde_json::from_str(json_str).unwrap();
                assert_eq!(deserialized, *value);
            }
        };
    }
}

#[test]
fn test_deserialization_v1_to_v2() {
    let test_entries = vec![
        TestEntry {
            json_str: r#"{"uuid":"456122b1-7e19-4148-a890-579ca785a119","size":2147483648,"labels":{"local":"true"},"num_replicas":3,"status":{"Created":"Online"},"target":{"node":"mayastor-node4","nexus":"d6ccbb97-d13e-4ffb-91a0-c7607bb01f8f","protocol":"nvmf"},"policy":{"self_heal":true},"topology":{"node":{"Explicit":{"allowed_nodes":["mayastor-node2","mayastor-master","mayastor-node3","mayastor-node1","mayastor-node4"],"preferred_nodes":["mayastor-node2","mayastor-node3","mayastor-node4","mayastor-master","mayastor-node1"]}},"pool":{"Labelled":{"exclusion":{},"inclusion":{"org.com/created-by":"msp-operator"}}}},"last_nexus_id":"d6ccbb97-d13e-4ffb-91a0-c7607bb01f8f","operation":null}"#,
            expected: Expected::VolumeSpec(VolumeSpec {
                uuid: VolumeId::try_from("456122b1-7e19-4148-a890-579ca785a119").unwrap(),
                size: 2147483648,
                labels: {
                    let mut labels: VolumeLabels = HashMap::new();
                    labels.insert("local".to_string(), "true".to_string());
                    Some(labels)
                },
                num_replicas: 3,
                status: VolumeSpecStatus::Created(VolumeStatus::Online),
                policy: VolumePolicy { self_heal: true },
                topology: Some(Topology {
                    node: Some(NodeTopology::Explicit(ExplicitNodeTopology {
                        allowed_nodes: vec![
                            "mayastor-node2",
                            "mayastor-master",
                            "mayastor-node3",
                            "mayastor-node1",
                            "mayastor-node4",
                        ]
                            .into_iter()
                            .map(|id| id.into())
                            .collect(),
                        preferred_nodes: vec![
                            "mayastor-node2",
                            "mayastor-node3",
                            "mayastor-node4",
                            "mayastor-master",
                            "mayastor-node1",
                        ]
                            .into_iter()
                            .map(|id| id.into())
                            .collect(),
                    })),
                    pool: Some(PoolTopology::Labelled(LabelledTopology {
                        exclusion: HashMap::new(),
                        inclusion: {
                            let mut labels = HashMap::new();
                            labels.insert(
                                "org.com/created-by".to_string(),
                                "msp-operator".to_string(),
                            );
                            labels
                        },
                    })),
                }),
                sequencer: Default::default(),
                last_nexus_id: Some(
                    NexusId::try_from("d6ccbb97-d13e-4ffb-91a0-c7607bb01f8f").unwrap(),
                ),
                operation: None,
                thin: false,
                target_config: Some(TargetConfig::new(
                    VolumeTarget::new(
                        "mayastor-node4".into(),
                        NexusId::try_from("d6ccbb97-d13e-4ffb-91a0-c7607bb01f8f").unwrap(),
                        Some(VolumeShareProtocol::Nvmf),
                    ),
                    Default::default(),
                    Default::default()
                )),
                publish_context: None,
                affinity_group: None,
                .. Default::default()
            }),
        },
        TestEntry {
            json_str: r#"{"node":"mayastor-node1","id":"pool-1-on-mayastor-node1","disks":["/dev/sdb"],"status":{"Created":"Online"},"labels":{"org.com/created-by":"msp-operator"},"operation":null}"#,
            expected: Expected::PoolSpec(PoolSpec {
                node: "mayastor-node1".into(),
                id: "pool-1-on-mayastor-node1".into(),
                disks: vec!["/dev/sdb".into()],
                status: PoolSpecStatus::Created(PoolStatus::Online),
                labels: {
                    let mut labels = HashMap::new();
                    labels.insert(
                        "org.com/created-by".to_string(),
                        "msp-operator".to_string(),
                    );
                    Some(labels)
                },
                sequencer: Default::default(),
                operation: None,
                creat_tsc: None,
            }),
        },
        TestEntry {
            json_str: r#"{"uuid":"d6ccbb97-d13e-4ffb-91a0-c7607bb01f8f","name":"456122b1-7e19-4148-a890-579ca785a119","node":"mayastor-node4","children":[{"Replica":{"uuid":"82779efa-a0c7-4652-a37b-83eefd894714","share_uri":"bdev:///82779efa-a0c7-4652-a37b-83eefd894714?uuid=82779efa-a0c7-4652-a37b-83eefd894714"}},{"Replica":{"uuid":"2d98fa96-ac12-40be-acdc-e3559c0b1530","share_uri":"nvmf://136.144.51.237:8420/nqn.2019-05.com.org:2d98fa96-ac12-40be-acdc-e3559c0b1530?uuid=2d98fa96-ac12-40be-acdc-e3559c0b1530"}},{"Replica":{"uuid":"620ff519-419a-48d6-97a8-c1ba3260d87e","share_uri":"nvmf://136.144.51.239:8420/nqn.2019-05.com.org:620ff519-419a-48d6-97a8-c1ba3260d87e?uuid=620ff519-419a-48d6-97a8-c1ba3260d87e"}}],"size":2147483648,"spec_status":{"Created":"Online"},"share":"nvmf","managed":true,"owner":"456122b1-7e19-4148-a890-579ca785a119","operation":null}"#,
            expected: Expected::NexusSpec(NexusSpec {
                uuid: NexusId::try_from("d6ccbb97-d13e-4ffb-91a0-c7607bb01f8f").unwrap(),
                name: "456122b1-7e19-4148-a890-579ca785a119".to_string(),
                node: "mayastor-node4".into(),
                children: vec![
                    NexusChild::Replica(ReplicaUri::new(&ReplicaId::try_from("82779efa-a0c7-4652-a37b-83eefd894714").unwrap(), &ChildUri::try_from("bdev:///82779efa-a0c7-4652-a37b-83eefd894714?uuid=82779efa-a0c7-4652-a37b-83eefd894714").unwrap())),
                    NexusChild::Replica(ReplicaUri::new(&ReplicaId::try_from("2d98fa96-ac12-40be-acdc-e3559c0b1530").unwrap(), &ChildUri::try_from("nvmf://136.144.51.237:8420/nqn.2019-05.com.org:2d98fa96-ac12-40be-acdc-e3559c0b1530?uuid=2d98fa96-ac12-40be-acdc-e3559c0b1530").unwrap())),
                    NexusChild::Replica(ReplicaUri::new(&ReplicaId::try_from("620ff519-419a-48d6-97a8-c1ba3260d87e").unwrap(), &ChildUri::try_from("nvmf://136.144.51.239:8420/nqn.2019-05.com.org:620ff519-419a-48d6-97a8-c1ba3260d87e?uuid=620ff519-419a-48d6-97a8-c1ba3260d87e").unwrap()))
                ],
                size: 2147483648,
                spec_status: NexusSpecStatus::Created(NexusStatus::Online),
                share: Protocol::Nvmf,
                managed: true,
                owner: Some(VolumeId::try_from("456122b1-7e19-4148-a890-579ca785a119").unwrap()),
                ..Default::default()
            }),
        },
        TestEntry {
            json_str: r#"{"name":"2d98fa96-ac12-40be-acdc-e3559c0b1530","uuid":"2d98fa96-ac12-40be-acdc-e3559c0b1530","size":2147483648,"pool":"pool-2-on-mayastor-node2","share":"nvmf","thin":false,"status":{"Created":"online"},"managed":true,"owners":{"volume":"456122b1-7e19-4148-a890-579ca785a119"},"operation":null}"#,
            expected: Expected::ReplicaSpec(ReplicaSpec {
                name: "2d98fa96-ac12-40be-acdc-e3559c0b1530".into(),
                uuid: ReplicaId::try_from("2d98fa96-ac12-40be-acdc-e3559c0b1530").unwrap(),
                size: 2147483648,
                pool: PoolRef::Named("pool-2-on-mayastor-node2".into()),
                share: Protocol::Nvmf,
                thin: false,
                status: ReplicaSpecStatus::Created(ReplicaStatus::Online),
                managed: true,
                owners: ReplicaOwners::new(
                    Some(VolumeId::try_from("456122b1-7e19-4148-a890-579ca785a119").unwrap()),
                    vec![],
                ),
                ..Default::default()
            }),
        },
        TestEntry {
            json_str: r#"{"id":"mayastor-node1","endpoint":"136.144.51.107:10124","labels":{}}"#,
            expected: Expected::NodeSpec(NodeSpec::new(
                "mayastor-node1".into(),
                "136.144.51.107:10124".parse().unwrap(),
                Default::default(),
                None,
                None,
                None,
                None,
                None
            )),
        },
        TestEntry {
            json_str: r#"{"children":[{"healthy":true,"uuid":"82779efa-a0c7-4652-a37b-83eefd894714"},{"healthy":true,"uuid":"2d98fa96-ac12-40be-acdc-e3559c0b1530"},{"healthy":true,"uuid":"620ff519-419a-48d6-97a8-c1ba3260d87e"}],"clean_shutdown":false}"#,
            expected: Expected::NexusInfo(NexusInfo {
                clean_shutdown: false,
                children: vec![
                    ChildInfo {
                        uuid: ReplicaId::try_from("82779efa-a0c7-4652-a37b-83eefd894714").unwrap(),
                        healthy: true,
                    },
                    ChildInfo {
                        uuid: ReplicaId::try_from("2d98fa96-ac12-40be-acdc-e3559c0b1530").unwrap(),
                        healthy: true,
                    },
                    ChildInfo {
                        uuid: ReplicaId::try_from("620ff519-419a-48d6-97a8-c1ba3260d87e").unwrap(),
                        healthy: true,
                    },
                ],
                ..Default::default()
            }),
        }
    ];

    validate_deserialization(&test_entries);
}
