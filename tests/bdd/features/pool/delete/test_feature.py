"""Pool deletion feature tests."""

import http
import os
import pytest

from pytest_bdd import (
    given,
    scenario,
    then,
    when,
)
from retrying import retry

from common.deployer import Deployer
from common.apiclient import ApiClient
from common.docker import Docker
from openapi.model.create_pool_body import CreatePoolBody
from openapi.model.create_volume_body import CreateVolumeBody
from openapi.model.node_status import NodeStatus
from openapi.exceptions import ApiException
from openapi.exceptions import NotFoundException
from openapi.model.volume_policy import VolumePolicy


@scenario("feature.feature", "deleting a non-existing pool")
def test_deleting_a_nonexisting_pool():
    """deleting a non-existing pool."""


@scenario("feature.feature", "deleting a pool that is in use")
def test_deleting_a_pool_that_is_in_use():
    """deleting a pool that is in use."""


@scenario(
    "feature.feature",
    "deleting an existing pool on an unreachable node that is still marked online",
)
def test_deleting_an_existing_pool_on_an_unreachable_node_that_is_still_marked_online():
    """deleting an existing pool on an unreachable node."""


@scenario("feature.feature", "deleting an existing pool on an unreachable offline node")
def test_deleting_an_existing_pool_on_an_unreachable_offline_node():
    """deleting an existing pool on an unreachable offline node."""


@scenario("feature.feature", "deleting an existing unused pool")
def test_deleting_an_existing_unused_pool():
    """deleting an existing unused pool."""


@given("a control plane, Io-Engine instances")
def a_control_plane_io_engine_instances(background):
    """a control plane, Io-Engine instances."""


@given("file based pool disks")
def file_based_pool_disks(node_disks):
    """file based pool disks."""


@given("a pool on an unreachable offline node")
def a_pool_on_an_unreachable_offline_node(a_pool_on_an_unreachable_offline_node):
    """a pool on an unreachable offline node."""


@given("a pool on an unreachable online node")
def a_pool_on_an_unreachable_online_node(a_pool_on_an_unreachable_online_node):
    """a pool on an unreachable online node."""


@given("a pool with no replicas")
def a_pool_with_no_replicas(a_pool_with_no_replicas):
    """a pool with no replicas."""


@given("a pool with replicas")
def a_pool_with_replicas(a_pool_with_replicas):
    """a pool with replicas."""


@when("the user attempts to delete a pool that does not exist")
def the_user_attempts_to_delete_a_pool_that_does_not_exist(
    pool, remove_pool, attempt_delete_the_pool
):
    """the user attempts to delete a pool that does not exist."""


@when("the user attempts to delete the pool")
def the_user_attempts_to_delete_the_pool(pool, attempt_delete_the_pool):
    """the user attempts to delete the pool."""


@then('the pool deletion should fail with error kind "InUse"')
def the_pool_deletion_should_fail_with_error_kind_inuse(attempt_delete_the_pool):
    """the pool deletion should fail with error kind "InUse"."""
    assert attempt_delete_the_pool.status == http.HTTPStatus.CONFLICT
    assert ApiClient.exception_to_error(attempt_delete_the_pool).kind == "InUse"


@then('the pool deletion should fail with error kind "Unavailable"')
def the_pool_deletion_should_fail_with_error_kind_unavailable(attempt_delete_the_pool):
    """the pool deletion should fail with error kind "Unavailable"."""
    assert (
        attempt_delete_the_pool.status == http.HTTPStatus.SERVICE_UNAVAILABLE
        or attempt_delete_the_pool.status == http.HTTPStatus.PRECONDITION_FAILED
    )
    assert (
        ApiClient.exception_to_error(attempt_delete_the_pool).kind == "Unavailable"
        or ApiClient.exception_to_error(attempt_delete_the_pool).kind
        == "FailedPrecondition"
    )


@then('the pool deletion should fail with error kind "NodeOffline"')
def the_pool_deletion_should_fail_with_error_kind_nodeoffline(attempt_delete_the_pool):
    """the pool deletion should fail with error kind "NodeOffline"."""
    assert attempt_delete_the_pool.status == http.HTTPStatus.PRECONDITION_FAILED
    assert (
        ApiClient.exception_to_error(attempt_delete_the_pool).kind
        == "FailedPrecondition"
    )


@then('the pool deletion should fail with error kind "NotFound"')
def the_pool_deletion_should_fail_with_error_kind_notfound(attempt_delete_the_pool):
    """the pool deletion should fail with error kind "NotFound"."""
    assert attempt_delete_the_pool.status == http.HTTPStatus.NOT_FOUND
    assert ApiClient.exception_to_error(attempt_delete_the_pool).kind == "NotFound"


@then("the pool should be deleted")
def the_pool_should_be_deleted(pool, attempt_delete_the_pool):
    """the pool should be deleted."""
    assert attempt_delete_the_pool is None
    try:
        ApiClient.pools_api().get_pool(pool.id)
    except NotFoundException:
        pass


"""" Implementations """

VOLUME_UUID = "5cd5378e-3f05-47f1-a830-a0f5873a1449"
VOLUME_SIZE = 10485761


@pytest.fixture(scope="module")
def background():
    Deployer.start(2, node_deadline="250ms", io_engine_env="MAYASTOR_HB_INTERVAL_SEC=0")
    yield
    Deployer.stop()


@pytest.fixture
def nodes():
    return ApiClient.nodes_api().get_nodes()


@pytest.fixture
def node_disks(nodes):
    pools = Deployer.create_disks(1, size=100 * 1024 * 1024)
    yield pools
    Deployer.cleanup_disks(len(pools))


@pytest.fixture
def attempt_delete_the_pool(pool):
    try:
        return ApiClient.pools_api().del_pool(pool.id)
    except ApiException as exception:
        return exception


@pytest.fixture
def pool(node_disks, nodes):
    assert len(nodes) > 0
    assert len(node_disks) > 0
    node = nodes[0]
    pool = ApiClient.pools_api().put_node_pool(
        node.id, f"{node.id}-pool", CreatePoolBody([node_disks[0]])
    )
    yield pool


@pytest.fixture
def remove_pool(pool):
    ApiClient.pools_api().del_pool(pool.id)


@pytest.fixture
def a_pool_with_no_replicas(pool):
    yield pool
    try:
        ApiClient.pools_api().del_pool(pool.id)
    except NotFoundException:
        pass


@pytest.fixture
def a_pool_with_replicas(pool):
    volume = ApiClient.volumes_api().put_volume(
        VOLUME_UUID, CreateVolumeBody(VolumePolicy(False), 1, VOLUME_SIZE, False)
    )
    print(volume)
    yield
    ApiClient.volumes_api().del_volume(VOLUME_UUID)
    ApiClient.pools_api().del_pool(pool.id)


@pytest.fixture
def a_pool_on_an_unreachable_online_node(pool):
    Docker.kill_container(pool.spec.node)
    yield pool
    wait_node_offline(pool.spec.node)
    Docker.restart_container(pool.spec.node)
    wait_node_online(pool.spec.node)
    try:
        ApiClient.pools_api().del_pool(pool.id)
    except NotFoundException:
        pass


@pytest.fixture
def a_pool_on_an_unreachable_offline_node(pool):
    Docker.stop_container(pool.spec.node)
    wait_node_offline(pool.spec.node)
    yield pool
    Docker.restart_container(pool.spec.node)
    wait_node_online(pool.spec.node)
    try:
        ApiClient.pools_api().del_pool(pool.id)
    except NotFoundException:
        pass


@retry(wait_fixed=200, stop_max_attempt_number=30)
def wait_node_online(node_id):
    assert ApiClient.nodes_api().get_node(node_id).state.status == NodeStatus("Online")


@retry(wait_fixed=200, stop_max_attempt_number=30)
def wait_node_offline(node_id):
    node_status = ApiClient.nodes_api().get_node(node_id).state.status
    assert node_status == NodeStatus("Offline") or node_status == NodeStatus("Unknown")
