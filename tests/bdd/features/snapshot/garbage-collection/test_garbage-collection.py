"""Volume Snapshot garbage collection feature tests."""

import os

import pytest
from pytest_bdd import given, scenario, then, when, parsers
from retrying import retry
import openapi
from common.docker import Docker
from common.apiclient import ApiClient
from common.deployer import Deployer
from common.operations import Volume, Snapshot, wait_node_online
from openapi.model.create_pool_body import CreatePoolBody
from openapi.model.create_volume_body import CreateVolumeBody
from openapi.model.volume_share_protocol import VolumeShareProtocol
from openapi.model.publish_volume_body import PublishVolumeBody
from openapi.model.volume_policy import VolumePolicy

VOLUME1_UUID = "d01b8bfb-0116-47b0-a03a-447fcbdc0e99"
POOL_DISK1 = "pdisk1.img"
POOL1_NAME = "pool-1"
NODE1 = "io-engine-1"
NODE2 = "io-engine-2"
VOLUME1_SIZE = 1024 * 1024 * 10
SNAP1_UUID = "3f49d30d-a446-4b40-b3f6-f439345f1ce9"


@scenario("garbage-collection.feature", "Garbage collection for failed transactions")
def test_garbage_collection_for_failed_transactions():
    """Garbage collection for failed transactions."""


@scenario(
    "garbage-collection.feature",
    "Garbage collection for stuck creating snapshots when source is deleted",
)
def test_garbage_collection_for_stuck_creating_snapshots_when_source_is_deleted():
    """Garbage collection for stuck creating snapshots when source is deleted."""


@pytest.fixture(scope="module")
def create_pool_disk_images():
    pools = Deployer.create_disks(1, size=100 * 1024 * 1024)
    yield pools
    Deployer.cleanup_disks(len(pools))


@pytest.fixture(scope="module")
def setup(create_pool_disk_images):
    Deployer.start(
        io_engines=1,
        cache_period="50ms",
        reconcile_period="50ms",
        request_timeout="300ms",
        no_min_timeouts=True,
        io_engine_env="MAYASTOR_HB_INTERVAL_SEC=1",
    )
    ApiClient.pools_api().put_node_pool(
        NODE1,
        POOL1_NAME,
        CreatePoolBody(["aio://{}".format(create_pool_disk_images[0])]),
    )
    pytest.exception = None
    yield
    Deployer.stop()


@given("a deployer cluster")
def a_deployer_cluster(setup):
    """a deployer cluster."""


@given("the node hosting the replica is paused")
def the_node_hosting_the_replica_is_paused():
    """the node hosting the replica is paused."""
    Docker.pause_container(NODE1)


@given(parsers.parse("we have a single replica {publish_status} volume"))
def we_have_single_replica_publish_status_volume(publish_status):
    """we have single replica <publish_status> volume."""
    ApiClient.volumes_api().put_volume(
        VOLUME1_UUID,
        CreateVolumeBody(
            VolumePolicy(True),
            replicas=1,
            size=VOLUME1_SIZE,
            thin=False,
        ),
    )
    if publish_status == "published":
        ApiClient.volumes_api().put_volume_target(
            VOLUME1_UUID,
            publish_volume_body=PublishVolumeBody(
                {}, VolumeShareProtocol("nvmf"), node=NODE1, frontend_node="app-node-1"
            ),
        )
    yield
    Volume.cleanup(VOLUME1_UUID)


@when("a get snapshot call is made for the snapshot")
def a_get_snapshot_call_is_made_for_the_snapshot():
    """a get snapshot call is made for the snapshot."""


@when("the node hosting the replica is unpaused")
def the_node_hosting_the_replica_is_unpaused():
    """the node hosting the replica is unpaused."""
    Docker.unpause_container(NODE1)
    wait_node_online(NODE1)


@when("the source volume is deleted")
def the_source_volume_is_deleted():
    """the source volume is deleted."""
    ApiClient.volumes_api().del_volume(VOLUME1_UUID)


@when("we attempt to create a snapshot for the volume")
def we_attempt_to_create_a_snapshot_for_the_volume():
    """we attempt to create a snapshot for the volume."""
    try:
        ApiClient.snapshots_api().put_volume_snapshot(VOLUME1_UUID, SNAP1_UUID)
    except openapi.exceptions.ServiceException as e:
        pytest.exception = e
    yield
    Snapshot.cleanup(SNAP1_UUID)


@when("we attempt to create a snapshot with the same id for the volume")
def we_attempt_to_create_a_snapshot_with_the_same_id_for_the_volume():
    """we attempt to create a snapshot with the same id for the volume."""
    try:
        ApiClient.snapshots_api().put_volume_snapshot(VOLUME1_UUID, SNAP1_UUID)
    except openapi.exceptions.ApiException as api_exception:
        if api_exception.status == 422:
            pytest.skip(
                "Skipping the test as the resource has been created in the first attempt, no garbage collection would happen after this."
            )
        else:
            pytest.exception = api_exception
    except openapi.exceptions.ServiceException as service_exception:
        pytest.exception = service_exception
    yield
    Snapshot.cleanup(SNAP1_UUID)


@then("the corresponding snapshot stuck in creating state should be deleted")
@retry(wait_fixed=100, stop_max_attempt_number=50)
def the_corresponding_snapshot_stuck_in_creating_state_should_be_deleted():
    """the corresponding snapshot stuck in creating state should be deleted."""
    try:
        response = ApiClient.snapshots_api().get_volumes_snapshot(SNAP1_UUID)
        # If the snapshot is found it should be in created state, nothing to garbage collect here.
        assert len(response.definition.metadata.transactions) == 1
        assert response.definition.metadata.transactions["1"] is not None
        assert len(response.definition.metadata.transactions["1"]) == 1
        assert (
            str(response.definition.metadata.transactions["1"][0]["status"])
            == "Created"
        )
    except openapi.exceptions.NotFoundException:
        pass


@then("the response should not contain the failed transactions")
@retry(wait_fixed=100, stop_max_attempt_number=50)
def the_response_should_not_contain_the_failed_transactions():
    """the response should not contain the failed transactions."""
    response = ApiClient.snapshots_api().get_volumes_snapshot(SNAP1_UUID)
    assert len(response.definition.metadata.transactions) == 1
    transactions = list(response.definition.metadata.transactions.values())[0]
    assert transactions is not None
    assert len(transactions) == 1
    assert str(transactions[0]["status"]) == "Created"


@then("the snapshot creation should be successful")
def the_snapshot_creation_should_be_successful():
    """the snapshot creation should be successful."""
    assert pytest.exception is None


@then("the snapshot creation should fail")
def the_snapshot_creation_should_fail():
    """the snapshot creation should fail."""
    assert pytest.exception is not None
    pytest.exception = None
