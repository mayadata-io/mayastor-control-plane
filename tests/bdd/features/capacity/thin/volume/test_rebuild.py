"""Thin Provisioning - Volume Rebuild feature tests."""
import time

from retrying import retry
from urllib.parse import urlparse
import pytest
import subprocess

from pytest_bdd import (
    given,
    scenario,
    then,
    when,
)

from common.deployer import Deployer
from common.apiclient import ApiClient
from common.fio import Fio

from openapi.model.create_pool_body import CreatePoolBody
from openapi.model.create_volume_body import CreateVolumeBody
from openapi.model.publish_volume_body import PublishVolumeBody
from openapi.model.protocol import Protocol
from openapi.model.volume_status import VolumeStatus
from openapi.model.volume_policy import VolumePolicy
from openapi.model.create_replica_body import CreateReplicaBody

VOLUME_UUID = "ec4e66fd-3b33-4439-b504-d49aba53da26"
VOLUME_SIZE = 20 * 1024 * 1024
POOL_SIZE = VOLUME_SIZE + 8 * 1024 * 1024
NUM_VOLUME_REPLICAS = 1
CREATE_REQUEST_KEY = "create_request"
POOL_UUID_1 = "4cc6ee64-7232-497d-a26f-38284a444980"
POOL_UUID_2 = "4cc6ee64-7232-497d-a26f-38284a444981"
POOL_UUID_3 = "4cc6ee64-7232-497d-a26f-38284a444982"
REPL_UUID = "4cc6ee64-7232-497d-a26f-38284a444981"
NODE_NAME_1 = "io-engine-1"
NODE_NAME_2 = "io-engine-2"
NODE_NAME_3 = "io-engine-3"


# This fixture will be automatically used by all tests.
# It starts the deployer which launches all the necessary containers.
# A pool is created for convenience such that it is available for use by the tests.
@pytest.fixture(autouse=True)
def init():
    Deployer.start(
        io_engines=2,
        cache_period="250ms",
        reconcile_period="500ms",
        fio_spdk=True,
        io_engine_coreisol=True,
    )
    pool_size_mb = int(POOL_SIZE / (1024 * 1024))
    ApiClient.pools_api().put_node_pool(
        NODE_NAME_1,
        POOL_UUID_1,
        CreatePoolBody([f"malloc:///disk1?size_mb={pool_size_mb}"]),
    )
    ApiClient.pools_api().put_node_pool(
        NODE_NAME_2,
        POOL_UUID_2,
        CreatePoolBody([f"malloc:///disk2?size_mb={pool_size_mb}"]),
    )

    yield
    Deployer.stop()


@scenario("rebuild.feature", "Replicating a thin volume")
def test_replicating_a_thin_volume():
    """Replicating a thin volume."""


@scenario(
    "rebuild.feature",
    "Resolving out-of-space condition for a thin multi-replica volume",
)
def test_resolving_outofspace_condition_for_a_thin_multireplica_volume():
    """Resolving out-of-space condition for a thin multi-replica volume."""


@scenario("rebuild.feature", "Rebuild is not started if there's no space in the pool")
def test_rebuild_is_not_started_if_theres_no_space_in_the_pool():
    """Rebuild is not started if there's no space in the pool."""


@given("a control plane, Io-Engine instances and pools")
def a_control_plane_ioengine_instances_and_pools(init):
    """a control plane, Io-Engine instances and pools."""


@given("a thin provisioned volume with 1 replica")
def a_thin_provisioned_volume_with_1_replica():
    """a thin provisioned volume with 1 replica."""
    volume = ApiClient.volumes_api().put_volume(
        VOLUME_UUID,
        create_volume_body=CreateVolumeBody(VolumePolicy(True), 1, VOLUME_SIZE, True),
    )
    volume = ApiClient.volumes_api().put_volume_target(
        volume.spec.uuid,
        publish_volume_body=PublishVolumeBody({}, Protocol("nvmf")),
    )
    assert hasattr(volume.spec, "target")
    assert str(volume.spec.target.protocol) == str(Protocol("nvmf"))
    pytest.volume = volume


@given("a thin provisioned volume with 2 replicas")
def a_thin_provisioned_volume_with_2_replicas():
    """a thin provisioned volume with 2 replicas."""
    volume = ApiClient.volumes_api().put_volume(
        VOLUME_UUID,
        create_volume_body=CreateVolumeBody(VolumePolicy(True), 2, VOLUME_SIZE, True),
    )

    # Ensure it's the first replica that gets faulted with enospc to work around a datapath bug: GTM-609.
    # Once fixed we can set it to None.
    node = NODE_NAME_1
    volume = ApiClient.volumes_api().put_volume_target(
        volume.spec.uuid,
        publish_volume_body=PublishVolumeBody({}, Protocol("nvmf"), node=node),
    )
    assert hasattr(volume.spec, "target")
    assert str(volume.spec.target.protocol) == str(Protocol("nvmf"))
    pytest.volume = volume


@when("data is written to the volume")
def data_is_written_to_the_volume():
    """data is written to the volume."""
    volume = pytest.volume
    uri = urlparse(volume.state.target["deviceUri"])

    fio = Fio(name="job", rw="write", uri=uri, size="10M")

    code = fio.run().returncode
    assert code == 0, "Fio is expected to execute successfully"


@when("data is written to the volume which exceeds the free space on one of the pools")
def data_is_written_to_the_volume_which_exceeds_the_free_space_on_one_of_the_pools():
    """data is written to the volume which exceeds the free space on one of the pools."""

    # Fill up a pool with other data
    ApiClient.replicas_api().put_pool_replica(
        POOL_UUID_1, REPL_UUID, CreateReplicaBody(size=int(VOLUME_SIZE / 2), thin=False)
    )

    volume = pytest.volume
    uri = urlparse(volume.state.target["deviceUri"])

    fio = Fio(name="job", rw="write", uri=uri)
    pytest.fio = fio.open()


@when("the new replica is fully rebuilt")
def the_new_replica_is_fully_rebuilt():
    """the new replica is fully rebuilt."""
    wait_volume_rebuild(pytest.volume)


@when("the number of replicas is increased")
def the_number_of_replicas_is_increased():
    """the number of replicas is increased."""
    ApiClient.volumes_api().put_volume_replica_count(
        VOLUME_UUID, NUM_VOLUME_REPLICAS + 1
    )


@then("a replica is reported as faulted due to out-of-space")
def a_replica_is_reported_as_faulted_due_to_outofspace():
    """a replica is reported as faulted due to out-of-space."""
    volume = ApiClient.volumes_api().get_volume(pytest.volume.spec.uuid)
    wait_volume_enospc(volume)


@then("the client app is not affected because other healthy replicas exists")
def the_client_app_is_not_affected_because_other_healthy_replicas_exists():
    """the client app is not affected because other healthy replicas exists."""
    fio = pytest.fio

    try:
        code = fio.wait(timeout=30)
        assert code == 0, "FIO failed, exit code: %d" % code
    except subprocess.TimeoutExpired:
        assert False, "FIO timed out"


@then("the faulted replica is relocated to another pool with sufficient free space")
def the_faulted_replica_is_relocated_to_another_pool_with_sufficient_free_space():
    """the faulted replica is relocated to another pool with sufficient free space."""

    volume = pytest.volume
    replicas = volume.state.replica_topology.keys()

    # Make free capacity available on another pool
    pool_size_mb = int(POOL_SIZE / (1024 * 1024))
    ApiClient.pools_api().put_node_pool(
        NODE_NAME_1,
        POOL_UUID_3,
        CreatePoolBody([f"malloc:///disk22?size_mb={pool_size_mb}"]),
    )

    wait_volume_new_replica(volume, replicas)


@then("the new replica allocation equals the volume allocation")
def the_new_replica_allocation_equals_the_volume_allocation():
    """the new replica allocation equals the volume allocation."""
    wait_volume_replica_allocated(pytest.volume)


@then("the total number of healthy replicas is restored")
def the_total_number_of_healthy_replicas_is_restored():
    """the total number of healthy replicas is restored."""
    volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    assert volume.state.status == VolumeStatus("Online")
    assert len(volume.state.target["children"]) == 2


@then("the total number of healthy replicas of the volume decreases by one")
def the_total_number_of_healthy_replicas_of_the_volume_decreases_by_one():
    """the total number of healthy replicas of the volume decreases by one."""
    volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    wait_volume_degraded(volume)


@then("if the volume is republished again")
def if_the_volume_is_republished_again():
    """if the volume is republished again."""
    ApiClient.volumes_api().del_volume_target(VOLUME_UUID)
    volume = ApiClient.volumes_api().put_volume_target(
        VOLUME_UUID,
        publish_volume_body=PublishVolumeBody({}, Protocol("nvmf")),
    )
    assert hasattr(volume.spec, "target")
    assert str(volume.spec.target.protocol) == str(Protocol("nvmf"))
    pytest.volume = volume


@then("the faulted replica should be deleted")
def the_faulted_replica_should_be_deleted():
    """the faulted replica should be deleted."""
    wait_unrebuildable_removed(pytest.volume, True)


@then("the faulted replica should not be rebuilt as there's no free space in the pool")
def the_faulted_replica_should_not_be_rebuilt_as_theres_no_free_space_in_the_pool():
    """the faulted replica should not be rebuilt as there's no free space in the pool."""
    wait_unrebuildable_removed(pytest.volume, False)


@retry(wait_fixed=200, stop_max_attempt_number=20)
def wait_unrebuildable_removed(volume, deleted):
    volume = ApiClient.volumes_api().get_volume(volume.spec.uuid)
    assert volume.state.status == VolumeStatus("Degraded")
    assert len(volume.state.target["children"]) == 1, "We should not add the Child"
    assert not deleted or len(volume.state.replica_topology.keys()) == 1


@retry(wait_fixed=200, stop_max_attempt_number=20)
def wait_volume_rebuild(volume):
    volume = ApiClient.volumes_api().get_volume(volume.spec.uuid)
    assert volume.state.status == VolumeStatus("Online")
    assert len(volume.state.target["children"]) == 2


@retry(wait_fixed=200, stop_max_attempt_number=20)
def wait_volume_enospc(volume):
    volume = ApiClient.volumes_api().get_volume(volume.spec.uuid)
    assert volume.state.status == VolumeStatus("Degraded")
    assert len(volume.state.target["children"]) == 2

    enospcs = list(
        filter(
            lambda child: child.get("state_reason") == "OutOfSpace",
            list(volume.state.target["children"]),
        )
    )
    assert len(enospcs) == 1


@retry(wait_fixed=200, stop_max_attempt_number=20)
def wait_volume_degraded(volume):
    volume = ApiClient.volumes_api().get_volume(volume.spec.uuid)
    assert volume.state.status == VolumeStatus("Degraded")


@retry(wait_fixed=200, stop_max_attempt_number=20)
def wait_volume_new_replica(volume, prev_replicas):
    volume = ApiClient.volumes_api().get_volume(volume.spec.uuid)
    new_replicas = list(
        filter(
            lambda uuid: uuid not in prev_replicas,
            list(volume.state.replica_topology),
        )
    )
    assert len(new_replicas) == 1


@retry(wait_fixed=200, stop_max_attempt_number=20)
def wait_volume_replica_allocated(volume):
    volume = ApiClient.volumes_api().get_volume(volume.spec.uuid)
    total_allocated = 0
    for replica in volume.state.replica_topology.values():
        assert replica.usage.capacity == volume.state.usage.capacity
        assert replica.usage.allocated == volume.state.usage.allocated
        total_allocated += replica.usage.allocated

    assert volume.state.usage.total_allocated == total_allocated
    assert volume.state.usage.allocated > 10 * 1024 * 1024 + 4 * 1024 * 1024
