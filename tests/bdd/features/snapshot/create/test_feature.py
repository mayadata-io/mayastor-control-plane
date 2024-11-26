"""Volume Snapshot Creation feature tests."""

from pytest_bdd import given, scenario, then, when, parsers

import pytest
import os
import datetime
from retrying import retry

from common.deployer import Deployer
from common.apiclient import ApiClient
from common.docker import Docker
from common.operations import Cluster

from openapi.model.create_pool_body import CreatePoolBody
from openapi.model.create_volume_body import CreateVolumeBody
from openapi.model.spec_status import SpecStatus
from openapi.model.volume_policy import VolumePolicy
from openapi.model.volume_share_protocol import VolumeShareProtocol
from openapi.model.publish_volume_body import PublishVolumeBody
from openapi.model.replica_state import ReplicaState
from openapi.model.pool_status import PoolStatus
from openapi.model.node_status import NodeStatus
from openapi.exceptions import NotFoundException
import openapi.exceptions

VOLUME_UUID = "5cd5378e-3f05-47f1-a830-a0f5873a1449"
SNAP_UUID = "107ec5c6-879d-4ca5-bc8c-c1948b454ac0"
NODE_NAME = "io-engine-1"
REMOTE_NODE_NAME = "io-engine-2"
VOLUME_CTX_KEY = "volume"
REMOTE_POOL_NAME = "remote_pool_1"
VOLUME_SIZE = 12582912
POOL_SIZE = 200 * 1024 * 1024


@pytest.fixture(scope="module")
def disks():
    pools = Deployer.create_disks(2, size=POOL_SIZE)
    yield pools
    Deployer.cleanup_disks(len(pools))


@pytest.fixture(scope="module")
def deployer_cluster(disks):
    Deployer.start(
        2,
        cache_period="150ms",
        reconcile_period="150ms",
        request_timeout="1s",
        no_min_timeouts=True,
    )
    ApiClient.pools_api().put_node_pool(NODE_NAME, "pool-1", CreatePoolBody([disks[0]]))
    yield
    Deployer.stop()


@scenario("feature.feature", "Existence of snapshot after node restart")
def test_existence_of_snapshot_after_node_restart():
    """Existence of snapshot after node restart."""


@scenario(
    "feature.feature", "Retrying snapshot creation after source is deleted should fail"
)
def test_retrying_snapshot_creation_after_source_is_deleted_should_fail():
    """Retrying snapshot creation after source is deleted should fail."""


@scenario("feature.feature", "Retrying snapshot creation until it succeeds")
def test_retrying_snapshot_creation_until_it_succeeds():
    """Retrying snapshot creation until it succeeds."""


@scenario(
    "feature.feature",
    "Snapshot creation for a single replica volume fails when the replica is offline",
)
def test_snapshot_creation_for_a_single_replica_volume_fails_when_the_replica_is_offline():
    """Snapshot creation for a single replica volume fails when the replica is offline."""


@scenario("feature.feature", "Snapshot creation for multi-replica volume")
def test_snapshot_creation_for_multireplica_volume():
    """Snapshot creation for multi-replica volume."""


@scenario("feature.feature", "Snapshot creation of a single replica volume")
def test_snapshot_creation_of_a_single_replica_volume():
    """Snapshot creation of a single replica volume."""


@scenario(
    "feature.feature",
    "Snapshot creation of a single replica volume after its source is deleted",
)
def test_snapshot_creation_of_a_single_replica_volume_after_its_source_is_deleted():
    """Snapshot creation of a single replica volume after its source is deleted."""


@scenario("feature.feature", "Subsequent creation with same snapshot id should fail")
def test_subsequent_creation_with_same_snapshot_id_should_fail():
    """Subsequent creation with same snapshot id should fail."""


@scenario("feature.feature", "Multi-replica snapshot with one node down")
def test_multi_replica_snapshot_with_one_node_down():
    """Multi-replica snapshot with one node down."""


@given("a deployer cluster")
def a_deployer_cluster(deployer_cluster):
    """a deployer cluster."""


@given("a successfully created snapshot")
def a_successfully_created_snapshot(volume):
    """a successfully created snapshot."""
    put_snapshot(volume)
    yield
    if Cluster.fixture_cleanup():
        del_snapshot()


@given("the node A is paused")
def the_node_a_is_paused(node_a):
    """the node A is paused."""
    Docker.pause_container(node_a)


@then("the node A is stopped")
def the_node_a_is_stopped(node_a):
    """the node A is stopped."""
    Docker.stop_container(node_a)


@given("the replica is offline")
def the_replica_is_offline(volume, node_a):
    """the replica is offline."""
    Docker.stop_container(node_a)
    wait_volume_replica_offline(volume)
    yield
    restart_node(node_a)


@given("the volume is published on node A")
def the_volume_is_published_on_node_a(node_a, volume):
    """the volume is published on node A."""
    ApiClient.volumes_api().put_volume_target(
        volume.spec.uuid,
        publish_volume_body=PublishVolumeBody(
            {}, VolumeShareProtocol("nvmf"), node=node_a
        ),
    )


@given("we have a multi-replica volume")
def we_have_a_multireplica_volume(disks):
    """we have a multi-replica volume."""
    ApiClient.pools_api().put_node_pool(
        REMOTE_NODE_NAME, REMOTE_POOL_NAME, CreatePoolBody([disks[1]])
    )
    put_volume(2)
    yield
    if Cluster.fixture_cleanup():
        del_volume()
        del_remote_pool()


@given(
    parsers.parse(
        "we have a single replica {publish_status} volume where the replica is {replica_location} to the target"
    )
)
def we_have_a_single_replica_publish_status_volume_where_the_replica_is_replica_location_to_the_target(
    publish_status, replica_location
):
    """we have a single replica <publish_status> volume where the replica is <replica_location> to the target."""
    put_volume(1, publish_status, replica_location)
    yield
    if Cluster.fixture_cleanup():
        del_volume()


@given("we have a single replica volume")
def we_have_a_single_replica_volume():
    """we have a single replica volume."""
    put_volume(1)
    yield
    if Cluster.fixture_cleanup():
        del_volume()


@given("we have a single replica volume with replica on node A")
def we_have_a_single_replica_volume_with_replica_on_node_a():
    """we have a single replica volume with replica on node A."""
    put_volume(1, "published", "remote")
    yield
    if Cluster.fixture_cleanup():
        del_volume()


@when("a snapshot is created for the volume")
def a_snapshot_is_created_for_the_volume(volume):
    """a snapshot is created for the volume."""
    put_snapshot(volume)
    yield
    if Cluster.fixture_cleanup():
        del_snapshot()


@when("the node A is brought back")
def the_node_a_is_brought_back(node_a):
    """the node A is brought back."""
    restart_node(node_a)


@when("the node B is brought back")
def the_node_b_is_brought_back(node_b):
    """the node B is brought back."""
    restart_node(node_b)


@when("the volume is deleted")
def the_volume_is_deleted():
    """the volume is deleted."""
    del_volume()


@when(parsers.parse("we {bring_down} the node hosting the snapshot"))
def we_bring_down_the_node_hosting_the_snapshot(bring_down, snapshot_node):
    """we <bring_down> the node hosting the snapshot."""
    if bring_down == "stop":
        Docker.stop_container(snapshot_node)
    if bring_down == "kill":
        Docker.kill_container(snapshot_node)
    if bring_down == "bring up":
        restart_node(snapshot_node)
    yield
    if bring_down != "bring up":
        restart_node(snapshot_node)


@then("we delete the source volume")
def we_delete_the_source_volume(volume):
    """we delete the source volume."""
    del_volume()


@when("we retry the same snapshot creation")
@then("we retry the same snapshot creation")
def we_retry_the_same_snapshot_creation(snapshot_create_request):
    """we retry the same snapshot creation."""
    snapshot_create_attempt(snapshot_create_request)


@when("we try to create a snapshot for the volume")
def we_try_to_create_a_snapshot_for_the_volume(snapshot_create_request):
    """we try to create a snapshot for the volume."""
    snapshot_create_attempt(snapshot_create_request)


@then("eventually the snapshot should be deleted")
def eventually_the_snapshot_should_be_deleted():
    """eventually the snapshot should be deleted."""
    wait_snapshot_deleted()


@then("it should fail with already exists")
def it_should_fail_with_already_exists():
    """it should fail with already exists."""
    assert pytest.exception is not None
    assert pytest.exception.status == 422


@then("the replica snapshot should be reported as offline")
def the_replica_snapshot_should_be_reported_as_offline(snapshot):
    """the replica snapshot should be reported as offline."""
    wait_snapshot_state(snapshot, "offline")


@then("the replica snapshot should be reported as online")
def the_replica_snapshot_should_be_reported_as_online(snapshot):
    """the replica snapshot should be reported as online."""
    wait_snapshot_state(snapshot, "online")


@then("the replica snapshot's timestamp should be in the RFC 3339 format")
def the_replica_snapshots_timestamp_should_be_in_the_rfc_3339_format(snapshot):
    """the replica snapshot's timestamp should be in the RFC 3339 format."""


@then("the replica snapshot's timestamp should be within 1 minute of creation (UTC)")
def the_replica_snapshots_timestamp_should_be_within_1_minute_of_creation_utc(snapshot):
    """the replica snapshot's timestamp should be within 1 minute of creation (UTC)."""
    replica_snapshot = snapshot.state.replica_snapshots[0]
    timestamp_within(replica_snapshot["online"].timestamp)


@then("the snapshot creation should be fail with already exists")
def the_snapshot_creation_should_be_fail_with_already_exists():
    """the snapshot creation should be fail with already exists."""
    assert pytest.exception is not None
    assert pytest.exception.status == 422


@then("the snapshot creation should be fail with preconditions failed")
def the_snapshot_creation_should_be_fail_with_preconditions_failed():
    """the snapshot creation should be fail with preconditions failed."""
    assert pytest.exception is not None
    assert pytest.exception.status == 412


@then("the snapshot creation should be fail with invalid arguments")
def the_snapshot_creation_should_be_fail_with_preconditions_failed():
    """the snapshot creation should be fail with invalid arguments."""
    assert pytest.exception is not None
    assert pytest.exception.status == 400


@then("the snapshot creation should be successful")
def the_snapshot_creation_should_be_successful(snapshot):
    """the snapshot creation should be successful."""
    assert pytest.exception is None
    assert snapshot.definition.spec.uuid == SNAP_UUID
    assert snapshot.definition.metadata.status == SpecStatus("Created")
    yield
    del_snapshot()


@then("the snapshot creation should fail")
def the_snapshot_creation_should_fail():
    """the snapshot creation should fail."""
    assert pytest.exception is not None


@then("the snapshot creation should fail with failed preconditions")
def the_snapshot_creation_should_fail_with_failed_preconditions():
    """the snapshot creation should fail with failed preconditions."""
    assert pytest.exception is not None
    assert pytest.exception.status == 412


@then("the snapshot creation should fail with preconditions failed")
def the_snapshot_creation_should_fail_with_preconditions_failed():
    """the snapshot creation should fail with preconditions failed."""
    assert pytest.exception is not None
    assert pytest.exception.status == 412


@then("the snapshot should not be listed")
def the_snapshot_should_not_be_listed():
    """the snapshot should not be listed."""
    with pytest.raises(NotFoundException) as e:
        ApiClient.snapshots_api().get_volumes_snapshot(SNAP_UUID)


@then("the snapshot size should be equal to the volume size")
def the_snapshot_size_should_be_equal_to_the_volume_size(volume, snapshot):
    """the snapshot size should be equal to the volume size."""
    assert snapshot.state.allocated_size == volume.spec.size


@then("the snapshot source size should be equal to the volume size")
def the_snapshot_source_size_should_be_equal_to_the_volume_size(volume, snapshot):
    """the snapshot source size should be equal to the volume size."""
    assert snapshot.state.allocated_size == volume.spec.size


@then("the snapshot status ready as source should be false")
def the_snapshot_status_ready_as_source_should_be_false(snapshot):
    """the snapshot status ready as source should be false."""
    assert snapshot.state.ready_as_source is False


@then("the snapshot timestamp should be in the RFC 3339 format")
def the_snapshot_timestamp_should_be_in_the_rfc_3339_format(snapshot):
    """the snapshot timestamp should be in the RFC 3339 format."""
    # TODO: check that it's RFC 3339, this may already be validated by the python openapi code
    print(snapshot.state.timestamp)
    print(snapshot.definition.metadata.timestamp)


@then("the snapshot timestamp should be within 1 minute of creation (UTC)")
def the_snapshot_timestamp_should_be_within_1_minute_of_creation_utc(snapshot):
    """the snapshot timestamp should be within 1 minute of creation (UTC)."""
    timestamp_within(snapshot.state.timestamp)


@then("the volume snapshot state has a single online replica snapshot")
def the_volume_snapshot_state_has_a_single_online_replica_snapshot(snapshot):
    """the volume snapshot state has a single online replica snapshot."""
    assert len(snapshot.state.replica_snapshots) == 1
    replica_snapshot = snapshot.state.replica_snapshots[0]
    assert hasattr(replica_snapshot, "online")


@then("the volume snapshot state has multiple online replica snapshots")
def the_volume_snapshot_state_has_multiple_online_replica_snapshots(snapshot):
    """the volume snapshot state has multiple online replica snapshots."""
    assert len(snapshot.state.replica_snapshots) == 2
    replica_snapshot = snapshot.state.replica_snapshots[0]
    assert hasattr(replica_snapshot, "online")
    replica_snapshot = snapshot.state.replica_snapshots[1]
    assert hasattr(replica_snapshot, "online")


###


@pytest.fixture
def node_a():
    return NODE_NAME


@pytest.fixture
def node_b():
    return REMOTE_NODE_NAME


@pytest.fixture
def volume():
    return pytest.volume


def del_volume():
    try:
        ApiClient.volumes_api().del_volume(VOLUME_UUID)
    except NotFoundException:
        pass


def del_remote_pool():
    try:
        ApiClient.pools_api().del_pool(REMOTE_POOL_NAME)
    except NotFoundException:
        pass


def put_volume(replicas=1, publish_status="", replica_location=""):
    volume = ApiClient.volumes_api().put_volume(
        VOLUME_UUID,
        CreateVolumeBody(
            VolumePolicy(True),
            replicas=replicas,
            size=VOLUME_SIZE,
            thin=False,
        ),
    )

    if publish_status == "published":
        node = NODE_NAME
        if replica_location == "remote":
            node = REMOTE_NODE_NAME

        ApiClient.volumes_api().put_volume_target(
            VOLUME_UUID,
            publish_volume_body=PublishVolumeBody(
                {}, VolumeShareProtocol("nvmf"), node=node
            ),
        )

    pytest.volume = volume
    return volume


@pytest.fixture
def snapshot_create_request(volume):
    def request():
        snapshot = ApiClient.snapshots_api().put_volume_snapshot(
            volume.spec.uuid, SNAP_UUID
        )
        print(snapshot)
        return snapshot

    yield request


def snapshot_create_attempt(snapshot_create_request):
    pytest.exception = None
    pytest.snapshot = None
    try:
        pytest.snapshot = snapshot_create_request()
    except openapi.exceptions.ApiException as e:
        pytest.exception = e


@pytest.fixture
def snapshot():
    return pytest.snapshot


def put_snapshot(volume):
    snapshot = ApiClient.snapshots_api().put_volume_snapshot(
        volume.spec.uuid, SNAP_UUID
    )
    pytest.snapshot = snapshot
    return snapshot


def del_snapshot():
    try:
        ApiClient.snapshots_api().del_snapshot(SNAP_UUID)
    except NotFoundException:
        pass


@pytest.fixture
def snapshot_node(snapshot):
    print(snapshot)
    return NODE_NAME


@retry(wait_fixed=100, stop_max_attempt_number=20)
def wait_snapshot_deleted():
    with pytest.raises(NotFoundException) as e:
        ApiClient.snapshots_api().get_volumes_snapshot(SNAP_UUID)


@retry(wait_fixed=100, stop_max_attempt_number=20)
def wait_snapshot_state(snapshot, state):
    snapshot = ApiClient.snapshots_api().get_volumes_snapshot(snapshot.state.uuid)
    assert len(snapshot.state.replica_snapshots) == 1
    replica_snapshot = snapshot.state.replica_snapshots[0]
    assert hasattr(replica_snapshot, state), f"state: {state}"


@retry(wait_fixed=100, stop_max_attempt_number=20)
def wait_volume_replica_offline(volume):
    volume = ApiClient.volumes_api().get_volume(volume.spec.uuid)
    replicas = list(volume.state.replica_topology.values())
    assert len(replicas) == 1
    assert replicas[0].state == ReplicaState("Unknown")


def restart_node(name):
    Docker.restart_container(name)
    wait_node_ready(name)


@retry(wait_fixed=100, stop_max_attempt_number=20)
def wait_node_ready(name):
    node = ApiClient.nodes_api().get_node(name)
    assert node.state.status == NodeStatus("Online")
    for pool in ApiClient.pools_api().get_node_pools(name):
        assert pool.state.status == PoolStatus("Online")


def timestamp_within(timestamp, minutes=1):
    now = datetime.datetime.utcnow().replace(tzinfo=datetime.timezone.utc)
    if timestamp > now:
        pytest.fail("Timestamp is in the future?")
    age = now - timestamp
    if age > datetime.timedelta(minutes=minutes):
        pytest.fail("Timestamp is older than 1 minute")
