"""Adjusting the volume replicas feature tests."""
import os
import time

from pytest_bdd import (
    given,
    scenario,
    then,
    when,
)

import pytest
from retrying import retry

from common.deployer import Deployer
from common.apiclient import ApiClient
from common.docker import Docker
from common.operations import Cluster

from openapi.model.create_pool_body import CreatePoolBody
from openapi.model.create_volume_body import CreateVolumeBody
from openapi.model.protocol import Protocol
from openapi.model.volume_policy import VolumePolicy
from openapi.model.publish_volume_body import PublishVolumeBody

POOL_1_UUID = "4cc6ee64-7232-497d-a26f-38284a444980"
POOL_2_UUID = "91a60318-bcfe-4e36-92cb-ddc7abf212ea"
POOL_3_UUID = "0b6cd331-60d9-48ae-ac00-dbe0430d6c1f"
VOLUME_UUID = "5cd5378e-3f05-47f1-a830-a0f5873a1449"
NODE_1_NAME = "io-engine-1"
NODE_2_NAME = "io-engine-2"
NODE_3_NAME = "io-engine-3"
VOLUME_SIZE = 10485761
NUM_IO_ENGINES = 3
NUM_VOLUME_REPLICAS = 2
REPLICA_CONTEXT_KEY = "replica"


@pytest.fixture(scope="module")
def background():
    Deployer.start(
        NUM_IO_ENGINES,
        faulted_child_wait_period="100ms",
        reconcile_period="150ms",
        cache_period="100ms",
        io_engine_env="MAYASTOR_HB_INTERVAL_SEC=1",
    )
    yield
    Deployer.stop()


@pytest.fixture
def disks():
    pools = Deployer.create_disks(NUM_IO_ENGINES, size=100 * 1024 * 1024)
    yield pools
    Deployer.cleanup_disks(len(pools))


@pytest.fixture
def init(background, disks):
    init_resources(disks)
    yield
    Cluster.cleanup()


# Create pools and a volume for use in the test cases.
def init_resources(disks):
    ApiClient.pools_api().put_node_pool(
        NODE_1_NAME, POOL_1_UUID, CreatePoolBody([disks[0]])
    )
    ApiClient.pools_api().put_node_pool(
        NODE_2_NAME, POOL_2_UUID, CreatePoolBody([disks[1]])
    )
    ApiClient.volumes_api().put_volume(
        VOLUME_UUID,
        CreateVolumeBody(VolumePolicy(True), NUM_VOLUME_REPLICAS, VOLUME_SIZE, False),
    )
    ApiClient.pools_api().put_node_pool(
        NODE_3_NAME, POOL_3_UUID, CreatePoolBody([disks[2]])
    )
    # Publish volume so that there is a nexus to add a replica to.
    volume = ApiClient.volumes_api().put_volume_target(
        VOLUME_UUID,
        publish_volume_body=PublishVolumeBody({}, Protocol("nvmf"), node=NODE_1_NAME),
    )
    assert hasattr(volume.spec, "target")
    assert str(volume.spec.target.protocol) == str(Protocol("nvmf"))


# Fixture used to pass the replica context between test steps.
@pytest.fixture(scope="function")
def replica_ctx():
    return {}


@scenario(
    "feature.feature",
    "decreasing the replica count when the runtime replica count matches the desired count",
)
def test_decreasing_the_replica_count_when_the_runtime_replica_count_matches_the_desired_count(
    init,
):
    """removing a replica when the runtime replica count matches the desired count."""


@scenario("feature.feature", "setting volume replicas to zero")
def test_setting_volume_replicas_to_zero(init):
    """setting volume replicas to zero."""


@scenario("feature.feature", "successfully adding a replica")
def test_successfully_adding_a_replica(init):
    """successfully adding a replica."""


@scenario("feature.feature", "successfully removing a replica")
def test_successfully_removing_a_replica(init):
    """successfully removing a replica."""


@given("a suitable available pool")
def a_suitable_available_pool():
    """a suitable available pool."""
    pools = ApiClient.pools_api().get_pools()
    assert len(pools) == 3


@given("a volume with 2 replicas")
def a_volume_with_2_replicas():
    """a volume with 2 replicas."""
    assert num_runtime_volume_replicas() == 2


@given("an existing volume")
def an_existing_volume():
    """an existing volume."""
    volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    assert volume.spec.uuid == VOLUME_UUID


@given("no available pools for replacement replicas")
def no_available_pools_for_replacement_replicas():
    """no available pools for replacement replicas."""
    pool_api = ApiClient.pools_api()
    pools = pool_api.get_pools()
    assert len(pools) == 3

    # Delete the additional pool so that a replacement replica cannot be created.
    pool_api.del_pool(POOL_3_UUID)
    pools = pool_api.get_pools()
    assert len(pools) == 2


@given("the number of volume replicas is greater than one")
def the_number_of_volume_replicas_is_greater_than_one():
    """the number of volume replicas is greater than one."""
    volume = set_num_volume_replicas(NUM_VOLUME_REPLICAS + 1)
    assert volume.spec.num_replicas > 1


@when("a user attempts to decrease the number of volume replicas")
def a_user_attempts_to_decrease_the_number_of_volume_replicas(replica_ctx):
    """a user attempts to decrease the number of volume replicas."""
    volume = set_num_volume_replicas(num_desired_volume_replicas() - 1)
    replica_ctx[REPLICA_CONTEXT_KEY] = volume.spec.num_replicas


@when("a user attempts to decrease the number of volume replicas from 2 to 1")
def a_user_attempts_to_decrease_the_number_of_volume_replicas_from_2_to_1():
    """a user attempts to decrease the number of volume replicas from 2 to 1."""
    assert num_desired_volume_replicas() == 2
    volume = set_num_volume_replicas(1)
    assert volume.spec.num_replicas == 1


@when("a user attempts to increase the number of volume replicas")
def a_user_attempts_to_increase_the_number_of_volume_replicas(replica_ctx):
    """a user attempts to increase the number of volume replicas."""
    volume = ApiClient.volumes_api().put_volume_replica_count(
        VOLUME_UUID, NUM_VOLUME_REPLICAS + 1
    )
    replica_ctx[REPLICA_CONTEXT_KEY] = volume.spec.num_replicas


@when("the number of runtime replicas is 1")
def the_number_of_runtime_replicas_is_1():
    """the number of runtime replicas is 1."""
    # Stopping an io-engine instance will cause a replica to be faulted and removed from the volume.
    Docker.stop_container(NODE_2_NAME)
    # Wait for the replica to be removed from the volume.
    wait_for_volume_replica_count(1)
    yield
    Docker.restart_container(NODE_2_NAME)
    time.sleep(1)


@then("a replica should be removed from the volume")
def a_replica_should_be_removed_from_the_volume(replica_ctx):
    """a replica should be removed from the volume."""
    assert replica_ctx[REPLICA_CONTEXT_KEY] == num_runtime_volume_replicas()


@then("an additional replica should be added to the volume")
def an_additional_replica_should_be_added_to_the_volume(replica_ctx):
    """an additional replica should be added to the volume."""
    assert replica_ctx[REPLICA_CONTEXT_KEY] == num_runtime_volume_replicas()


@then("setting the number of replicas to zero should fail with a suitable error")
def setting_the_number_of_replicas_to_zero_should_fail_with_a_suitable_error():
    """the replica removal should fail with a suitable error."""
    volumes_api = ApiClient.volumes_api()
    volume = volumes_api.get_volume(VOLUME_UUID)
    assert hasattr(volume.state, "target")
    try:
        volumes_api.put_volume_replica_count(VOLUME_UUID, 0)
    except Exception as e:
        # TODO: Return a proper error rather than asserting for a substring
        assert "ApiValueError" in str(type(e))


@then("the volume spec should show 1 replica")
def the_volume_spec_should_show_1_replica():
    """the volume spec should show 1 replica."""
    assert num_desired_volume_replicas() == 1


# Wait for the number of runtime volume replicas to reach the expected number of replicas.
@retry(wait_fixed=200, stop_max_attempt_number=20)
def wait_for_volume_replica_count(expected_num_replicas):
    assert num_runtime_volume_replicas() == expected_num_replicas


# Get the number of replicas from the volume state.
def num_runtime_volume_replicas():
    volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    assert hasattr(volume.state, "target")
    nexus = volume.state.target
    return len(nexus["children"])


# Get the number of replicase from the volume spec.
def num_desired_volume_replicas():
    volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    return volume.spec.num_replicas


# Set the volume spec to have the desired number of replicas.
def set_num_volume_replicas(num_replicas):
    volumes_api = ApiClient.volumes_api()
    volume = volumes_api.put_volume_replica_count(VOLUME_UUID, num_replicas)
    return volume
