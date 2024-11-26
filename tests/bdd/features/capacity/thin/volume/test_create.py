"""Thin Provisioning - Volume Creation feature tests."""

from pytest_bdd import (
    given,
    scenario,
    then,
    when,
)

import pytest
import os
import requests

from common.deployer import Deployer
from common.apiclient import ApiClient

from openapi.model.create_pool_body import CreatePoolBody
from openapi.model.create_volume_body import CreateVolumeBody
from openapi.model.create_replica_body import CreateReplicaBody
from openapi.model.volume_policy import VolumePolicy

VOLUME_UUID = "5cd5378e-3f05-47f1-a830-a0f5873a1449"
VOLUME_SIZE = 104857600
POOL_SIZE = 104857600 * 3
NUM_VOLUME_REPLICAS = 1
CREATE_REQUEST_KEY = "create_request"
POOL_UUID = "4cc6ee64-7232-497d-a26f-38284a444980"
REPL_UUID = "4cc6ee64-7232-497d-a26f-38284a444981"
NODE_NAME = "io-engine-1"


# This fixture will be automatically used by all tests.
# It starts the deployer which launches all the necessary containers.
# A pool is created for convenience such that it is available for use by the tests.
@pytest.fixture(autouse=True)
def init(disks):
    Deployer.start(1)
    Deployer.create_disks(1, POOL_SIZE)
    ApiClient.pools_api().put_node_pool(
        NODE_NAME, POOL_UUID, CreatePoolBody([f"{disks[0]}"])
    )
    # Add some used space here
    ApiClient.replicas_api().put_pool_replica(
        POOL_UUID, REPL_UUID, CreateReplicaBody(size=VOLUME_SIZE, thin=False)
    )

    yield
    Deployer.stop()


@pytest.fixture
def disks():
    yield Deployer.create_disks(1, size=POOL_SIZE)
    Deployer.cleanup_disks(1)


@pytest.fixture(scope="function")
def create_request():
    return {}


# Fixture used to pass the volume create request between test steps.
@pytest.fixture(scope="function")
def vol_request(create_request):
    return create_request[CREATE_REQUEST_KEY]


@scenario("create.feature", "Creating a thick provisioned volume")
def test_creating_a_thick_provisioned_volume():
    """Creating a thick provisioned volume."""


@scenario("create.feature", "Creating a thin provisioned volume")
def test_creating_a_thin_provisioned_volume():
    """Creating a thin provisioned volume."""


@scenario("create.feature", "Creating an oversized thick provisioned volume")
def test_creating_an_oversized_thick_provisioned_volume():
    """Creating an oversized thick provisioned volume."""


@scenario("create.feature", "Creating an over-committed thin volume")
def test_creating_an_overcommitted_thin_volume():
    """Creating an over-committed thin volume."""


@given("a control plane, Io-Engine instances and a pool")
def a_control_plane_ioengine_instances_and_a_pool(init):
    """a control plane, Io-Engine instances and a pool."""


@given("a request for a thin provisioned volume")
def a_request_for_a_thin_provisioned_volume(create_request):
    """a request for a thin provisioned volume."""
    request = CreateVolumeBody(
        VolumePolicy(False), NUM_VOLUME_REPLICAS, VOLUME_SIZE, True
    )
    request.uuid = VOLUME_UUID
    create_request[CREATE_REQUEST_KEY] = request


@given("a request for a thick provisioned volume")
def a_request_for_a_thick_provisioned_volume(create_request):
    """a request for a thick provisioned volume."""
    request = CreateVolumeBody(
        VolumePolicy(False), NUM_VOLUME_REPLICAS, VOLUME_SIZE, False
    )
    request.uuid = VOLUME_UUID
    create_request[CREATE_REQUEST_KEY] = request


@when("the request size exceeds available pools free space")
def the_request_size_exceeds_available_pools_free_space(vol_request):
    """the request size exceeds available pools free space."""
    pool = ApiClient.pools_api().get_pool(POOL_UUID)
    assert pool.id == POOL_UUID
    assert len(ApiClient.pools_api().get_pools()) == 1

    pool_free = pool.state.capacity - pool.state.used
    vol_request.size = pool_free + 4 * 1024 * 1024
    assert vol_request.size > pool_free


@when("a volume is successfully created")
@then("a volume is successfully created")
def a_volume_is_successfully_created(vol_request):
    """a volume is successfully created."""
    pool = ApiClient.pools_api().get_pool(POOL_UUID)
    assert pool.id == POOL_UUID
    assert len(ApiClient.pools_api().get_pools()) == 1

    vol_request.previous_pool_usage = pool.state.used
    yield ApiClient.volumes_api().put_volume(vol_request.uuid, vol_request)

    ApiClient.volumes_api().del_volume(vol_request.uuid)


@then("its replicas are reported to be thick provisioned")
def its_replicas_are_reported_to_be_thick_provisioned(vol_request):
    """its replicas are reported to be thick provisioned."""
    volume = ApiClient.volumes_api().get_volume(vol_request.uuid)
    replicas = volume.state.replica_topology
    for replica_uuid in replicas:
        replica = ApiClient.replicas_api().get_replica(replica_uuid)
        assert replica.thin is False


@then("its replicas are reported to be thin provisioned")
def its_replicas_are_reported_to_be_thin_provisioned(vol_request):
    """its replicas are reported to be thin provisioned."""
    volume = ApiClient.volumes_api().get_volume(vol_request.uuid)
    replicas = volume.state.replica_topology
    for replica_uuid in replicas:
        replica = ApiClient.replicas_api().get_replica(replica_uuid)
        assert replica.thin is True


@then("the pools usage increases by 4MiB and metadata")
def the_pools_usage_increases_by_4mib_and_metadata(vol_request):
    """the pools usage increases by 4MiB and metadata."""
    pool = ApiClient.pools_api().get_pool(POOL_UUID)
    assert len(ApiClient.pools_api().get_pools()) == 1

    label_part = 4 * 1024 * 1024
    metadata = 4 * 1024 * 1024
    assert pool.state.used == vol_request.previous_pool_usage + label_part + metadata


@then("the pools usage increases by volume size")
def the_pools_usage_increases_by_volume_size(vol_request):
    """the pools usage increases by volume size."""
    pool = ApiClient.pools_api().get_pool(POOL_UUID)
    assert len(ApiClient.pools_api().get_pools()) == 1

    assert pool.state.used == vol_request.previous_pool_usage + vol_request.size


@then("volume creation fails due to insufficient space")
def volume_creation_fails_due_to_insufficient_space(vol_request):
    """volume creation fails due to insufficient space."""
    try:
        ApiClient.volumes_api().put_volume(vol_request.uuid, vol_request)
    except Exception as e:
        exception_info = e.__dict__
        assert exception_info["status"] == requests.codes["insufficient_storage"]
    finally:
        # Check that the volume wasn't created.
        volumes = ApiClient.volumes_api().get_volumes().entries
        assert len(volumes) == 0
