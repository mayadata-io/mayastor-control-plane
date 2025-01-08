"""Anti-Affinity for Affinity Group feature tests."""

from typing import Dict
from concurrent.futures import ThreadPoolExecutor, as_completed

import pytest
from pytest_bdd import (
    given,
    scenario,
    then,
    when,
)

import openapi.exceptions
from common.apiclient import ApiClient
from common.deployer import Deployer
from common.operations import Cluster
from openapi.model.create_pool_body import CreatePoolBody
from openapi.model.create_volume_body import CreateVolumeBody
from openapi.model.volume_share_protocol import VolumeShareProtocol
from openapi.model.publish_volume_body import PublishVolumeBody
from openapi.model.volume_policy import VolumePolicy

NODE_NAME_1 = "io-engine-1"
NODE_NAME_2 = "io-engine-2"
NON_AG_VOLUME_UUID = "5cd5378e-3f05-47f1-a830-a0f5873a1449"
AG_VOLUME_UUID_1 = "c36f7453-ae65-43bb-8ee9-4b5a373a8ed4"
AG_VOLUME_UUID_2 = "07805245-ec1c-43bd-a950-c42f4c3a9ac7"
VOLUME_SIZE = 10485761
AG_PARAM = {"id": "ag"}


@pytest.fixture(autouse=True, scope="module")
def init():
    """a deployer cluster."""
    Deployer.start(
        io_engines=2,
    )
    yield
    Deployer.stop()


@scenario("feature.feature", "affinity group target distribution")
def test_affinity_group_target_distribution():
    """affinity group target distribution."""


@scenario("feature.feature", "affinity group volume undergoes republish")
def test_affinity_group_volume_undergoes_republish():
    """affinity group volume undergoes republish."""


@pytest.fixture(autouse=True)
@given("a deployer cluster")
def a_deployer_cluster(init):
    """a deployer cluster."""
    pytest.exception = ""
    yield
    Cluster.cleanup()


@given("one non affinity group volume with 2 replica published on node B")
def one_non_affinity_group_volume_with_2_replica_published_on_node_b():
    """one non affinity group volume with 2 replica published on node B."""
    ApiClient.volumes_api().put_volume(
        NON_AG_VOLUME_UUID,
        CreateVolumeBody(VolumePolicy(False), replicas=2, size=VOLUME_SIZE, thin=True),
    )
    ApiClient.volumes_api().put_volume_target(
        NON_AG_VOLUME_UUID,
        publish_volume_body=PublishVolumeBody(
            {},
            VolumeShareProtocol("nvmf"),
            node=NODE_NAME_2,
            frontend_node="app-node-1",
        ),
    )


@given("one unpublished affinity group volume with 2 replica")
def one_unpublished_affinity_group_volume_with_2_replica():
    """one unpublished affinity group volume with 2 replica."""
    ApiClient.volumes_api().put_volume(
        AG_VOLUME_UUID_2,
        CreateVolumeBody(
            VolumePolicy(False),
            replicas=2,
            size=VOLUME_SIZE,
            thin=True,
            affinity_group=AG_PARAM,
        ),
    )


@given("one affinity group volume with 2 replica published on node A")
def one_affinity_group_volume_with_2_replica_published_on_node_a():
    """one affinity group volume with 2 replica published on node A."""
    ApiClient.volumes_api().put_volume(
        AG_VOLUME_UUID_1,
        CreateVolumeBody(
            VolumePolicy(False),
            replicas=2,
            size=VOLUME_SIZE,
            thin=True,
            affinity_group=AG_PARAM,
        ),
    )
    ApiClient.volumes_api().put_volume_target(
        AG_VOLUME_UUID_1,
        publish_volume_body=PublishVolumeBody(
            {},
            VolumeShareProtocol("nvmf"),
            node=NODE_NAME_1,
            frontend_node="app-node-1",
        ),
    )


@given("the volume targets are on different nodes")
def the_volume_targets_are_on_different_nodes():
    """the volume targets are on different nodes."""
    volumes = get_affinity_group_volumes()
    assert volumes[0].state.target["node"] != volumes[1].state.target["node"]


@given("two pools, one on node A and another on node B")
def two_pools_one_on_node_a_and_another_on_node_b():
    """two pools, one on node A and another on node B."""
    node_pool_map = {NODE_NAME_1: 1, NODE_NAME_2: 1}
    create_node_pools(node_pool_map)


@given("two published volumes with two replicas each")
def two_published_volumes_with_two_replicas_each():
    """two published volumes with two replicas each."""
    create_affinity_group()
    publish_affinity_group()


@when("one volume is republished")
def one_volume_is_republished():
    """one volume is republished."""
    try:
        ApiClient.volumes_api().put_volume_target(
            AG_VOLUME_UUID_2,
            publish_volume_body=PublishVolumeBody(
                {},
                VolumeShareProtocol("nvmf"),
                republish=True,
                reuse_existing=False,
                frontend_node="app-node-1",
            ),
        )
    except openapi.exceptions.ServiceException as e:
        pytest.exception = e.reason


@when("the non published volume is published")
def the_non_published_volume_is_published():
    """the non published volume is published."""
    try:
        ApiClient.volumes_api().put_volume_target(
            AG_VOLUME_UUID_2,
            publish_volume_body=PublishVolumeBody(
                {}, VolumeShareProtocol("nvmf"), frontend_node="app-node-1"
            ),
        )
    except openapi.exceptions.ServiceException as e:
        pytest.exception = e.reason


@then("both the targets should be on the same node")
def both_the_targets_should_be_on_the_same_node():
    """both the targets should be on the same node."""
    volumes = get_affinity_group_volumes()
    assert volumes[0].state.target["node"] == volumes[1].state.target["node"]


@then("the publish should not fail")
def the_publish_should_not_fail():
    """the publish should not fail."""
    assert pytest.exception == ""


@then("the republish should not fail")
def the_republish_should_not_fail():
    """the republish should not fail."""
    assert pytest.exception == ""


@then("the target should land on node B")
def the_target_should_land_on_node_b():
    """the target should land on node B."""
    volume = ApiClient.volumes_api().get_volume(AG_VOLUME_UUID_2)
    assert volume.state.target["node"] == NODE_NAME_2


# HELPER METHODS


# Creates the pools based on given nodes and counts.
def create_node_pools(node_pool_map: Dict[str, int]):
    pool_index = 1
    for node_name, num_pools in node_pool_map.items():
        for i in range(num_pools):
            pool_name = f"pool_{pool_index}"
            disk_name = f"disk{i + 1}"
            ApiClient.pools_api().put_node_pool(
                node_name,
                pool_name,
                CreatePoolBody([f"malloc:///{disk_name}?size_mb=200"]),
            )
            pool_index += 1


# Create affinity group volumes parallely.
def create_affinity_group():
    def put_volume(volume_uuid):
        ApiClient.volumes_api().put_volume(
            volume_uuid,
            CreateVolumeBody(
                VolumePolicy(False),
                replicas=2,
                size=VOLUME_SIZE,
                thin=True,
                affinity_group=AG_PARAM,
            ),
        )

    volume_uuids = [AG_VOLUME_UUID_1, AG_VOLUME_UUID_2]

    with ThreadPoolExecutor(max_workers=len(volume_uuids)) as executor:
        futures = [
            executor.submit(put_volume, volume_uuid) for volume_uuid in volume_uuids
        ]
        for future in as_completed(futures):
            future.result()


# Publishes all the affinity group volumes parallely.
def publish_affinity_group():
    def put_volume_target(volume_uuid):
        ApiClient.volumes_api().put_volume_target(
            volume_uuid,
            publish_volume_body=PublishVolumeBody(
                {}, VolumeShareProtocol("nvmf"), frontend_node="app-node-1"
            ),
        )

    volume_uuids = [AG_VOLUME_UUID_1, AG_VOLUME_UUID_2]

    with ThreadPoolExecutor(max_workers=len(volume_uuids)) as executor:
        futures = [
            executor.submit(put_volume_target, volume_uuid)
            for volume_uuid in volume_uuids
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
