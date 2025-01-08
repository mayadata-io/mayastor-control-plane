"""Volume Pool Topology feature tests."""

from pytest_bdd import given, scenario, then, when, parsers

import pytest
import docker
import requests
import time

from common import prod_domain_name, disk_pool_label_key, disk_pool_label_val
from common.deployer import Deployer
from common.apiclient import ApiClient
from common.docker import Docker
from common.nvme import nvme_connect, nvme_disconnect
from common.fio import Fio
from common.operations import Cluster

from openapi.model.create_pool_body import CreatePoolBody
from openapi.model.create_volume_body import CreateVolumeBody
from openapi.model.volume_policy import VolumePolicy
from openapi.model.topology import Topology
from openapi.model.pool_topology import PoolTopology
from openapi.model.labelled_topology import LabelledTopology
from openapi.model.spec_status import SpecStatus
from openapi.model.volume_spec import VolumeSpec

NUM_IO_ENGINES = 3
NODE_1_NAME = "io-engine-1"
NODE_2_NAME = "io-engine-2"
NODE_3_NAME = "io-engine-3"
NODE_1_POOL_1_UUID = "node1pool1"
NODE_1_POOL_2_UUID = "node1pool2"
NODE_1_POOL_3_UUID = "node1pool3"
NODE_2_POOL_1_UUID = "node2pool1"
NODE_2_POOL_2_UUID = "node2pool2"
NODE_2_POOL_3_UUID = "node2pool3"
NODE_3_POOL_1_UUID = "node3pool1"
NODE_3_POOL_2_UUID = "node3pool2"
NODE_3_POOL_3_UUID = "node3pool3"
CREATE_REQUEST_KEY = "create_request"
VOLUME_SIZE = 10485761
VOLUME_UUID = "5cd5378e-3f05-47f1-a830-a0f5873a1441"
DISKPOOL_LABEL_KEY = disk_pool_label_key
DISKPOOL_LABEL_VAL = disk_pool_label_val


# The labels to be applied to the pools.
###############################################################################################
#   Description            ||   Pool Name   ||         Label           ||        Node        ||
# ==============================================================================================
#     "pool1" has          ||   node1pool1  ||     zone-us=us-west-1   ||     io-engine-1    ||
#       the label          ||   node2pool1  ||     zone-us=us-west-1   ||     io-engine-2    ||
#   "zone-us=us-west-1"    ||   node3pool1  ||     zone-us=us-west-1   ||     io-engine-3    ||
# ==============================================================================================
#     "pool2" has          ||   node1pool2  ||     zone-ap=ap-south-1  ||     io-engine-1    ||
#      the label           ||   node2pool2  ||     zone-ap=ap-south-1  ||     io-engine-2    ||
#   "zone-ap=ap-south-1"   ||   node3pool2  ||     zone-ap=ap-south-1  ||     io-engine-3    ||
# ==============================================================================================
#     "pool3" has          ||   node1pool3  ||     zone-eu=eu-west-3   ||     io-engine-1    ||
#      the label           ||   node2pool3  ||     zone-eu=eu-west-3   ||     io-engine-2    ||
#   "zone-eu=eu-west-3"    ||   node3pool3  ||     zone-eu=eu-west-3   ||     io-engine-3    ||
# ==============================================================================================
# =========================================================================================

POOL_CONFIGURATIONS = [
    # Pool node1pool1 has the label "zone-us=us-west-1" and is on node "io-engine-1"
    {
        "node_name": NODE_1_NAME,
        "pool_uuid": NODE_1_POOL_1_UUID,
        "pool_body": CreatePoolBody(
            ["malloc:///disk1?size_mb=32"],
            labels={
                DISKPOOL_LABEL_KEY: DISKPOOL_LABEL_VAL,
                "node": "io-engine-1",
                "zone-us": "us-west-1",
            },
        ),
    },
    # Pool node2pool1 has the label "zone-us=us-west-1" and is on node "io-engine-2"
    {
        "node_name": NODE_2_NAME,
        "pool_uuid": NODE_2_POOL_1_UUID,
        "pool_body": CreatePoolBody(
            ["malloc:///disk1?size_mb=32"],
            labels={
                DISKPOOL_LABEL_KEY: DISKPOOL_LABEL_VAL,
                "node": "io-engine-2",
                "zone-us": "us-west-1",
            },
        ),
    },
    # Pool node3pool1 has the label "zone-us=us-west-1" and is on node "io-engine-3"
    {
        "node_name": NODE_3_NAME,
        "pool_uuid": NODE_3_POOL_1_UUID,
        "pool_body": CreatePoolBody(
            ["malloc:///disk1?size_mb=32"],
            labels={
                DISKPOOL_LABEL_KEY: DISKPOOL_LABEL_VAL,
                "node": "io-engine-3",
                "zone-us": "us-west-1",
            },
        ),
    },
    # Pool node1pool2 has the label "zone-ap=ap-south-1" and is on node "io-engine-1"
    {
        "node_name": NODE_1_NAME,
        "pool_uuid": NODE_1_POOL_2_UUID,
        "pool_body": CreatePoolBody(
            ["malloc:///disk2?size_mb=32"],
            labels={
                DISKPOOL_LABEL_KEY: DISKPOOL_LABEL_VAL,
                "node": "io-engine-1",
                "zone-ap": "ap-south-1",
            },
        ),
    },
    # Pool node2pool2 has the label "zone-ap=ap-south-1" and is on node "io-engine-2"
    {
        "node_name": NODE_2_NAME,
        "pool_uuid": NODE_2_POOL_2_UUID,
        "pool_body": CreatePoolBody(
            ["malloc:///disk2?size_mb=32"],
            labels={
                DISKPOOL_LABEL_KEY: DISKPOOL_LABEL_VAL,
                "node": "io-engine-2",
                "zone-ap": "ap-south-1",
            },
        ),
    },
    # Pool node3pool2 has the label "zone-ap=ap-south-1" and is on node "io-engine-3"
    {
        "node_name": NODE_3_NAME,
        "pool_uuid": NODE_3_POOL_2_UUID,
        "pool_body": CreatePoolBody(
            ["malloc:///disk2?size_mb=32"],
            labels={
                DISKPOOL_LABEL_KEY: DISKPOOL_LABEL_VAL,
                "node": "io-engine-3",
                "zone-ap": "ap-south-1",
            },
        ),
    },
    # Pool node1pool3 has the label "zone-eu=eu-west-3" and is on node "io-engine-1"
    {
        "node_name": NODE_1_NAME,
        "pool_uuid": NODE_1_POOL_3_UUID,
        "pool_body": CreatePoolBody(
            ["malloc:///disk3?size_mb=32"],
            labels={
                DISKPOOL_LABEL_KEY: DISKPOOL_LABEL_VAL,
                "node": "io-engine-1",
                "zone-eu": "eu-west-3",
            },
        ),
    },
    # Pool node2pool3 has the label "zone-eu=eu-west-3" and is on node "io-engine-2"
    {
        "node_name": NODE_2_NAME,
        "pool_uuid": NODE_2_POOL_3_UUID,
        "pool_body": CreatePoolBody(
            ["malloc:///disk3?size_mb=32"],
            labels={
                DISKPOOL_LABEL_KEY: DISKPOOL_LABEL_VAL,
                "node": "io-engine-2",
                "zone-eu": "eu-west-3",
            },
        ),
    },
    # Pool node3pool3 has the label "zone-eu=eu-west-3" and is on node "io-engine-3"
    {
        "node_name": NODE_3_NAME,
        "pool_uuid": NODE_3_POOL_3_UUID,
        "pool_body": CreatePoolBody(
            ["malloc:///disk3?size_mb=32"],
            labels={
                DISKPOOL_LABEL_KEY: DISKPOOL_LABEL_VAL,
                "node": "io-engine-3",
                "zone-eu": "eu-west-3",
            },
        ),
    },
]


@pytest.fixture(scope="module")
def init():
    Deployer.start(NUM_IO_ENGINES, io_engine_coreisol=True)

    try:
        # Create the pools.
        for config in POOL_CONFIGURATIONS:
            ApiClient.pools_api().put_node_pool(
                config["node_name"],
                config["pool_uuid"],
                config["pool_body"],
            )

        yield
    finally:
        Deployer.stop()


# Fixture used to pass the volume create request between test steps.
@pytest.fixture(scope="function")
def create_request():
    return {}


# Fixture used to pass the replica context between test steps.
@pytest.fixture(scope="function")
def replica_ctx():
    return {}


@scenario(
    "pool-topology.feature", "Suitable pools which contain volume topology labels"
)
def test_suitable_pools_which_contain_volume_topology_labels():
    """Suitable pools which contain volume topology labels."""


@scenario(
    "pool-topology.feature", "Suitable pools which contain volume topology keys only"
)
def test_suitable_pools_which_contain_volume_topology_keys_only():
    """Suitable pools which contain volume topology keys only."""


@given("a control plane, three Io-Engine instances, nine pools")
def a_control_plane_three_ioengine_instances_nine_pools(init):
    """a control plane, three Io-Engine instances, nine pools."""
    docker_client = docker.from_env()

    # The control plane comprises the core agents, rest server and etcd instance.
    for component in ["core", "rest", "etcd"]:
        Docker.check_container_running(component)

    # Check all Io-Engine instances are running
    try:
        io_engines = docker_client.containers.list(
            all=True, filters={"name": "io-engine"}
        )

    except docker.errors.NotFound:
        raise Exception("No Io-Engine instances")

    for io_engine in io_engines:
        Docker.check_container_running(io_engine.attrs["Name"])

    # Check for a pools
    pools = ApiClient.pools_api().get_pools()
    assert len(pools) == 9
    yield
    Cluster.cleanup(pools=False)


@given(
    parsers.parse(
        "a request for a {replica} replica volume with poolAffinityTopologyLabel as {pool_affinity_topology_label} and pool topology inclusion as {volume_pool_topology_inclusion_label}"
    )
)
def a_request_for_a_replica_replica_volume_with_poolaffinitytopologylabel_as_pool_affinity_topology_label_and_pool_topology_inclusion_as_volume_pool_topology_inclusion_label(
    create_request,
    replica,
    pool_affinity_topology_label,
    volume_pool_topology_inclusion_label,
):
    """a request for a <replica> replica volume with poolAffinityTopologyLabel as <pool_affinity_topology_label> and pool topology inclusion as <volume_pool_topology_inclusion_label>."""
    if pool_affinity_topology_label == "True":
        request = create_volume_body(replica, volume_pool_topology_inclusion_label)
        create_request[CREATE_REQUEST_KEY] = request


@given(
    parsers.parse(
        "a request for a {replica} replica volume with poolHasTopologyKey as {has_topology_key} and pool topology inclusion as {volume_pool_topology_inclusion_label}"
    )
)
def a_request_for_a_replica_replica_volume_with_poolhastopologykey_as_has_topology_key_and_pool_topology_inclusion_as_volume_pool_topology_inclusion_label(
    create_request, replica, has_topology_key, volume_pool_topology_inclusion_label
):
    """a request for a <replica> replica volume with poolHasTopologyKey as <has_topology_key> and pool topology inclusion as <volume_pool_topology_inclusion_label>."""
    if has_topology_key == "True":
        request = create_volume_body(replica, volume_pool_topology_inclusion_label)
        create_request[CREATE_REQUEST_KEY] = request


@when(
    parsers.parse(
        "the desired number of replica of volume i.e. {replica} here; is {expression} number of the pools containing the label {volume_pool_topology_inclusion_label}"
    )
)
def the_desired_number_of_replica_of_volume_ie_replica_here_is_expression_number_of_the_pools_containing_the_label_volume_pool_topology_inclusion_label(
    create_request, replica, expression, volume_pool_topology_inclusion_label
):
    """the desired number of replica of volume i.e. <replica> here; is <expression> number of the pools containing the label <volume_pool_topology_inclusion_label>."""
    no_of_eligible_pools = no_of_suitable_pools(
        create_request[CREATE_REQUEST_KEY]["topology"]["pool_topology"]["labelled"][
            "inclusion"
        ],
    )
    if expression == "<=":
        assert int(replica) <= no_of_eligible_pools
    elif expression == ">":
        assert int(replica) > no_of_eligible_pools


@then(
    parsers.parse(
        "the {replica} replica volume creation should {result} and {provisioned} provisioned on pools with labels {pool_label}"
    )
)
def the_replica_replica_volume_creation_should_result_and_provisioned_provisioned_on_pools_with_labels_pool_label(
    create_request, replica, result, provisioned, pool_label
):
    """the <replica> replica volume creation should <result> and <provisioned> provisioned on pools with labels <pool_label>."""
    if result == "succeed":
        # Check the volume object returned is as expected
        request = create_request[CREATE_REQUEST_KEY]
        volume = ApiClient.volumes_api().put_volume(VOLUME_UUID, request)
        expected_spec = expected_volume_spec(
            replica, create_request[CREATE_REQUEST_KEY]["topology"]
        )
        assert str(volume.spec) == str(expected_spec)
        pools_names_which_has_given_labels = get_pool_names_with_given_labels(
            pool_label
        )
        pools_on_which_volume_provisioned = list()
        for replica_id, replica_details in volume.state["replica_topology"].items():
            pools_on_which_volume_provisioned.append(replica_details["pool"])
        assert set(pools_on_which_volume_provisioned).issubset(
            set(pools_names_which_has_given_labels)
        )
    elif result == "fail":
        """volume creation should fail with an insufficient storage error."""
        request = create_request[CREATE_REQUEST_KEY]
        try:
            ApiClient.volumes_api().put_volume(VOLUME_UUID, request)
        except Exception as e:
            exception_info = e.__dict__
            assert exception_info["status"] == requests.codes["insufficient_storage"]

        # Check that the volume wasn't created.
        volumes = ApiClient.volumes_api().get_volumes().entries
        assert len(volumes) == 0


def get_pool_names_with_given_labels(pool_label):
    pool_with_labels = get_pool_names_with_its_corresponding_labels()
    qualified_pools = list()
    if "=" in pool_label:
        # Splitting the pool_label into parts
        split_parts = pool_label.split("=")
        # Accessing the parts after splitting
        pool_key = split_parts[0]
        pool_value = split_parts[1]
        for pool_id, pool_labels in pool_with_labels.items():
            for key, value in pool_labels.items():
                if key == pool_key and value == pool_value:
                    qualified_pools.append(pool_id)
    else:
        pool_key = pool_label
        for pool_id, pool_labels in pool_with_labels.items():
            for key, value in pool_labels.items():
                if key == pool_key:
                    qualified_pools.append(pool_id)
    return qualified_pools


# Return the create volume request body based on the input parameters.
def create_volume_body(replica, volume_pool_topology_inclusion_label):
    """Create a volume body."""
    key, _, value = volume_pool_topology_inclusion_label.partition("=")
    topology = Topology(
        pool_topology=PoolTopology(
            labelled=LabelledTopology(
                exclusion={},
                inclusion={
                    key.strip(): value.strip(),
                    DISKPOOL_LABEL_KEY: DISKPOOL_LABEL_VAL,
                },
            )
        )
    )
    return CreateVolumeBody(
        VolumePolicy(False),
        int(replica),
        VOLUME_SIZE,
        False,
        topology=topology,
    )


# Return the expected volume spec based on the input parameters.
def expected_volume_spec(replica, toplogy):
    """Return the expected volume spec."""
    return VolumeSpec(
        int(replica),
        VOLUME_SIZE,
        SpecStatus("Created"),
        VOLUME_UUID,
        VolumePolicy(False),
        False,
        0,
        topology=toplogy,
    )


# Return the number of pools that qualify based on the volume topology inclusion labels.
def no_of_suitable_pools(volume_pool_topology_inclusion_labels):
    """Return the number of pools that qualify based on the volume topology inclusion labels."""
    pool_with_labels = get_pool_names_with_its_corresponding_labels()
    qualified_pools = list()
    for pool_id, pool_labels in pool_with_labels.items():
        if does_pool_qualify_inclusion_labels(
            volume_pool_topology_inclusion_labels, pool_labels
        ):
            qualified_pools.append(pool_id)
    return len(qualified_pools)


# Return the pool names with its corresponding labels.
def get_pool_names_with_its_corresponding_labels():
    """Return the pool names with the given labels."""
    pool_with_its_corresponding_labels = {}
    pools = ApiClient.pools_api().get_pools()
    for pool in pools:
        pool_with_its_corresponding_labels[pool["id"]] = pool["spec"]["labels"]
    return pool_with_its_corresponding_labels


# Return whether the pool qualifies based on the volume topology inclusion labels.
def does_pool_qualify_inclusion_labels(
    volume_pool_topology_inclusion_labels, pool_labels
):
    inc_match = True
    for key, value in volume_pool_topology_inclusion_labels.items():
        if key in pool_labels:
            if value == "":
                inc_match = True
                break
            if volume_pool_topology_inclusion_labels[key] != pool_labels[key]:
                inc_match = False
                break
        else:
            inc_match = False
    return inc_match
