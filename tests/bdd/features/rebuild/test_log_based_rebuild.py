"""Partial Rebuild feature tests."""

from pytest_bdd import (
    given,
    scenario,
    then,
    when,
)

import pytest
import os
import subprocess
import time
from retrying import retry

from common.deployer import Deployer
from common.apiclient import ApiClient
from common.docker import Docker
from common.nvme import nvme_connect, nvme_disconnect
from time import sleep
from common.fio import Fio
from common.operations import Cluster

from openapi.model.create_pool_body import CreatePoolBody
from openapi.model.create_volume_body import CreateVolumeBody
from openapi.model.publish_volume_body import PublishVolumeBody
from openapi.model.protocol import Protocol
from openapi.model.volume_status import VolumeStatus
from openapi.model.volume_policy import VolumePolicy
from openapi.model.replica_state import ReplicaState
from openapi.model.child_state import ChildState
from openapi.model.child_state_reason import ChildStateReason

VOLUME_UUID = "5cd5378e-3f05-47f1-a830-a0f5873a1449"
NODE_1_NAME = "io-engine-1"
NODE_2_NAME = "io-engine-2"
NODE_3_NAME = "io-engine-3"
NODE_4_NAME = "io-engine-4"
POOL_1_UUID = "4cc6ee64-7232-497d-a26f-38284a444980"
POOL_2_UUID = "91a60318-bcfe-4e36-92cb-ddc7abf212ea"
POOL_3_UUID = "4d471e62-ca17-44d1-a6d3-8820f6156c1a"
POOL_4_UUID = "d5c5e3de-d77b-11ed-afa1-0242ac120002"
VOLUME_SIZE = 524288000
POOL_SIZE = 734003200
NUM_VOLUME_REPLICAS = 3
FAULTED_CHILD_WAIT_SECS = 2
FIO_RUN = 7
SLEEP_BEFORE_START = 1


@pytest.fixture(autouse=True, scope="module")
def init():
    Deployer.start(
        io_engines=4,
        node_agent=True,
        cluster_agent=True,
        cluster_agent_fast="100ms",
        agents_env="DETECTION_PERIOD=100ms,SUBSYS_REFRESH_PERIOD=100ms",
        csi_node=True,
        reconcile_period="300ms",
        faulted_child_wait_period=f"{FAULTED_CHILD_WAIT_SECS}s",
        cache_period="200ms",
        io_engine_coreisol=True,
    )
    yield
    Deployer.stop()


@pytest.fixture(autouse=True)
def init_scenario(init, disks):
    assert len(disks) == (NUM_VOLUME_REPLICAS + 1)
    # Only create 3 pools, so we can control where the initial replicas are placed.
    ApiClient.pools_api().put_node_pool(
        NODE_1_NAME, POOL_1_UUID, CreatePoolBody([f"aio://{disks[0]}"])
    )

    ApiClient.pools_api().put_node_pool(
        NODE_2_NAME, POOL_2_UUID, CreatePoolBody([f"aio://{disks[1]}"])
    )

    ApiClient.pools_api().put_node_pool(
        NODE_3_NAME, POOL_3_UUID, CreatePoolBody([f"aio://{disks[2]}"])
    )
    yield
    Cluster.cleanup(waitPools=True)


@pytest.fixture
def disks():
    pools = Deployer.create_disks(NUM_VOLUME_REPLICAS + 1, size=POOL_SIZE)
    yield pools
    Deployer.cleanup_disks(len(pools))


@scenario(
    "log-based-rebuild.feature",
    "Faulted child is not online again within timed-wait period",
)
def test_faulted_child_is_not_online_again_within_timedwait_period():
    """Faulted child is not online again within timed-wait period."""


@scenario(
    "log-based-rebuild.feature",
    "Faulted child is online again within timed-wait period",
)
def test_faulted_child_is_online_again_within_timedwait_period():
    """Faulted child is online again within timed-wait period."""


@scenario(
    "log-based-rebuild.feature",
    "Node goes down while log based rebuild running",
)
def test_node_goes_down_while_log_based_rebuild_running():
    """Node goes down while log based rebuild running."""


@scenario(
    "log-based-rebuild.feature",
    "Volume target is unreachable during log based rebuild",
)
def test_volume_target_is_unreachable_during_log_based_rebuild():
    """Volume target is unreachable during log based rebuild"""


@given("io-engine is installed and running")
def io_engine_is_installed_and_running():
    """io-engine is installed and running."""


@given("a volume with three replicas, filled with user data")
def a_volume_with_three_replicas_filled_with_user_data(disks):
    """a volume with three replicas, filled with user data."""
    request = CreateVolumeBody(
        VolumePolicy(True), NUM_VOLUME_REPLICAS, VOLUME_SIZE, False
    )
    ApiClient.volumes_api().put_volume(VOLUME_UUID, request)
    volume = ApiClient.volumes_api().put_volume_target(
        VOLUME_UUID,
        publish_volume_body=PublishVolumeBody(
            {}, Protocol("nvmf"), node=NODE_1_NAME, frontend_node="app-node-1"
        ),
    )

    # Now the volume has been created, create an additional pool.
    ApiClient.pools_api().put_node_pool(
        NODE_4_NAME, POOL_4_UUID, CreatePoolBody([f"aio://{disks[3]}"])
    )
    # Launch fio in background and let it run along with the test.
    uri = volume["state"]["target"]["deviceUri"]
    disk = nvme_connect(uri)
    fio = Fio(name="job", rw="randwrite", device=disk, runtime=FIO_RUN)
    fio = fio.open()
    yield
    try:
        code = fio.wait(timeout=FIO_RUN)
        assert code == 0, "FIO failed, exit code: %d" % code
    except subprocess.TimeoutExpired:
        assert False, "FIO timed out"
    nvme_disconnect(uri)


@when("a child becomes faulted")
def a_child_becomes_faulted():
    """a child becomes faulted."""
    # Fault a replica by stopping the container with the replica.
    # Check the replica becomes unhealthy by waiting for the volume to become degraded.
    Docker.stop_container(NODE_2_NAME)
    wait_for_degraded_volume()
    wait_child_faulted()


@retry(wait_fixed=100, stop_max_attempt_number=20)
def wait_child_faulted():
    vol = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    target = vol.state.target
    childlist = target["children"]
    pytest.faulted_child_uri = None
    for child in childlist:
        if ChildState(child["state"]) == ChildState("Faulted"):
            pytest.faulted_child_uri = child["uri"]
    assert pytest.faulted_child_uri is not None, "Failed to find Faulted child!"


@when("a non-local child becomes faulted")
def a_non_local_child_becomes_faulted():
    """a non-local child becomes faulted."""
    a_child_becomes_faulted()


@when("the replica is not online again within the timed-wait period")
def the_replica_is_not_online_again_within_the_timed_wait_period():
    """the replica is not online again within the timed-wait period."""
    # The replica shouldn't be available on this node still.
    time.sleep(FAULTED_CHILD_WAIT_SECS)
    replica = ApiClient.replicas_api().get_node_replicas(NODE_2_NAME)
    assert len(replica) == 0, "Replica not expected to be available on " + NODE_2_NAME


@when("the replica is online again within the timed-wait period")
def the_replica_is_online_again_within_the_timed_wait_period():
    """the replica is online again within the timed-wait period."""
    # this sleep would ensure some new writes to be in the log
    sleep(SLEEP_BEFORE_START)
    Docker.restart_container(NODE_2_NAME)
    # allow replica to be online after restart
    check_replica_online()


@retry(wait_fixed=200, stop_max_attempt_number=20)
def check_replica_online():
    replica = ApiClient.replicas_api().get_node_replicas(NODE_2_NAME)
    if len(replica) == 1:
        assert replica[0].state == ReplicaState("Online")
    else:
        assert False, "Expected a replica to be available again on " + NODE_2_NAME


@then("a full rebuild starts for unhealthy child")
@retry(wait_fixed=200, stop_max_attempt_number=20)
def a_full_rebuild_starts_for_unhealthy_child():
    """a full rebuild starts for unhealthy child"""
    vol = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    target = vol.state.target
    childlist = target["children"]
    assert len(childlist) == NUM_VOLUME_REPLICAS
    for child in childlist:
        if ChildState(child["state"]) == ChildState("Degraded"):
            assert child["uri"] != pytest.faulted_child_uri
            assert child["rebuildProgress"] != 0


@then("a log-based rebuild starts for the unhealthy child")
@retry(wait_fixed=200, stop_max_attempt_number=20)
def a_log_based_rebuild_starts_for_the_unhealthy_child():
    """a log-based rebuild starts for the unhealthy child"""
    # TODO: Check log-based rebuild is running.
    # No direct way to check partial rebuild via openapi yet
    vol = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    target = vol.state.target
    childlist = target["children"]
    assert len(childlist) == NUM_VOLUME_REPLICAS
    assert pytest.faulted_child_uri is not None
    for child in childlist:
        if child["uri"] == pytest.faulted_child_uri:
            assert ChildState(child["state"]) == ChildState("Degraded")


@then("a full rebuild starts before the timed-wait period")
def a_full_rebuild_starts_before_the_timedwait_period():
    """a full rebuild starts before the timed-wait period."""
    rebuild_ongoing_before_timed_wait()


@retry(wait_fixed=200, stop_max_attempt_number=20)
def rebuild_ongoing_before_timed_wait():
    vol = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    target = vol.state.target
    childlist = target["children"]
    assert len(childlist) == NUM_VOLUME_REPLICAS
    degraded = list(
        filter(
            lambda child: str(child["state"]) == "Degraded",
            childlist,
        )
    )
    print(degraded)
    assert len(degraded) is 1, "Should find one degraded child"


@then("the node hosting rebuilding replica crashes")
def the_node_hosting_rebuilding_replica_crashes():
    """the node hosting rebuilding replica crashes."""
    Docker.kill_container(NODE_2_NAME)


@then("a full rebuild starts for both of the unhealthy children")
def a_full_rebuild_starts_for_both_of_the_unhealthy_children():
    """a full rebuild starts for both of the unhealthy children"""
    verify_full_rebuilds()


@then("the volume target becomes unreachable along with its local child")
def the_volume_target_becomes_unreachable_along_with_its_local_child():
    """the volume target becomes unreachable along with its local child"""
    pytest.nexus_local_child = ApiClient.replicas_api().get_node_replicas(NODE_1_NAME)[
        0
    ]["uri"]
    Docker.stop_container(NODE_1_NAME)
    # wait for nexus republish
    verify_nexus_switchover()


@retry(wait_fixed=200, stop_max_attempt_number=80)
def verify_full_rebuilds():
    """a full rebuild starts for both of the unhealthy children"""
    children = []
    new_nexus = ApiClient.volumes_api().get_volume(VOLUME_UUID).state.target
    for child in new_nexus["children"]:
        children.append(child["uri"])
    assert pytest.nexus_local_child not in children

    rebuild_hist = ApiClient.volumes_api().get_rebuild_history(VOLUME_UUID)
    record = list(
        filter(
            lambda record: record.child_uri == pytest.faulted_child_uri,
            rebuild_hist.records,
        )
    )

    assert pytest.faulted_child_uri not in children or record[0].is_partial == False
    childs = new_nexus["children"]
    print(f"child list during full rebuild: {childs}")


@retry(wait_fixed=200, stop_max_attempt_number=60)
def verify_nexus_switchover():
    vol = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    new_target = vol["state"].get("target", None)
    if new_target is not None:
        assert NODE_1_NAME != new_target["node"]
    else:
        assert False, "target not yet failed over"


@retry(wait_fixed=200, stop_max_attempt_number=40)
def check_child_removal():
    vol = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    child_list = vol.state.target["children"]
    child_removed = True
    for child in child_list:
        if pytest.faulted_child_uri == child["uri"]:
            child_removed = False
            break
    assert (
        child_removed
    ), f"child list is {child_list}, faulted child : {pytest.faulted_child_uri}"


@retry(wait_fixed=200, stop_max_attempt_number=20)
def wait_for_degraded_volume():
    volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    assert volume.state.status == VolumeStatus("Degraded")
