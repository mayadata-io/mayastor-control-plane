"""Garbage collection of replicas feature tests."""

import requests
from pytest_bdd import (
    given,
    scenario,
    then,
)

from retrying import retry

import os
import pytest

from common.deployer import Deployer
from common.apiclient import ApiClient
from common.docker import Docker

from openapi.model.create_pool_body import CreatePoolBody
from openapi.model.create_volume_body import CreateVolumeBody
from openapi.model.protocol import Protocol
from openapi.model.volume_policy import VolumePolicy
from openapi.model.publish_volume_body import PublishVolumeBody

VOLUME_UUID = "5cd5378e-3f05-47f1-a830-a0f5873a1449"
VOLUME_SIZE = 10485761
NUM_VOLUME_REPLICAS = 2

IO_ENGINE_1 = "io-engine-1"
IO_ENGINE_2 = "io-engine-2"

POOL1_UUID = "4cc6ee64-7232-497d-a26f-38284a444980"
POOL2_UUID = "4cc6ee64-7232-497d-a26f-38284a444990"


@pytest.fixture(scope="function")
def create_pool_disk_images():
    pools = Deployer.create_disks(2, size=20 * 1024 * 1024)
    yield pools
    Deployer.cleanup_disks(len(pools))


# This fixture will be automatically used by all tests.
# It starts the deployer which launches all the necessary containers.
# A pool is created for convenience such that it is available for use by the tests.
@pytest.fixture(autouse=True)
def init(create_pool_disk_images):
    # Shorten the reconcile periods and cache period to speed up the tests.
    Deployer.start(2, reconcile_period="500ms", cache_period="1s")

    # Create pools
    ApiClient.pools_api().put_node_pool(
        IO_ENGINE_1,
        POOL1_UUID,
        CreatePoolBody(["aio://{}".format(create_pool_disk_images[0])]),
    )
    ApiClient.pools_api().put_node_pool(
        IO_ENGINE_2,
        POOL2_UUID,
        CreatePoolBody(["aio://{}".format(create_pool_disk_images[1])]),
    )

    # Create and publish a volume on node 1
    request = CreateVolumeBody(
        VolumePolicy(False), NUM_VOLUME_REPLICAS, VOLUME_SIZE, False
    )
    ApiClient.volumes_api().put_volume(VOLUME_UUID, request)
    ApiClient.volumes_api().put_volume_target(
        VOLUME_UUID,
        publish_volume_body=PublishVolumeBody(
            {}, Protocol("nvmf"), node=IO_ENGINE_1, frontend_node=""
        ),
    )

    yield
    Deployer.stop()


@scenario("feature.feature", "destroying an orphaned replica")
def test_destroying_an_orphaned_replica():
    """destroying an orphaned replica."""


@given("a replica which is managed but does not have any owners")
def a_replica_which_is_managed_but_does_not_have_any_owners():
    """a replica which is managed but does not have any owners."""

    # Kill the Io-Engine instance which does not host the nexus.
    Docker.kill_container(IO_ENGINE_2)

    # Attempt to delete the volume. This will leave a replica behind on the node that is
    # inaccessible.
    try:
        ApiClient.volumes_api().del_volume(VOLUME_UUID)
    except Exception as e:
        # An Io-Engine node is inaccessible, so deleting the volume will fail because the replica
        # on this node cannot be destroyed. Attempting to do so results in a timeout. This is
        # expected and results in a replica being orphaned.
        exception_info = e.__dict__
        assert exception_info["status"] == requests.codes["request_timeout"]
        pass

    check_orphaned_replica()


@then("the replica should eventually be destroyed")
def the_replica_should_eventually_be_destroyed():
    """the replica should eventually be destroyed."""

    # Restart the previously killed Io-Engine instance. This makes the previously inaccessible
    # node accessible, allowing the garbage collector to delete the replica.
    Docker.restart_container(IO_ENGINE_2)
    check_zero_replicas()


@retry(wait_fixed=1000, stop_max_attempt_number=10)
def check_zero_replicas():
    assert len(ApiClient.specs_api().get_specs()["replicas"]) == 0


@retry(wait_fixed=1000, stop_max_attempt_number=10)
def check_orphaned_replica():
    # There should only be one replica remaining - the one on the node that is inaccessible.
    replicas = ApiClient.specs_api().get_specs()["replicas"]
    assert len(replicas) == 1

    # Check that the replica is an orphan (i.e. it is managed but does not have any owners).
    replica = replicas[0]
    assert replica["managed"]
    assert len(replica["owners"]["nexuses"]) == 0
    assert "volume" not in replica["owners"]
