"""Anti-Affinity for Affinity Group feature tests."""

from typing import Dict
from concurrent.futures import ThreadPoolExecutor, as_completed

import pytest
from pytest_bdd import (
    given,
    scenario,
    then,
    when,
    parsers,
)

import common.operations
import openapi.exceptions
from common.apiclient import ApiClient
from common.deployer import Deployer
from openapi.model.create_pool_body import CreatePoolBody
from openapi.model.create_volume_body import CreateVolumeBody
from openapi.model.volume_policy import VolumePolicy

NODE_NAME_1 = "io-engine-1"
NODE_NAME_2 = "io-engine-2"
NODE_NAME_3 = "io-engine-3"
NON_AG_VOLUME_UUID = "5cd5378e-3f05-47f1-a830-a0f5873a1449"
AG_VOLUME_UUID_1 = "c36f7453-ae65-43bb-8ee9-4b5a373a8ed4"
AG_VOLUME_UUID_2 = "07805245-ec1c-43bd-a950-c42f4c3a9ac7"
AG_VOLUME_UUID_3 = "6805e636-1986-4ab7-95d0-75597df1532b"
VOLUME_SIZE = 10485761
AG_PARAM = {"id": "ag"}


@pytest.fixture(autouse=True, scope="module")
def init():
    """an io-engine cluster."""
    Deployer.start(
        io_engines=3,
    )
    yield
    Deployer.stop()


@scenario(
    "create.feature", "creating affinity group with 3 volumes with 1 replica each"
)
def test_creating_affinity_group_with_3_volumes_with_1_replica_each():
    """creating affinity group with 3 volumes with 1 replica each."""


@scenario(
    "create.feature",
    "creating affinity group with 3 volumes with 1 replica each with limited pools",
)
def test_creating_affinity_group_with_3_volumes_with_1_replica_each_with_limited_pools():
    """creating affinity group with 3 volumes with 1 replica each with limited pools."""


@scenario(
    "create.feature", "creating affinity group with 3 volumes with 3 replica on 3 pools"
)
def test_creating_affinity_group_with_3_volumes_with_3_replica_on_3_pools():
    """creating affinity group with 3 volumes with 3 replica on 3 pools."""


@scenario(
    "create.feature", "creating affinity group with 3 volumes with 3 replica on 5 pools"
)
def test_creating_affinity_group_with_3_volumes_with_3_replica_on_5_pools():
    """creating affinity group with 3 volumes with 3 replica on 5 pools."""


@scenario(
    "create.feature", "creating affinity group with 3 volumes with 3 replica on 9 pools"
)
def test_creating_affinity_group_with_3_volumes_with_3_replica_on_9_pools():
    """creating affinity group with 3 volumes with 3 replica on 9 pools."""


@given("2 pools, 1 on node A, 0 on node B, 1 on node C")
def create_node_pools_for_given_two_pools_configuration():
    """2 pools, 1 on node A, 0 on node B, 1 on node C."""
    node_pool_map = {NODE_NAME_1: 1, NODE_NAME_2: 0, NODE_NAME_3: 1}
    create_node_pools(node_pool_map)


@given("3 pools, 1 on node A, 1 on node B, 1 on node C")
def create_node_pools_for_given_three_pools_configuration():
    """3 pools, 1 on node A, 1 on node B, 1 on node C."""
    node_pool_map = {NODE_NAME_1: 1, NODE_NAME_2: 1, NODE_NAME_3: 1}
    create_node_pools(node_pool_map)


@given("5 pools, 2 on node A, 1 on node B, 2 on node C")
def create_node_pools_for_given_five_pools_configuration():
    """5 pools, 2 on node A, 1 on node B, 2 on node C."""
    node_pool_map = {NODE_NAME_1: 2, NODE_NAME_2: 1, NODE_NAME_3: 2}
    create_node_pools(node_pool_map)


@given("9 pools, 3 on node A, 3 on node B, 3 on node C")
def create_node_pools_for_given_nine_pools_configuration():
    """9 pools, 3 on node A, 3 on node B, 3 on node C."""
    node_pool_map = {NODE_NAME_1: 3, NODE_NAME_2: 3, NODE_NAME_3: 3}
    create_node_pools(node_pool_map)


@pytest.fixture(autouse=True)
@given("an io-engine cluster")
def setup_io_engine_cluster(init):
    """an io-engine cluster."""
    pytest.exception = ""
    yield
    common.operations.Cluster.cleanup()


@when(
    parsers.parse(
        "the create volume request with affinity group and {replicas:d} and {thin} executed"
    )
)
def execute_create_volume_request_with_affinity_group_and_replicas_and_thin(
    replicas, thin
):
    """the create volume request with affinity group and `replicas` and `thin` executed."""
    try:
        create_affinity_group(replicas, bool(thin))
    except openapi.exceptions.ServiceException as e:
        pytest.exception = e.reason


@then("1 volume creation should fail due to insufficient storage")
def check_for_volume_creation():
    """1 volume creation should fail due to insufficient storage."""
    assert pytest.exception == "Insufficient Storage"


@then("2 volumes should be created with 1 replica each")
def two_volumes_created_with_single_replica():
    """2 volumes should be created with 1 replica each."""
    filtered_volumes = get_affinity_group_volumes()
    assert len(filtered_volumes) == 2
    for volume in filtered_volumes:
        assert volume.spec.num_replicas == 1


@then("3 volumes should be created with 1 replica each")
def three_volumes_created_with_single_replica():
    """3 volumes should be created with 1 replica each."""
    filtered_volumes = get_affinity_group_volumes()
    assert len(filtered_volumes) == 3
    for volume in filtered_volumes:
        assert volume.spec.num_replicas == 1


@then("3 volumes should be created with 3 replica each")
def three_volumes_created_with_three_replicas():
    """3 volumes should be created with 3 replica each."""
    filtered_volumes = get_affinity_group_volumes()
    assert len(filtered_volumes) == 3
    for volume in filtered_volumes:
        assert volume.spec.num_replicas == 3


@then("each replica of individual volume should be on a different node")
def check_replicas_different_node_individual_volume():
    """each replica of individual volume should be on a different node."""
    filtered_volumes = get_affinity_group_volumes()
    for volume in filtered_volumes:
        replica_topology = volume.state.replica_topology
        node_locations = [replica.node for replica in replica_topology.values()]
        assert len(set(node_locations)) == len(replica_topology)


@then("each replica should reside on a different node")
def check_replicas_different_node():
    """each replica should reside on a different node."""
    filtered_volumes = get_affinity_group_volumes()
    already_encountered_nodes = set()
    for volume in filtered_volumes:
        replica_topology = volume.state.replica_topology
        for replica in replica_topology.values():
            node = replica.node
            if node in already_encountered_nodes:
                assert False
            already_encountered_nodes.add(node)


@then("each replica should reside on a different pool")
def check_replicas_different_pool():
    """each replica should reside on a different pool."""
    filtered_volumes = get_affinity_group_volumes()
    already_encountered_pools = set()
    for volume in filtered_volumes:
        replica_topology = volume.state.replica_topology
        for replica in replica_topology.values():
            pool = replica.pool
            if pool in already_encountered_pools:
                assert False
            already_encountered_pools.add(pool)


@then("pools can have more than one replicas all belonging to different volumes")
def check_pool_replicas_different_volumes():
    """pools can have more than one replicas all belonging to different volumes."""
    filtered_volumes = get_affinity_group_volumes()
    pool_replicas = {}
    for volume in filtered_volumes:
        replica_topology = volume.state.replica_topology
        for replica in replica_topology.values():
            pool = replica.pool
            if pool in pool_replicas:
                if volume.spec.uuid in pool_replicas[pool]:
                    assert False
                else:
                    pool_replicas[pool].append(volume.spec.uuid)
            else:
                pool_replicas[pool] = [volume.spec.uuid]


# HELPER METHODS


# Creates the pools based on given nodes and counts.
def create_node_pools(node_pool_map: Dict[str, int]):
    pool_index = 1
    for node_name, num_pools in node_pool_map.items():
        for i in range(num_pools):
            pool_name = f"pool_{pool_index}"
            disk_name = f"disk{i+1}"
            ApiClient.pools_api().put_node_pool(
                node_name,
                pool_name,
                CreatePoolBody([f"malloc:///{disk_name}?size_mb=200"]),
            )
            pool_index += 1
    ApiClient.volumes_api().put_volume(
        NON_AG_VOLUME_UUID, CreateVolumeBody(VolumePolicy(False), 2, VOLUME_SIZE, False)
    )


# Creates a 3 volume affinity_group based on number of replica of each volume
# and thin provisioning enabled or not. Launches 3 threads in parallel to execute the request.
def create_affinity_group(replicas, thin):
    def put_volume(volume_uuid):
        ApiClient.volumes_api().put_volume(
            volume_uuid,
            CreateVolumeBody(
                VolumePolicy(False),
                replicas=replicas,
                size=VOLUME_SIZE,
                thin=thin,
                affinity_group=AG_PARAM,
            ),
        )

    volume_uuids = [AG_VOLUME_UUID_1, AG_VOLUME_UUID_2, AG_VOLUME_UUID_3]

    with ThreadPoolExecutor(max_workers=len(volume_uuids)) as executor:
        futures = [
            executor.submit(put_volume, volume_uuid) for volume_uuid in volume_uuids
        ]
        for future in as_completed(futures):
            future.result()


# Get the affinity group volumes based on uuids
def get_affinity_group_volumes():
    volumes = ApiClient.volumes_api().get_volumes()
    return [
        volume
        for volume in volumes.entries
        if hasattr(volume.spec, "affinity_group")
        and volume.spec.affinity_group["id"] == AG_PARAM["id"]
    ]
