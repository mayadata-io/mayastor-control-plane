"""Volume Snapshot listing feature tests."""

from pytest_bdd import given, scenario, then, when, parsers

import pytest
import pytest
import os
import datetime
from retrying import retry

import openapi.exceptions
from common.apiclient import ApiClient
from common.deployer import Deployer
from common.docker import Docker

from openapi.model.create_pool_body import CreatePoolBody
from openapi.model.create_volume_body import CreateVolumeBody
from openapi.model.node_status import NodeStatus
from openapi.model.spec_status import SpecStatus
from openapi.model.replica_state import ReplicaState
from openapi.exceptions import NotFoundException
from openapi.model.pool_status import PoolStatus
from openapi.model.protocol import Protocol
from openapi.model.publish_volume_body import PublishVolumeBody
from openapi.model.volume_policy import VolumePolicy


VOLUME1_UUID = "d01b8bfb-0116-47b0-a03a-447fcbdc0e99"
VOLUME2_UUID = "e01b8bfb-0116-47b0-a03a-447fcbdc0e99"
POOL_DISK1 = "pdisk1.img"
POOL1_NAME = "pool-1"
NODE1 = "io-engine-1"
VOLUME_SIZE = 1024 * 1024 * 64
VOLUME1_SNAP_IDS = [
    "1f49d30d-a446-4b40-b3f6-f439345f1ce9",
    "2f49d30d-a446-4b40-b3f6-f439345f1ce9",
    "3f49d30d-a446-4b40-b3f6-f439345f1ce9",
    "4f49d30d-a446-4b40-b3f6-f439345f1ce9",
]
VOLUME2_SNAP_IDS = [
    "1e49d30d-a446-4b40-b3f6-f439345f1ce9",
    "2e49d30d-a446-4b40-b3f6-f439345f1ce9",
    "3e49d30d-a446-4b40-b3f6-f439345f1ce9",
    "4e49d30d-a446-4b40-b3f6-f439345f1ce9",
]


@pytest.fixture
def listed_snapshots():
    return pytest.listed_snapshots


@pytest.fixture
def volume():
    return pytest.volume


@scenario("list.feature", "Paginated Volume snapshots list")
def test_paginated_volume_snapshots_list():
    """Paginated Volume snapshots list."""


@scenario("list.feature", "Paginated Volume snapshots list more than available")
def test_paginated_volume_snapshots_list_more_than_available():
    """Paginated Volume snapshots list more than available."""


@scenario("list.feature", "Volume snapshots list")
def test_volume_snapshots_list():
    """Volume snapshots list."""


@scenario("list.feature", "Volume snapshots list involving disrupted node")
def test_volume_snapshots_list_involving_disrupted_node():
    """Volume snapshots list involving disrupted node."""


@pytest.fixture(scope="module")
def create_pool_disk_images():
    pools = Deployer.create_disks(1, size=800 * 1024 * 1024)
    yield pools
    Deployer.cleanup_disks(len(pools))


@pytest.fixture(scope="module")
def setup(create_pool_disk_images):
    Deployer.start(
        io_engines=1,
        cache_period="250ms",
        reconcile_period="350ms",
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


@given(parsers.parse("we have a single replica {publish_status} volume"))
def we_have_a_single_replica_publish_status_volume(publish_status):
    """we have a single replica <publish_status> volume."""
    create_volume(1)
    yield
    del_volume()


@given("we have a single replica volume")
def we_have_a_single_replica_volume():
    """we have a single replica volume."""
    create_volume(1)
    yield
    del_volume()


@given("we have created more than one snapshots for each of the volumes")
def we_have_created_more_than_one_snapshots_for_each_of_the_volumes():
    """we have created more than one snapshots for each of the volumes."""
    create_snapshots_for_volume(VOLUME1_UUID, VOLUME1_SNAP_IDS)
    create_snapshots_for_volume(VOLUME2_UUID, VOLUME2_SNAP_IDS)
    yield
    for snapid in VOLUME1_SNAP_IDS + VOLUME2_SNAP_IDS:
        ApiClient.snapshots_api().del_snapshot(snapid)


@given("we have created more than one snapshot for the volume")
def we_have_created_more_than_one_snapshot_for_the_volume():
    """we have created more than one snapshot for the volume."""
    snap_id_list = VOLUME1_SNAP_IDS
    for snapid in snap_id_list:
        snap = ApiClient.snapshots_api().put_volume_snapshot(VOLUME1_UUID, snapid)
    yield
    for snapid in snap_id_list:
        ApiClient.snapshots_api().del_snapshot(snapid)


@given("we have more than one single replica volumes")
def we_have_more_than_one_single_replica_volumes():
    """we have more than one single replica volumes."""
    create_volume(1, VOLUME1_UUID)
    create_volume(1, VOLUME2_UUID)

    yield
    del_volume()


@when(parsers.parse("we {bring_down} the node hosting the snapshot"))
def we_bring_down_the_node_hosting_the_snapshot(volume, bring_down):
    """we <bring_down> the node hosting the snapshot."""
    if bring_down == "stop":
        Docker.stop_container(NODE1)
    if bring_down == "kill":
        Docker.kill_container(NODE1)
    wait_volume_replica_offline(volume)
    yield
    restart_node(NODE1)


@when("we bring up the node hosting the snapshot")
def we_bring_up_the_node_hosting_the_snapshot(volume):
    """we bring up the node hosting the snapshot."""
    restart_node(NODE1)


@then("the snapshots should be listed")
def the_snapshots_should_be_listed():
    """the snapshots should be listed."""
    try:
        snapshots = ApiClient.snapshots_api().get_volumes_snapshots(
            volume_id=VOLUME1_UUID, max_entries=0
        )
        pytest.listed_snapshots = snapshots
    except openapi.exceptions.ApiException:
        # Hitting this assert means test failed.
        assert False, "Error in listing snapshots"


@then("all of the snapshots should be listed for that volume")
def all_of_the_snapshots_should_be_listed_for_that_volume():
    """all of the snapshots should be listed for that volume."""
    total_fetched = len(pytest.listed_snapshots.entries)
    assert total_fetched == len(VOLUME2_SNAP_IDS)


@then("all the replica snapshots should be reported as online and validated")
def all_the_replica_snapshots_should_be_reported_as_online_and_validated(
    listed_snapshots, volume
):
    """all the replica snapshots should be reported as online and validated."""
    validate_snapshots(listed_snapshots, VOLUME1_SNAP_IDS, "online", volume)


@then("all the replica snapshots should be reported as offline and validated")
def all_the_replica_snapshots_should_be_reported_as_offline_and_validated(
    listed_snapshots, volume
):
    """all the replica snapshots should be reported as offline and validated."""
    validate_snapshots(listed_snapshots, VOLUME1_SNAP_IDS, "offline", volume)


@when("we try to list max_entries more than available snapshots for a volume")
def we_try_to_list_max_entries_more_than_available_snapshots_for_a_volume():
    """we try to list max_entries more than available snapshots for a volume."""
    try:
        # Try requesting twice of how many we created.
        snapshots = ApiClient.snapshots_api().get_volumes_snapshots(
            volume_id=VOLUME2_UUID,
            max_entries=2 * len(VOLUME2_SNAP_IDS),
            starting_token=0,
        )
        pytest.listed_snapshots = snapshots
    except openapi.exceptions.ApiException:
        # Hitting this assert means test failed.
        assert False, "Error in listing snapshots"


@then(parsers.parse("the snapshots should be listed by {filter}"))
def the_snapshots_should_be_listed_by_filter(filter):
    """the snapshots should be listed by <filter>."""
    try:
        if filter == "by_none":
            # fetch all snapshots.
            snapshots = ApiClient.snapshots_api().get_volumes_snapshots(max_entries=0)
            pytest.listed_snapshots = snapshots
        if filter == "by_volume":
            # fetch all snapshots for the volume.
            snapshots = ApiClient.snapshots_api().get_volumes_snapshots(
                volume_id=VOLUME1_UUID, max_entries=0
            )
            pytest.listed_snapshots = snapshots
        if filter == "by_volume_snap":
            # fetch a particular snapshot id for the volume.
            snapshot = ApiClient.snapshots_api().get_volume_snapshot(
                volume_id=VOLUME1_UUID, snapshot_id=VOLUME1_SNAP_IDS[0]
            )
            assert snapshot.state.uuid == VOLUME1_SNAP_IDS[0]
        if filter == "by_snap":
            # fetch a particular snapshot id.
            snapshot = ApiClient.snapshots_api().get_volumes_snapshot(
                VOLUME1_SNAP_IDS[0]
            )
            assert snapshot.state.uuid == VOLUME1_SNAP_IDS[0]
    except openapi.exceptions.ApiException:
        # Hitting this assert means test failed.
        assert False, "Expected snapshot to be found"


@then(parsers.parse("the snapshots should be listed in a paginated manner by {filter}"))
def the_snapshots_should_be_listed_in_a_paginated_manner_by_filter(filter):
    """the snapshots should be listed in a paginated manner by <filter>."""
    try:
        token = 0
        total_fetched = 0
        if filter == "by_none":
            # Fetch paginated - all volume snapshots
            while token is not None:
                snapshots = ApiClient.snapshots_api().get_volumes_snapshots(
                    max_entries=1, starting_token=token
                )
                assert len(snapshots.entries) == 1
                total_fetched += len(snapshots.entries)
                if hasattr(snapshots, "next_token"):
                    token = snapshots.next_token
                else:
                    break
            assert total_fetched == len(VOLUME1_SNAP_IDS + VOLUME2_SNAP_IDS)

        if filter == "by_volume":
            # Fetch paginated - for a specfic volume
            while token is not None:
                snapshots = ApiClient.snapshots_api().get_volumes_snapshots(
                    volume_id=VOLUME2_UUID, max_entries=1, starting_token=token
                )
                total_fetched += len(snapshots.entries)
                if hasattr(snapshots, "next_token"):
                    token = snapshots.next_token
                else:
                    break
            assert total_fetched == len(VOLUME2_SNAP_IDS)
    except openapi.exceptions.ApiException:
        # Hitting this assert means test failed.
        assert False, "Error in listing snapshots"


def validate_snapshots(listed_snapshots, ref_snaps_list, state, volume):
    """Validate the snapshots"""
    assert len(listed_snapshots.entries) == len(ref_snaps_list)
    for snapshot in listed_snapshots.entries:
        try:
            idx = VOLUME1_SNAP_IDS.index(snapshot.state.uuid)
            # Since there are multiple snapshots created for volume, and
            # we are not writing anything to the volume, only first snapshot
            # will have size equal to volume size.
            if idx == 0 and hasattr(snapshot, "size"):
                assert snapshot.state.size == volume.spec.size
            assert snapshot.state.ready_as_source is False
            timestamp_within(snapshot.state.timestamp)
            wait_snapshot_state(snapshot, state)
        except ValueError as ve:
            assert False, "Invalid snapshot id found: " + snapshot.state.uuid


def create_volume(replicas=1, volume_id="", publish_status="", replica_location=""):
    if volume_id == "":
        volume_id = VOLUME1_UUID

    volume = ApiClient.volumes_api().put_volume(
        volume_id,
        CreateVolumeBody(
            VolumePolicy(True),
            replicas=replicas,
            size=VOLUME_SIZE,
            thin=False,
        ),
    )

    if publish_status == "published":
        node = NODE1
        ApiClient.volumes_api().put_volume_target(
            volume_id,
            publish_volume_body=PublishVolumeBody({}, Protocol("nvmf"), node=node),
        )

    pytest.volume = volume
    return volume


def create_snapshots_for_volume(volume_id, snap_id_list):
    for snapid in snap_id_list:
        ApiClient.snapshots_api().put_volume_snapshot(volume_id, snapid)


def restart_node(name):
    Docker.restart_container(name)
    wait_node_ready(name)


@retry(wait_fixed=100, stop_max_attempt_number=20)
def wait_node_ready(name):
    node = ApiClient.nodes_api().get_node(name)
    assert node.state.status == NodeStatus("Online")
    for pool in ApiClient.pools_api().get_node_pools(name):
        assert pool.state.status == PoolStatus("Online")


@retry(wait_fixed=100, stop_max_attempt_number=20)
def wait_volume_replica_offline(volume):
    volume = ApiClient.volumes_api().get_volume(volume.spec.uuid)
    replicas = list(volume.state.replica_topology.values())
    assert len(replicas) == 1
    assert replicas[0].state == ReplicaState("Unknown")


@retry(wait_fixed=100, stop_max_attempt_number=20)
def wait_snapshot_state(snapshot, state):
    snapshot = ApiClient.snapshots_api().get_volumes_snapshot(snapshot.state.uuid)
    assert len(snapshot.state.replica_snapshots) == 1
    replica_snapshot = snapshot.state.replica_snapshots[0]
    assert hasattr(replica_snapshot, state)


def timestamp_within(timestamp, minutes=1):
    now = datetime.datetime.utcnow().replace(tzinfo=datetime.timezone.utc)
    if timestamp > now:
        pytest.fail("Timestamp is in the future?")
    age = now - timestamp
    if age > datetime.timedelta(minutes=minutes):
        pytest.fail("Timestamp is older than 1 minute")


def del_volume():
    for vol in [VOLUME1_UUID, VOLUME2_UUID]:
        try:
            ApiClient.volumes_api().del_volume(vol)
        except NotFoundException:
            pass
