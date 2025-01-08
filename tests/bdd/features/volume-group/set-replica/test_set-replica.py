"""Anti-Affinity for Affinity Group feature tests."""

import json
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

import openapi.exceptions
from common.deployer import Deployer
from common.operations import Cluster
from openapi.model.create_volume_body import CreateVolumeBody
from openapi.model.volume_policy import VolumePolicy
from common.apiclient import ApiClient
from openapi.model.create_pool_body import CreatePoolBody

NODE_NAME_1 = "io-engine-1"
NODE_NAME_2 = "io-engine-2"
NODE_NAME_3 = "io-engine-3"
NON_VG_VOLUME_UUID = "5cd5378e-3f05-47f1-a830-a0f5873a1449"
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


@scenario("feature.feature", "scale up and subsequent creation")
def test_scale_up_and_subsequent_creation():
    """scale up and subsequent creation."""


@scenario("feature.feature", "affinity group volumes are scaled down")
def test_affinity_group_volumes_are_scaled_down():
    """affinity group volumes are scaled down."""


@scenario("feature.feature", "affinity group volumes are scaled down below 2 replicas")
def test_affinity_group_volumes_are_scaled_down_below_2_replicas():
    """affinity group volumes are scaled down below 2 replicas."""


@scenario("feature.feature", "affinity group volumes are scaled up")
def test_affinity_group_volumes_are_scaled_up():
    """affinity group volumes are scaled up."""


@given("2 pools, 1 on node A, 0 on node B, 1 on node C")
def pools_1_on_node_a_0_on_node_b_1_on_node_c():
    """2 pools, 1 on node A, 0 on node B, 1 on node C."""
    node_pool_map = {NODE_NAME_1: 1, NODE_NAME_2: 0, NODE_NAME_3: 1}
    create_node_pools(node_pool_map)


@given("3 pools, 1 on node A, 1 on node B, 1 on node C")
def pools_1_on_node_a_1_on_node_b_1_on_node_c():
    """3 pools, 1 on node A, 1 on node B, 1 on node C."""
    node_pool_map = {NODE_NAME_1: 1, NODE_NAME_2: 1, NODE_NAME_3: 1}
    create_node_pools(node_pool_map)


@given(
    parsers.parse(
        "3 volumes belonging to an affinity group with {replicas:d} each and {thin}"
    )
)
def volumes_belonging_to_a_affinity_group_with_replicas_each_and_thin(replicas, thin):
    """3 volumes belonging to an affinity group with <replicas> each and <thin>."""
    create_affinity_group(replicas, bool(thin))


@pytest.fixture(autouse=True)
@given("an io-engine cluster")
def an_ioengine_cluster(init):
    """an io-engine cluster."""
    pytest.vol_id = ""
    yield
    Cluster.cleanup()


@when("a new pool on node B is created")
def a_new_pool_on_node_b_is_created():
    """a new pool on node B is created."""
    ApiClient.pools_api().put_node_pool(
        NODE_NAME_2,
        "new_pool",
        CreatePoolBody(["malloc:///new_disk?size_mb=200"]),
    )


@when("the 3 volumes are scaled down to 1")
def the_3_volumes_are_scaled_down_to_1():
    """the 3 volumes are scaled down to 1."""
    try:
        scale_affinity_group(1)
    except openapi.exceptions.ApiException as e:
        pytest.error_body = json.loads(e.body)


@when("the 3 volumes are scaled down to 2")
def the_3_volumes_are_scaled_down_to_2():
    """the 3 volumes are scaled down to 2."""
    scale_affinity_group(2)


@when("the 3 volumes are scaled up by one")
def the_3_volumes_are_scaled_up_by_one():
    """the 3 volumes are scaled up by one."""
    scale_affinity_group(3)


@when(
    parsers.parse(
        "the volume belonging to an affinity group with one replica and {thin} is created"
    )
)
def the_volume_belonging_to_a_affinity_group_with_one_replica_and_thin_is_created(thin):
    """the volume belonging to an affinity group with one replica and <thin> is created."""
    try:
        if pytest.vol_id != "":
            ApiClient.volumes_api().put_volume(
                pytest.vol_id,
                CreateVolumeBody(
                    VolumePolicy(False),
                    replicas=1,
                    size=VOLUME_SIZE,
                    thin=bool(thin),
                    affinity_group=AG_PARAM,
                ),
            )
        else:
            ApiClient.volumes_api().put_volume(
                AG_VOLUME_UUID_1,
                CreateVolumeBody(
                    VolumePolicy(False),
                    replicas=1,
                    size=VOLUME_SIZE,
                    thin=bool(thin),
                    affinity_group=AG_PARAM,
                ),
            )
            pytest.vol_id = AG_VOLUME_UUID_2
        pytest.exception = False
    except openapi.exceptions.ServiceException:
        pytest.exception = True


@when("the volume is scaled up by one")
def the_volume_is_scaled_up_by_one():
    """the volume is scaled up by one."""
    ApiClient.volumes_api().put_volume_replica_count(AG_VOLUME_UUID_1, 2)


@then("3 new replicas should be created")
def new_replicas_should_be_created():
    """3 new replicas should be created."""
    ag_vols = get_affinity_group_volumes()
    for vol in ag_vols:
        assert vol.spec.num_replicas == 3


@then("3 replicas should be removed")
def replicas_should_be_removed():
    """3 replicas should be removed."""
    ag_vols = get_affinity_group_volumes()
    for vol in ag_vols:
        assert vol.spec.num_replicas == 2


@then("each node should have a maximum of 2 replicas from the affinityGroup")
def each_node_should_have_a_maximum_of_2_replicas_from_the_affinitygroup():
    """each node should have a maximum of 2 replicas from the affinityGroup."""
    filtered_volumes = get_affinity_group_volumes()
    encountered_node_counts = {}
    for volume in filtered_volumes:
        replica_topology = volume.state.replica_topology
        for replica in replica_topology.values():
            node = replica.node
            if node in encountered_node_counts:
                encountered_node_counts[node] += 1
                if encountered_node_counts[node] > 2:
                    assert False
            else:
                encountered_node_counts[node] = 1


@then("no volume should have less than 2 replicas")
def no_volume_should_have_less_than_2_replicas():
    """no volume should have less than 2 replicas."""
    ag_vols = get_affinity_group_volumes()
    for vol in ag_vols:
        assert vol.spec.num_replicas == 2


@then("the creation should fail")
def the_creation_should_fail():
    """the creation should fail."""
    assert pytest.exception == True


@then("the creation should not fail")
def the_creation_should_not_fail():
    """the creation should not fail."""
    assert pytest.exception == False


@then("the scale down operation should fail")
def the_scale_down_operation_should_fail():
    """the scale down operation should fail."""
    assert pytest.error_body["message"].startswith("SvcError :: RestrictedReplicaCount")
    assert pytest.error_body["kind"] == "FailedPrecondition"


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
        NON_VG_VOLUME_UUID, CreateVolumeBody(VolumePolicy(False), 2, VOLUME_SIZE, False)
    )


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


def scale_affinity_group(replicas):
    def put_volume_replica_count(volume_uuid):
        ApiClient.volumes_api().put_volume_replica_count(volume_uuid, replicas)

    volume_uuids = [AG_VOLUME_UUID_1, AG_VOLUME_UUID_2, AG_VOLUME_UUID_3]

    with ThreadPoolExecutor(max_workers=len(volume_uuids)) as executor:
        futures = [
            executor.submit(put_volume_replica_count, volume_uuid)
            for volume_uuid in volume_uuids
        ]
        for future in as_completed(futures):
            future.result()


def get_affinity_group_volumes():
    volumes = ApiClient.volumes_api().get_volumes()
    return [
        volume
        for volume in volumes.entries
        if hasattr(volume.spec, "affinity_group")
        and volume.spec.affinity_group["id"] == AG_PARAM["id"]
    ]
