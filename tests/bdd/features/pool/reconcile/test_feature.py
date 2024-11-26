"""Pool reconciliation feature tests."""

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
from openapi.model.node_status import NodeStatus
from openapi.model.pool_status import PoolStatus
from openapi.exceptions import ApiException
from openapi.exceptions import NotFoundException


@scenario("feature.feature", "destroying a pool that needs to be deleted")
def test_destroying_a_pool_that_needs_to_be_deleted():
    """destroying a pool that needs to be deleted."""


@scenario("feature.feature", "import a pool when a node restarts")
def test_import_a_pool_when_a_node_restarts():
    """import a pool when a node restarts."""


@given("a control plane, Io-Engine instances")
def a_control_plane_io_engine_instances(background):
    """a control plane, Io-Engine instances."""


@given("file based pool disks")
def file_based_pool_disks(node_disks):
    """file based pool disks."""


@given('a pool "p0" on an unreachable node')
def a_pool_p0_on_an_unreachable_node(a_pool_on_an_unreachable_offline_node):
    """a pool "p0" on an unreachable node."""


@given('a pool "p0" that could not be deleted due to an unreachable node')
def a_pool_p0_that_could_not_be_deleted_due_to_an_unreachable_node(
    pool, a_pool_on_an_unreachable_offline_node, attempt_delete_the_pool
):
    """a pool "p0" that could not be deleted due to an unreachable node."""
    assert attempt_delete_the_pool.status == http.HTTPStatus.PRECONDITION_FAILED
    assert not hasattr(ApiClient.pools_api().get_pool(pool.id), "state")
    wait_node_offline(pool.spec.node)


@when("the node comes back online")
def the_node_comes_back_online(pool):
    """the node comes back online."""
    Docker.restart_container(pool.spec.node)
    wait_node_online(pool.spec.node)


@then("the pool should eventually be deleted")
def the_pool_should_eventually_be_deleted(pool):
    """the pool should eventually be deleted."""
    wait_pool_deleted(pool)


@then("the pool should eventually be imported")
def the_pool_should_eventually_be_imported(pool):
    """the pool should eventually be imported."""
    wait_pool_imported(pool)


"""" Implementations """


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
def a_pool_on_an_unreachable_offline_node(pool):
    Docker.stop_container(pool.spec.node)
    wait_node_offline(pool.spec.node)
    yield pool
    if Docker.container_status(pool.spec.node) != "running":
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


@retry(wait_fixed=200, stop_max_attempt_number=30)
def wait_pool_deleted(pool):
    try:
        pool = ApiClient.pools_api().get_pool(pool.id)
        assert pool.state == PoolStatus("Deleted")
    except NotFoundException:
        pass


@retry(wait_fixed=200, stop_max_attempt_number=30)
def wait_pool_imported(pool):
    pool = ApiClient.pools_api().get_pool(pool.id)
    assert pool.state.status == PoolStatus("Online")
