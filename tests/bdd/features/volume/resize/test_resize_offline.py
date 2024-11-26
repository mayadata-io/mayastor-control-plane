"""Volume resize feature tests."""

from pytest_bdd import (
    given,
    scenario,
    then,
    when,
    parsers,
)

import http
import os
import pytest
import time
import subprocess

from urllib.parse import urlparse
from retrying import retry
from common.deployer import Deployer
from common.apiclient import ApiClient
from common.docker import Docker
from common.operations import Snapshot, Volume
from time import sleep

import openapi.exceptions
from openapi.model.create_pool_body import CreatePoolBody
from openapi.model.create_volume_body import CreateVolumeBody
from common.fio import Fio
from openapi.model.nexus_state import NexusState
from openapi.model.pool_status import PoolStatus
from openapi.model.protocol import Protocol
from openapi.model.publish_volume_body import PublishVolumeBody
from openapi.model.replica_state import ReplicaState
from openapi.model.spec_status import SpecStatus
from openapi.model.resize_volume_body import ResizeVolumeBody
from openapi.model.volume_policy import VolumePolicy
from openapi.model.volume_status import VolumeStatus


POOL1_UUID = "91a60318-bcfe-4e36-92cb-ddc7abf212ea"
POOL2_UUID = "92a60318-bcfe-4e36-92cb-ddc7abf212ea"
POOL3_UUID = "93a60318-bcfe-4e36-92cb-ddc7abf212ea"
POOL4_UUID = "94a60318-bcfe-4e36-92cb-ddc7abf212ea"
VOLUME_UUID = "5cd5378e-3f05-47f1-a830-a0f5873a1449"
RESTORE_VOLUME_UUID = "6cd5378e-3f05-47f1-a830-a0f5873a1449"
SNAP_UUID_1 = "7cd5378e-3f05-47f1-a830-a0f5873a1449"
SNAP_UUID_2 = "8cd5378e-3f05-47f1-a830-a0f5873a1449"
DEFAULT_REPLICA_CNT = 3
DEFAULT_POOL_SIZE = 209715200
NODE1_NAME = "io-engine-1"
NODE2_NAME = "io-engine-2"
NODE3_NAME = "io-engine-3"
NODE4_NAME = "io-engine-4"
VOLUME_CTX_KEY = "volume"
VOLUME_SIZE = 52428800  # 50MiB
VOLUME_NEW_SIZE = 83886080  # 80MiB
VOLUME_SHRINK_SIZE = 41943040  # 40MiB


# utility helper functions - BEGIN
def cordon_node(node_name, label):
    ApiClient.nodes_api().put_node_cordon(node_name, label)
    assert is_cordoned(node_name) == True


def is_cordoned(node_name):
    node = ApiClient.nodes_api().get_node(node_name)
    present = False
    try:
        assert node.spec.cordondrainstate["cordonedstate"]["cordonlabels"] != []
        return True
    except AttributeError as e:
        return False


def uncordon_node(node_name, label):
    try:
        ApiClient.nodes_api().delete_node_cordon(node_name, label)
        pytest.command_failed = False
    except Exception as e:
        pytest.command_failed = True


def create_volume_only(uuid, size, rcount):
    volume = ApiClient.volumes_api().put_volume(
        uuid,
        CreateVolumeBody(VolumePolicy(True), int(rcount), size, False),
    )
    assert volume.spec.uuid == uuid
    replicas = volume.state.replica_topology
    assert len(replicas) == int(rcount)
    pytest.volume = volume
    pytest.fio = None
    return volume


def publish_volume(uuid, publish_on):
    volume = ApiClient.volumes_api().put_volume_target(
        uuid,
        publish_volume_body=PublishVolumeBody(
            {}, Protocol("nvmf"), node=publish_on, frontend_node=""
        ),
    )
    assert hasattr(volume.state, "target")


def unpublish_volume(uuid):
    volume = ApiClient.volumes_api().del_volume_target(uuid)
    assert not hasattr(volume.spec, "target")


def create_and_publish_volume(uuid, size, rcount, publish_on):
    volume = create_volume_only(uuid, size, rcount)
    # Publish the volume now.
    volume = ApiClient.volumes_api().put_volume_target(
        uuid,
        publish_volume_body=PublishVolumeBody(
            {}, Protocol("nvmf"), node=publish_on, frontend_node=""
        ),
    )
    assert hasattr(volume.state, "target")
    pytest.volume = volume
    return volume


def create_volume_snapshot(volume, snapid):
    snapshot = ApiClient.snapshots_api().put_volume_snapshot(volume.spec.uuid, snapid)
    return snapshot


# utility helper functions - END


@scenario(
    "resize_offline.feature",
    "Expand a published volume after unpublishing it while having an offline replica",
)
def test_expand_a_published_volume_after_unpublishing_it_while_having_an_offline_replica():
    """Expand a published volume after unpublishing it while having an offline replica."""


@scenario("resize_offline.feature", "Expand an unpublished volume")
def test_expand_an_unpublished_volume():
    """Expand an unpublished volume."""


@scenario(
    "resize_offline.feature", "Expand an unpublished volume and then make it published"
)
def test_expand_an_unpublished_volume_and_then_make_it_published():
    """Expand an unpublished volume and then make it published."""


@scenario(
    "resize_offline.feature", "Expand an unpublished volume with an offline replica"
)
def test_expand_an_unpublished_volume_with_an_offline_replica():
    """Expand an unpublished volume with an offline replica."""


@scenario("resize_offline.feature", "Shrink an unpublished volume")
def test_shrink_an_unpublished_volume():
    """Shrink an unpublished volume."""


@scenario("resize_offline.feature", "Expand a volume and take a snapshot")
def test_expand_a_volume_and_take_a_snapshot():
    """Expand a volume and take a snapshot."""


@scenario("resize_offline.feature", "Expand a new volume created as a snapshot restore")
def test_expand_a_new_volume_created_as_a_snapshot_restore():
    """Expand a new volume created as a snapshot restore."""


@scenario("resize_offline.feature", "Take a snapshot and expand the volume")
def test_take_a_snapshot_and_expand_the_volume():
    """Take a snapshot and expand the volume."""


@given("a deployer cluster")
def a_deployer_cluster():
    """a deployer cluster."""


# Fixtures - BEGIN
@pytest.fixture(scope="module")
def disks():
    pools = Deployer.create_disks(DEFAULT_REPLICA_CNT + 1, size=DEFAULT_POOL_SIZE)
    yield pools
    Deployer.cleanup_disks(len(pools))


# Fixtures - END


# This fixture will be automatically used by all tests.
# It starts the deployer which launches all the necessary containers.
# Pools are created for convenience such that they are available for use by the tests.
@pytest.fixture(autouse=True, scope="module")
def init(disks):
    Deployer.start(
        4,
        faulted_child_wait_period="5s",
        reconcile_period="100ms",
        cache_period="100ms",
        fio_spdk=True,
        io_engine_coreisol=True,
    )
    assert len(disks) == DEFAULT_REPLICA_CNT + 1
    ApiClient.pools_api().put_node_pool(
        NODE1_NAME, POOL1_UUID, CreatePoolBody([f"aio://{disks[0]}"])
    )
    ApiClient.pools_api().put_node_pool(
        NODE2_NAME, POOL2_UUID, CreatePoolBody([f"aio://{disks[1]}"])
    )
    ApiClient.pools_api().put_node_pool(
        NODE3_NAME, POOL3_UUID, CreatePoolBody([f"aio://{disks[2]}"])
    )
    # Create an additional pool that is need for some tests, but keep it cordoned
    # until required.
    ApiClient.pools_api().put_node_pool(
        NODE4_NAME, POOL4_UUID, CreatePoolBody([f"aio://{disks[3]}"])
    )
    # Cordon the additional node in the beginning so that nothing gets placed there
    # in the start.
    cordon_node(NODE4_NAME, "dont_place_anything_here")

    pytest.replica_count = DEFAULT_REPLICA_CNT
    yield
    Deployer.stop()


@pytest.fixture(scope="function")
def create_volume(disks):
    volume = create_volume_only(VOLUME_UUID, VOLUME_SIZE, pytest.replica_count)
    yield volume
    Volume.cleanup(volume)


@given("a published volume with more than one replica and all are healthy")
def a_published_volume_with_more_than_one_replica_and_all_are_healthy(create_volume):
    """a published volume with more than one replica and all are healthy."""
    the_volume_is_published()
    pytest.exception = None


@given("a successful snapshot is created for an unpublished volume")
def a_successful_snapshot_is_created_for_an_unpublished_volume(create_volume):
    """a successful snapshot is created for an unpublished volume."""
    assert pytest.volume is not None
    test_volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    create_volume_snapshot(test_volume, SNAP_UUID_1)
    the_snapshot_should_be_successfully_created()
    yield test_volume
    Volume.cleanup(test_volume)
    Snapshot.cleanup(SNAP_UUID_1)


@given(
    parsers.parse(
        "an unpublished volume with {repl_count:d} replicas and all are healthy"
    )
)
@given("an unpublished volume with <repl_count> replicas and all are healthy")
def an_unpublished_volume_with_repl_count_replicas_and_all_are_healthy(repl_count):
    """an unpublished volume with <repl_count> replicas and all are healthy."""
    pytest.exception = None
    pytest.replica_count = repl_count
    volume = create_volume_only(VOLUME_UUID, VOLUME_SIZE, pytest.replica_count)
    yield volume
    Volume.cleanup(volume)


@given("an unpublished volume with more than one replica")
def an_unpublished_volume_with_more_than_one_replica(create_volume):
    """an unpublished volume with more than one replica."""


@given("an unpublished volume with more than one replica and all are healthy")
def an_unpublished_volume_with_more_than_one_replica_and_all_are_healthy(create_volume):
    """an unpublished volume with more than one replica and all are healthy."""


@when("one of the replica goes offline")
def one_of_the_replica_goes_offline():
    """one of the replica goes offline."""
    # Offline NODE3_NAME to make the replica Offline/Unknown
    Docker.stop_container(NODE3_NAME)


@given("one of the replica is not in online state")
def one_of_the_replica_is_not_in_online_state():
    """one of the replica is not in online state."""
    pytest.exception = None
    volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    # Offline NODE3_NAME to make the replica Offline/Unknown
    replica = ApiClient.replicas_api().get_node_replicas(NODE3_NAME)
    assert len(replica) == 1
    Docker.stop_container(NODE3_NAME)
    wait_volume_replica_offline(volume, replica[0])
    yield
    the_replica_comes_online_again()


@when("the volume is receiving IO")
def the_volume_is_receiving_io():
    """the volume is receiving IO."""
    volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    uri = urlparse(volume.state.target["deviceUri"])
    fio = Fio(name="fio-pre-resize", rw="write", uri=uri, extra_args="--loops=2")
    pytest.fio = fio.open()


@when("the volume is unpublished")
def the_volume_is_unpublished():
    """the volume is unpublished."""
    # Ensure fio is still running till unpublish
    time.sleep(0.5)
    if pytest.fio.poll() is None:
        pytest.fio.kill()
    unpublish_volume(VOLUME_UUID)


@when("the replica comes online again")
def the_replica_comes_online_again():
    """the replica comes online again."""
    Docker.restart_container(NODE3_NAME)
    wait_pool_online(POOL3_UUID)


@when("the volume is published")
def the_volume_is_published():
    """the volume is published."""
    publish_volume(VOLUME_UUID, NODE2_NAME)


@when("the volume is republished")
def the_volume_is_republished():
    """the volume is republished."""
    publish_volume(VOLUME_UUID, NODE2_NAME)
    wait_child_added_back()


@when("the volume replica count is increased by 1")
def the_volume_replica_count_is_increased_by_1():
    """the volume replica count is increased by 1."""
    uncordon_node(NODE4_NAME, "dont_place_anything_here")
    volume = ApiClient.volumes_api().put_volume_replica_count(
        VOLUME_UUID, DEFAULT_REPLICA_CNT + 1
    )
    assert len(volume.state.replica_topology) == (DEFAULT_REPLICA_CNT + 1)
    yield
    cordon_node(NODE4_NAME, "dont_place_anything_here")


@when("a new volume is created with the snapshot as its source")
def a_new_volume_is_created_with_the_snapshot_as_its_source():
    """a new volume is created with the snapshot as its source."""
    body = CreateVolumeBody(
        VolumePolicy(True),
        replicas=1,
        size=VOLUME_SIZE,
        thin=True,
    )
    volume = ApiClient.volumes_api().put_snapshot_volume(
        SNAP_UUID_1, RESTORE_VOLUME_UUID, body
    )
    pytest.volume = volume
    yield volume
    Volume.cleanup(volume)


@then("the new volume is published")
def the_new_volume_is_published():
    """the new volume is published."""
    test_volume = pytest.volume
    publish_volume(test_volume.spec.uuid, NODE2_NAME)


@then("the replica's capacity will be same as the snapshot")
def the_replicas_capacity_will_be_same_as_the_snapshot():
    """the replica's capacity will be same as the snapshot."""
    test_volume = pytest.volume
    replicas = list(test_volume.state.replica_topology.values())
    assert len(replicas) == 1
    # Can't compare these size as the state and spec differ by about 2MiB additional
    # padded by spdk.
    # assert replicas[0].usage.capacity == test_volume.spec.size
    assert replicas[0].usage.allocated == 0
    assert replicas[0].usage.allocated_snapshots == 0


@then("a new replica will be created for the new volume")
def a_new_replica_will_be_created_for_the_new_volume():
    """a new replica will be created for the new volume."""
    # check volume has a replica in the topology
    restored_volume = pytest.volume
    assert restored_volume.spec.uuid == RESTORE_VOLUME_UUID
    assert restored_volume.spec.num_replicas == 1
    assert restored_volume.spec.status == SpecStatus("Created")


@when("we issue a volume expand request")
def we_issue_a_volume_expand_request():
    """we issue a volume expand request."""
    try:
        volume = ApiClient.volumes_api().put_volume_size(
            pytest.volume.spec.uuid, ResizeVolumeBody(VOLUME_NEW_SIZE)
        )
        pytest.exception = None
    except openapi.exceptions.ApiException as err:
        pytest.exception = err


@when("we issue a volume shrink request")
def we_issue_a_volume_shrink_request():
    """we issue a volume shrink request."""
    pytest.exception = None
    try:
        ApiClient.volumes_api().put_volume_size(
            VOLUME_UUID, ResizeVolumeBody(VOLUME_SHRINK_SIZE)
        )
    except openapi.exceptions.ApiException as err:
        pytest.exception = err
        volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
        Volume.cleanup(volume)


@when("we take a snapshot of expanded volume")
def we_take_a_snapshot_of_expanded_volume():
    """we take a snapshot of expanded volume."""
    test_volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    create_volume_snapshot(test_volume, SNAP_UUID_1)
    yield
    Snapshot.cleanup(SNAP_UUID_1)


@then("we take a snapshot of expanded volume again")
def we_take_a_snapshot_of_expanded_volume_again():
    """we take a snapshot of expanded volume again."""
    test_volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    create_volume_snapshot(test_volume, SNAP_UUID_2)
    yield
    Snapshot.cleanup(SNAP_UUID_2)


@then("all the replicas of the volume should be resized to the new capacity")
def all_the_replicas_of_the_volume_should_be_resized_to_the_new_capacity():
    """all the replicas of the volume should be resized to the new capacity."""
    volume = ApiClient.volumes_api().get_volume(pytest.volume.spec.uuid)
    replicas = volume.state.replica_topology
    for replica_uuid in replicas:
        replica = ApiClient.replicas_api().get_replica(replica_uuid)
        assert replica.size >= VOLUME_NEW_SIZE


@then("the failure reason should be invalid arguments")
def the_failure_reason_should_be_invalid_arguments():
    """the failure reason should be invalid arguments."""
    # Reason code: Invalid Arguments, mapped from
    # SvcError::VolumeResizeSize
    assert pytest.exception.status == http.HTTPStatus.NOT_ACCEPTABLE


@then("the failure reason should be volume-in-use precondition")
def the_failure_reason_should_be_volumeinuse_precondition():
    """the failure reason should be volume-in-use precondition."""
    # Reason code: Conflict, mapped from SvcError::InUse
    assert pytest.exception.status == http.HTTPStatus.CONFLICT


@then("the new capacity should be available for the application to use")
def the_new_capacity_should_be_available_for_the_application_to_use():
    """the new capacity should be available for the application to use."""
    volume = ApiClient.volumes_api().get_volume(pytest.volume.spec.uuid)
    assert volume.state.target["size"] == VOLUME_NEW_SIZE
    # Run IO across expanded volume capacity
    uri = urlparse(volume.state.target["deviceUri"])
    fio = Fio(name="fio-post-resize", rw="write", uri=uri, size=VOLUME_NEW_SIZE)
    try:
        code = fio.run().returncode
        assert code == 0, "Fio is expected to execute successfully"
    except subprocess.CalledProcessError:
        assert False, "FIO is not expected to be errored out"


@then("the new replica should have expanded size")
def the_new_replica_should_have_expanded_size():
    """the new replica should have expanded size."""
    all_the_replicas_of_the_volume_should_be_resized_to_the_new_capacity()


@then("the onlined replica should be rebuilt")
def the_onlined_replica_should_be_rebuilt():
    """the onlined replica should be rebuilt."""
    vol = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    target = vol.state.target
    childlist = target["children"]
    assert len(childlist) == DEFAULT_REPLICA_CNT
    wait_rebuild_finish()


@then("the request should succeed")
def the_request_should_succeed():
    """the request should succeed."""
    assert pytest.exception is None


@then("the snapshot should be successfully created")
def the_snapshot_should_be_successfully_created():
    """the snapshot should be successfully created."""
    test_volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    snapshots = ApiClient.snapshots_api().get_volumes_snapshots(
        volume_id=test_volume.spec.uuid, max_entries=0
    )
    assert len(snapshots.entries) == 1
    assert snapshots.entries[0].state.uuid == SNAP_UUID_1
    assert snapshots.entries[0].state.source_volume == VOLUME_UUID


@then("the volume expand status should be failure")
def the_volume_expand_status_should_be_failure():
    """the volume expand status should be failure."""
    the_volume_resize_status_should_be_failure()


@then("the volume is expanded to the new capacity")
def the_volume_is_expanded_to_the_new_capacity():
    """the volume is expanded to the new capacity."""
    volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    assert volume.spec.size == VOLUME_NEW_SIZE


@then("the volume resize status should be failure")
def the_volume_resize_status_should_be_failure():
    """the volume resize status should be failure."""
    assert pytest.exception is not None


@then("the volume should get published with expanded capacity")
def the_volume_should_get_published_with_expanded_capacity():
    """the volume should get published with expanded capacity."""
    volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    assert hasattr(volume.spec, "target")
    assert str(volume.spec.target.protocol) == str(Protocol("nvmf"))
    assert volume.spec.size == VOLUME_NEW_SIZE
    assert volume.state.target["size"] == VOLUME_NEW_SIZE


# Retriable functions - BEGIN
@retry(wait_fixed=200, stop_max_attempt_number=30)
def wait_pool_online(pool_id):
    pool = ApiClient.pools_api().get_pool(pool_id)
    assert pool.state.status == PoolStatus("Online")


@retry(wait_fixed=200, stop_max_attempt_number=50)
def wait_rebuild_finish():
    vol = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    assert (vol.state.target["rebuilds"] == 0) and (
        NexusState(vol.state.target["state"]) == NexusState("Online")
    )


@retry(wait_fixed=100, stop_max_attempt_number=30)
def wait_child_added_back():
    vol = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    target = vol.state.target
    childlist = target["children"]
    degraded = list(filter(lambda child: child.get("state") == "Degraded", childlist))
    assert len(degraded) == 1, "Failed to add child again as Degraded!"
    assert target["rebuilds"] == 1


@retry(wait_fixed=100, stop_max_attempt_number=30)
def wait_volume_replica_offline(volume, replica):
    volume = ApiClient.volumes_api().get_volume(volume.spec.uuid)
    replicas = volume.state.replica_topology
    assert replicas.get(replica.uuid).get("state") == ReplicaState("Unknown")


# Retriable functions - END
