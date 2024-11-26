"""Managing a Volume Nexus Target feature tests."""
import time

import os

from pytest_bdd import given, scenario, then, when
import pytest
from retrying import retry

from common.deployer import Deployer
from common.apiclient import ApiClient
from common.docker import Docker

from openapi.model.create_pool_body import CreatePoolBody
from openapi.model.create_volume_body import CreateVolumeBody
from openapi.model.protocol import Protocol
from openapi.model.volume_policy import VolumePolicy
from openapi.model.nexus_state import NexusState
from openapi.model.topology import Topology
from openapi.model.pool_topology import PoolTopology
from openapi.model.labelled_topology import LabelledTopology
from openapi.model.publish_volume_body import PublishVolumeBody


@scenario("feature.feature", "the target nexus is faulted")
def test_the_target_nexus_is_faulted():
    """the target nexus is faulted."""


@given("a control plane, two Io-Engine instances, two pools")
def a_control_plane_two_io_engine_instances_two_pools(init):
    """a control plane, two Io-Engine instances, two pools."""


@given("a published self-healing volume")
def a_published_selfhealing_volume():
    """a published self-healing volume."""
    request = CreateVolumeBody(
        VolumePolicy(True),
        NUM_VOLUME_REPLICAS,
        VOLUME_SIZE,
        False,
        topology=Topology(
            pool_topology=PoolTopology(
                labelled=LabelledTopology(
                    exclusion={},
                    inclusion={"node": IO_ENGINE_2},
                )
            )
        ),
    )
    ApiClient.volumes_api().put_volume(VOLUME_UUID, request)
    ApiClient.volumes_api().put_volume_target(
        VOLUME_UUID,
        publish_volume_body=PublishVolumeBody(
            {}, Protocol("nvmf"), node=IO_ENGINE_1, frontend_node=""
        ),
    )


@when("its nexus target is faulted")
def its_nexus_target_is_faulted():
    """its nexus target is faulted."""
    # Stop remote io-engine instance, which should fault the volume nexus child
    # By Stopping instead of killing, the control plane should see the node is down due to the
    # DeRegister event.
    Docker.stop_container(IO_ENGINE_2)
    check_target_faulted()


@when("one or more of its healthy replicas are back online")
def one_or_more_of_its_healthy_replicas_are_back_online():
    """one or more of its healthy replicas are back online."""
    # Brings back the io-engine instance, which should expose the replicas upon pool import
    Docker.restart_container(IO_ENGINE_2)
    check_replicas_online()


@then("it shall be recreated")
def it_shall_be_recreated():
    """it shall be recreated."""
    check_nexus_online()


@then("the nexus target shall be removed from its associated node")
def the_nexus_target_shall_be_removed_from_its_associated_node():
    """the nexus shall be removed from its associated node."""
    check_nexus_removed()


""" ... """

VOLUME_UUID = "5cd5378e-3f05-47f1-a830-a0f5873a1449"
VOLUME_SIZE = 10485761
NUM_VOLUME_REPLICAS = 1

IO_ENGINE_1 = "io-engine-1"
IO_ENGINE_2 = "io-engine-2"

POOL_UUID = "4cc6ee64-7232-497d-a26f-38284a444990"


@pytest.fixture(scope="function")
def create_pool_disk_images():
    pools = Deployer.create_disks(1, size=20 * 1024 * 1024)
    yield pools
    Deployer.cleanup_disks(len(pools))


@pytest.fixture(scope="function")
def init(create_pool_disk_images):
    # Shorten the reconcile periods and cache period to speed up the tests.
    Deployer.start(2, reconcile_period="500ms", cache_period="1s", jaeger=True)

    # Create pools
    ApiClient.pools_api().put_node_pool(
        IO_ENGINE_2,
        POOL_UUID,
        CreatePoolBody([create_pool_disk_images[0]], labels={"node": IO_ENGINE_2}),
    )

    yield
    Deployer.stop()


@retry(wait_fixed=200, stop_max_delay=5000)
def check_target_faulted():
    volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    if hasattr(volume.state, "target"):
        assert NexusState(volume.state.target["state"]) == NexusState("Faulted")


@retry(wait_fixed=200, stop_max_attempt_number=30)
def check_nexus_online():
    volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    nexus_uuid = volume.state.target["uuid"]
    nexus = ApiClient.nexuses_api().get_nexus(nexus_uuid)
    assert nexus.state == NexusState("Online")


@retry(wait_fixed=200, stop_max_delay=5000)
def check_nexus_removed():
    volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    assert (
        not hasattr(volume.state, "target")
        # or it might have been recreated if we lost the "race"...
        or NexusState(volume.state.target["state"]) == NexusState("Online")
        or NexusState(volume.state.target["state"]) == NexusState("Degraded")
    )


@retry(wait_fixed=200, stop_max_delay=5000)
def check_replicas_online():
    volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    online_replicas = list(
        filter(
            lambda uuid: str(volume.state.replica_topology[uuid].state) == "Online",
            list(volume.state.replica_topology),
        )
    )
    assert len(online_replicas) > 0
