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
import datetime
import time
import subprocess

from urllib.parse import urlparse
from retrying import retry
from common.deployer import Deployer
from common.apiclient import ApiClient
from common.docker import Docker
from common.operations import Cluster, Snapshot, Volume
from time import sleep

import openapi.exceptions
from openapi.model.create_pool_body import CreatePoolBody
from openapi.model.create_volume_body import CreateVolumeBody
from openapi.model.volume_share_protocol import VolumeShareProtocol
from openapi.exceptions import NotFoundException
from openapi.model.volume_policy import VolumePolicy
from openapi.model.nexus_state import NexusState
from common.fio import Fio
from openapi.model.child_state import ChildState
from openapi.model.replica_state import ReplicaState
from openapi.model.spec_status import SpecStatus
from openapi.model.pool_status import PoolStatus
from openapi.model.publish_volume_body import PublishVolumeBody
from openapi.model.resize_volume_body import ResizeVolumeBody
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
DEFAULT_POOL_SIZE = 419430400  # 400MiB
NODE1_NAME = "io-engine-1"
NODE2_NAME = "io-engine-2"
NODE3_NAME = "io-engine-3"
NODE4_NAME = "io-engine-4"
VOLUME_CTX_KEY = "volume"
VOLUME_SIZE = 104857600  # 100MiB
VOLUME_NEW_SIZE = 209715200  # 200MiB
VOLUME_SHRINK_SIZE = 52428800  # 50MiB


# fixtures - BEGIN
@pytest.fixture(scope="module")
def disks():
    pools = Deployer.create_disks(DEFAULT_REPLICA_CNT + 1, size=DEFAULT_POOL_SIZE)
    yield pools
    Deployer.cleanup_disks(len(pools))


@pytest.fixture(autouse=True, scope="module")
def init(disks):
    Deployer.start(
        io_engines=4,
        cache_period="100ms",
        reconcile_period="100ms",
        faulted_child_wait_period="1s",
        fio_spdk=True,
        io_engine_coreisol=True,
    )
    # An extra disk for replica placement for rebuild purpose.
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
    pytest.restore_volume = None
    yield
    Deployer.stop()


@pytest.fixture(scope="function")
def create_volume(disks):
    volume = create_and_publish_volume(
        VOLUME_UUID, VOLUME_SIZE, pytest.replica_count, NODE2_NAME
    )
    yield volume
    Volume.cleanup(volume)


@pytest.fixture(scope="function")
def test_volume_factory():
    def _factory():
        return pytest.volume

    return _factory


# fixtures - END


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
            {}, VolumeShareProtocol("nvmf"), node=publish_on, frontend_node=""
        ),
    )
    assert hasattr(volume.state, "target")


def create_and_publish_volume(uuid, size, rcount, publish_on):
    volume = create_volume_only(uuid, size, rcount)
    # Publish the volume now.
    volume = ApiClient.volumes_api().put_volume_target(
        uuid,
        publish_volume_body=PublishVolumeBody(
            {}, VolumeShareProtocol("nvmf"), node=publish_on, frontend_node=""
        ),
    )
    assert hasattr(volume.state, "target")
    pytest.volume = volume
    wait_volume_published(uuid)
    return volume


def create_volume_snapshot(volume, snapid):
    snapshot = ApiClient.snapshots_api().put_volume_snapshot(volume.spec.uuid, snapid)
    return snapshot


def start_fio(volume):
    uri = urlparse(volume.state.target["device_uri"])
    fio = Fio(name="fio-pre-resize", rw="write", uri=uri)
    pytest.fio = fio.open()


def stop_fio():
    if pytest.fio is not None and pytest.fio.poll() is not None:
        pytest.fio.kill()


# utility helper functions - END

# test scenarios and steps - BEGIN


@scenario(
    "resize_online.feature", "Expand a published volume with a rebuilding replica"
)
def test_expand_a_published_volume_with_a_rebuilding_replica():
    """Expand a published volume with a rebuilding replica."""


@scenario("resize_online.feature", "Expand a volume")
def test_expand_a_volume():
    """Expand a volume."""


@scenario("resize_online.feature", "Expand a volume with an offline replica")
def test_expand_a_volume_with_an_offline_replica():
    """Expand a volume with an offline replica."""


@scenario("resize_online.feature", "Shrink a volume")
def test_shrink_a_volume():
    """Shrink a volume."""


@scenario("resize_online.feature", "Expand a volume and take a snapshot")
def test_expand_a_volume_and_take_a_snapshot():
    """Expand a volume and take a snapshot."""


@scenario("resize_online.feature", "Take a snapshot and expand the volume")
def test_take_a_snapshot_and_expand_the_volume():
    """Take a snapshot and expand the volume."""


@scenario("resize_online.feature", "Expand a new volume created as a snapshot restore")
def test_expand_a_new_volume_created_as_a_snapshot_restore():
    """Expand a new volume created as a snapshot restore."""


@given(
    parsers.parse("a published volume with {repl_count:d} replicas and all are healthy")
)
def a_published_volume_with_repl_count_replicas_and_all_are_healthy(repl_count):
    """a published volume with <repl_count> replicas and all are healthy."""
    volume = None
    volume = create_and_publish_volume(VOLUME_UUID, VOLUME_SIZE, repl_count, NODE2_NAME)
    start_fio(volume)

    pytest.exception = None
    yield volume
    Volume.cleanup(volume)


@given("a published volume with more than one replica and all are healthy")
def a_published_volume_with_more_than_one_replica_and_all_are_healthy():
    """a published volume with more than one replica and all are healthy."""
    volume = None
    volume = create_and_publish_volume(VOLUME_UUID, VOLUME_SIZE, 3, NODE2_NAME)
    start_fio(volume)

    pytest.exception = None
    yield volume
    Volume.cleanup(volume)


@given("a deployer cluster")
def a_deployer_cluster():
    """a deployer cluster."""
    pass


@given("a published volume with more than one replicas")
def a_published_volume_with_more_than_one_replicas(create_volume):
    """a published volume with more than one replicas."""


@given("a successful snapshot is created for a published volume")
def a_successful_snapshot_is_created_for_a_published_volume(
    create_volume, test_volume_factory
):
    """a successful snapshot is created for a published volume."""
    assert pytest.volume is not None
    test_volume = test_volume_factory()
    create_volume_snapshot(test_volume, SNAP_UUID_1)
    the_snapshot_should_be_successfully_created(test_volume_factory)
    yield test_volume
    Volume.cleanup(test_volume)
    Snapshot.cleanup(SNAP_UUID_1)


@given("one of the replica is not in online state")
def one_of_the_replica_is_not_in_online_state():
    """one of the replica is not in online state."""
    volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    replica = ApiClient.replicas_api().get_node_replicas(NODE3_NAME)
    assert len(replica) == 1
    # Offline NODE3_NAME to make the replica Offline/Unknown
    Docker.stop_container(NODE3_NAME)
    wait_volume_replica_offline(volume, replica[0])
    yield
    # Bring the node back up.
    if Docker.container_status(NODE3_NAME) != "running":
        Docker.restart_container(NODE3_NAME)
        wait_pool_online(POOL3_UUID)
        wait_volume_online(VOLUME_UUID)


@given("one of the replica is undergoing rebuild")
def one_of_the_replica_is_undergoing_rebuild():
    """one of the replica is undergoing rebuild."""
    uncordon_node(NODE4_NAME, "dont_place_anything_here")
    wait_cordon_remove(NODE4_NAME)
    # Offline NODE3_NAME to make the replica Faulted and retired.
    Docker.stop_container(NODE3_NAME)
    # Replica should be placed on spare node 4 pool now.
    check_replica_online(NODE4_NAME)
    wait_rebuild_start()
    yield
    cordon_node(NODE4_NAME, "dont_place_anything_here")
    # Docker.stop_container(NODE4_NAME)
    # Bring the origninal replica node back up.
    if Docker.container_status(NODE3_NAME) != "running":
        Docker.restart_container(NODE3_NAME)
        wait_pool_online(POOL3_UUID)
    wait_volume_online(VOLUME_UUID)


@given("the volume is receiving IO")
def the_volume_is_receiving_io(test_volume_factory):
    test_volume = test_volume_factory()
    """the volume is receiving IO."""
    start_fio(test_volume)


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


@when("we issue a volume expand request")
def we_issue_a_volume_expand_request(test_volume_factory):
    """we issue a volume expand request."""
    test_volume = test_volume_factory()
    try:
        volume = ApiClient.volumes_api().put_volume_size(
            test_volume.spec.uuid, ResizeVolumeBody(VOLUME_NEW_SIZE)
        )
        pytest.exception = None
        pytest.volume = volume
    except openapi.exceptions.ApiException as err:
        pytest.exception = err


@when("we issue a volume shrink request")
def we_issue_a_volume_shrink_request():
    """we issue a volume shrink request."""
    try:
        volume = ApiClient.volumes_api().put_volume_size(
            VOLUME_UUID, ResizeVolumeBody(VOLUME_SHRINK_SIZE)
        )
        pytest.volume = volume
        pytest.exception = None
    except openapi.exceptions.ApiException as err:
        pytest.exception = err


@when("we take a snapshot of expanded volume")
def we_take_a_snapshot_of_expanded_volume(test_volume_factory):
    """we take a snapshot of expanded volume."""
    test_volume = test_volume_factory()
    create_volume_snapshot(test_volume, SNAP_UUID_1)
    yield
    Snapshot.cleanup(SNAP_UUID_1)


@then("we take a snapshot of expanded volume again")
def we_take_a_snapshot_of_expanded_volume_again(test_volume_factory):
    """we take a snapshot of expanded volume again."""
    test_volume = test_volume_factory()
    create_volume_snapshot(test_volume, SNAP_UUID_2)
    yield
    Snapshot.cleanup(SNAP_UUID_2)


@then("IO on the new volume runs without error for the complete volume size")
def io_on_the_new_volume_runs_without_error_for_the_complete_volume_size(
    test_volume_factory,
):
    """IO on the new volume runs without error for the complete volume size."""
    test_volume = test_volume_factory()

    uri = urlparse(test_volume.state.target["device_uri"])
    fio = Fio(name="fio-restored-volume", rw="write", uri=uri)

    fio_proc = fio.open()
    assert fio_proc.poll() is None
    try:
        code = fio_proc.wait(timeout=30)
        assert code == 0, "FIO failed, exit code: %d" % code
    except subprocess.TimeoutExpired:
        # Fio timed out, still not terminated.
        fio_proc.kill()


@then("a new replica will be created for the new volume")
def a_new_replica_will_be_created_for_the_new_volume(test_volume_factory):
    """a new replica will be created for the new volume."""
    # check volume has a replica in the topology
    restored_volume = test_volume_factory()
    assert restored_volume.spec.uuid == RESTORE_VOLUME_UUID
    assert restored_volume.spec.num_replicas == 1
    assert restored_volume.spec.status == SpecStatus("Created")


@then("all the replicas of volume should be resized to new capacity")
def all_the_replicas_of_volume_should_be_resized_to_new_capacity(test_volume_factory):
    """all the replicas of volume should be resized to new capacity."""
    test_volume = test_volume_factory()
    replicas = test_volume.state.replica_topology
    for replica_uuid in replicas:
        replica = ApiClient.replicas_api().get_replica(replica_uuid)
        assert replica.size >= VOLUME_NEW_SIZE


@then("the new capacity should be available for the application")
def the_new_capacity_should_be_available_for_the_application(test_volume_factory):
    """the new capacity should be available for the application."""
    test_volume = test_volume_factory()
    # If it's a published volume, start a fio instance again that'll do
    # IO to the complete expanded volume.
    if hasattr(test_volume.state, "target"):
        assert test_volume.state.target["size"] >= VOLUME_NEW_SIZE
        uri = urlparse(test_volume.state.target["device_uri"])
        fio = Fio(name="fio-post-resize", rw="write", uri=uri)
        # If previous fio is running, kill and start fresh.
        if pytest.fio is not None and pytest.fio.poll() is not None:
            pytest.fio.kill()

        pytest.fio = fio.open()
        assert pytest.fio.poll() is None
        try:
            code = pytest.fio.wait(timeout=30)
            assert code == 0, "FIO failed, exit code: %d" % code
        except subprocess.TimeoutExpired:
            # Fio timed out, still not terminated.
            pytest.fio.kill()


@then("the failure reason should be invalid arguments")
def the_failure_reason_should_be_invalid_arguments():
    """the failure reason should be invalid arguments."""
    # Reason code: Invalid Arguments, mapped from
    # SvcError::VolumeResizeSize
    assert pytest.exception.status == http.HTTPStatus.NOT_ACCEPTABLE


@then("the new capacity should be available for the application to use")
def the_new_capacity_should_be_available_for_the_application_to_use():
    """the new capacity should be available for the application to use."""
    pass


@then("the new volume is published")
def the_new_volume_is_published(test_volume_factory):
    """the new volume is published."""
    test_volume = test_volume_factory()
    publish_volume(test_volume.spec.uuid, NODE2_NAME)


@then("the replica's capacity will be same as the snapshot")
def the_replicas_capacity_will_be_same_as_the_snapshot(test_volume_factory):
    """the replica's capacity will be same as the snapshot."""
    test_volume = test_volume_factory()
    replicas = list(test_volume.state.replica_topology.values())
    assert len(replicas) == 1
    assert replicas[0].usage.capacity == test_volume.spec.size
    assert replicas[0].usage.allocated == 0
    assert replicas[0].usage.allocated_snapshots == 0


@then("the request should succeed")
def the_request_should_succeed():
    """the request should succeed."""
    assert pytest.exception is None


@then("the snapshot should be successfully created")
def the_snapshot_should_be_successfully_created(test_volume_factory):
    """the snapshot should be successfully created."""
    test_volume = test_volume_factory()
    snapshots = ApiClient.snapshots_api().get_volumes_snapshots(
        volume_id=test_volume.spec.uuid, max_entries=0
    )
    assert len(snapshots.entries) == 1
    assert snapshots.entries[0].state.uuid == SNAP_UUID_1
    assert snapshots.entries[0].state.source_volume == VOLUME_UUID


@then("the volume expand status should be failure")
def the_volume_expand_status_should_be_failure():
    """the volume expand status should be failure."""
    assert pytest.exception is not None
    # If fio is running, stop it.
    stop_fio()


@then("the volume resize status should be failure")
def the_volume_resize_status_should_be_failure():
    """the volume resize status should be failure."""
    assert pytest.exception is not None
    # If fio is running, stop it.
    stop_fio()


@then("the volume should be expanded to the new capacity")
def the_volume_should_be_expanded_to_the_new_capacity(test_volume_factory):
    """the volume should be expanded to the new capacity."""
    test_volume = test_volume_factory()
    # spec.operation is cleared when volume resize completes.
    assert hasattr(test_volume.spec, "operation") is False
    if pytest.fio is not None:
        fio = pytest.fio
        try:
            code = fio.wait(timeout=30)
            assert code == 0, "FIO failed, exit code: %d" % code
        except subprocess.TimeoutExpired:
            # Fio timed out, still not terminated.
            fio.kill()


@then("the volume target should get resized to new capacity")
def the_volume_target_should_get_resized_to_new_capacity(test_volume_factory):
    """the volume target should get resized to new capacity."""
    test_volume = test_volume_factory()
    if hasattr(test_volume.state, "target"):
        assert test_volume.state.target["size"] >= VOLUME_NEW_SIZE


# test scenarios and steps - END

# Retriable fucntions - BEGIN


@retry(wait_fixed=100, stop_max_attempt_number=30)
def wait_volume_replica_offline(volume, replica):
    volume = ApiClient.volumes_api().get_volume(volume.spec.uuid)
    replicas = volume.state.replica_topology
    assert replicas.get(replica.uuid).get("state") == ReplicaState("Unknown")


@retry(wait_fixed=500, stop_max_attempt_number=20)
def wait_pool_online(pool_id):
    pool = ApiClient.pools_api().get_pool(pool_id)
    assert pool.state.status == PoolStatus("Online")


@retry(wait_fixed=200, stop_max_attempt_number=30)
def wait_volume_published(vol_id):
    volume = ApiClient.volumes_api().get_volume(vol_id)
    assert volume.state.target["state"] == NexusState("Online")


@retry(wait_fixed=500, stop_max_attempt_number=30)
def wait_volume_online(uuid):
    volume = ApiClient.volumes_api().get_volume(uuid)
    assert volume.state.status == VolumeStatus("Online")


@retry(wait_fixed=100, stop_max_attempt_number=30)
def check_replica_online(node):
    replica = ApiClient.replicas_api().get_node_replicas(node)
    if len(replica) == 1:
        assert replica[0].state == ReplicaState("Online")
    else:
        assert False, "Expected a replica to be available on " + node


@retry(wait_fixed=100, stop_max_attempt_number=30)
def wait_rebuild_start():
    vol = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    childlist = vol.state.target["children"]
    assert (len(childlist) == DEFAULT_REPLICA_CNT) and (
        vol.state.target["state"] == NexusState("Degraded")
    )


@retry(wait_fixed=500, stop_max_attempt_number=30)
def wait_cordon_remove(node_name):
    assert is_cordoned(node_name) is False


# Retriable fucntions - END
