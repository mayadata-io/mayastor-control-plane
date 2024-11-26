"""Pool creation feature tests."""

import http
import os
import pytest

from pytest_bdd import (
    given,
    scenario,
    then,
    when,
)

from common.deployer import Deployer
from common.apiclient import ApiClient
from common.operations import Pool as PoolOps
from openapi.model.create_pool_body import CreatePoolBody
from openapi.model.pool import Pool
from openapi.exceptions import ApiException


@scenario("disks.feature", "creating a pool with a URING disk")
def test_creating_a_pool_with_a_uring_disk():
    """creating a pool with a URING disk."""


@scenario("disks.feature", "creating a pool with a non-existent disk")
def test_creating_a_pool_with_a_nonexistent_disk():
    """creating a pool with a non-existent disk."""


@scenario("disks.feature", "creating a pool with an AIO disk")
def test_creating_a_pool_with_an_aio_disk():
    """creating a pool with an AIO disk."""


@scenario("disks.feature", "creating a pool with multiple disks")
def test_creating_a_pool_with_multiple_disks():
    """creating a pool with multiple disks."""


@given("a control plane and Io-Engine instances")
def a_control_plane_and_io_engine_instances(background):
    """a control plane and Io-Engine instances."""


@when("the user attempts to create a pool specifying a URI representing a uring disk")
def the_user_attempts_to_create_a_pool_specifying_a_uri_representing_a_uring_disk(
    attempt_create_pool_uring_disk,
):
    """the user attempts to create a pool specifying a URI representing a uring disk."""


@when("the user attempts to create a pool specifying a URI representing an aio disk")
def the_user_attempts_to_create_a_pool_specifying_a_uri_representing_an_aio_disk(
    attempt_create_pool_aio_disk,
):
    """the user attempts to create a pool specifying a URI representing an aio disk."""


@when("the user attempts to create a pool specifying multiple disks")
def the_user_attempts_to_create_a_pool_specifying_multiple_disks(
    attempt_create_pool_multiple_disk,
):
    """the user attempts to create a pool specifying multiple disks."""


@when("the user attempts to create a pool with a non-existent disk")
def the_user_attempts_to_create_a_pool_with_a_nonexistent_disk(
    attempt_create_pool_nonexistent_disk,
):
    """the user attempts to create a pool with a non-existent disk."""


@then('the pool creation should fail with error kind "NotFound"')
def the_pool_creation_should_fail_with_error_internal(pool_attempt):
    """the pool creation should fail with error kind "NotFound"."""
    assert pool_attempt.status == http.HTTPStatus.NOT_FOUND
    assert ApiClient.exception_to_error(pool_attempt).kind == "NotFound"


@then('the pool creation should fail with error kind "InvalidArgument"')
def the_pool_creation_should_fail_with_error_invalidargument(pool_attempt):
    """the pool creation should fail with error kind "InvalidArgument"."""
    assert pool_attempt.status == http.HTTPStatus.BAD_REQUEST
    assert ApiClient.exception_to_error(pool_attempt).kind == "InvalidArgument"


@then("the pool should be created successfully")
def the_pool_should_be_created_successfully(pool_attempt):
    """the pool should be created successfully."""
    pool_creation_succeeds(pool_attempt)


"""" Implementations """

POOL_NODE = "io-engine-1"
POOL_NAME = "p0"
POOL_DISK_COUNT = 2


@pytest.fixture(scope="module")
def background():
    Deployer.start(1)
    yield
    Deployer.stop()


@pytest.fixture(scope="function")
def context():
    return {}


@pytest.fixture(scope="function")
def pool_attempt(context):
    return context["attempt"]


@pytest.fixture
def disks():
    pools = Deployer.create_disks(POOL_DISK_COUNT, size=100 * 1024 * 1024)
    yield pools
    Deployer.cleanup_disks(len(pools))


@pytest.fixture
def attempt_create_pool_nonexistent_disk(context, attempt_create_pool):
    context["attempt"] = attempt_create_pool(POOL_NAME, POOL_NODE, ["/dev/no_there"])


@pytest.fixture
def attempt_create_pool_aio_disk(context, disks, attempt_create_pool):
    assert len(disks) > 0
    context["attempt"] = attempt_create_pool(
        POOL_NAME, POOL_NODE, [f"aio://{disks[0]}"]
    )


@pytest.fixture
def attempt_create_pool_uring_disk(context, disks, attempt_create_pool):
    assert len(disks) > 0
    context["attempt"] = attempt_create_pool(
        POOL_NAME, POOL_NODE, [f"uring://{disks[0]}"]
    )


@pytest.fixture
def attempt_create_pool_multiple_disk(context, disks, attempt_create_pool):
    assert len(disks) > 0
    context["attempt"] = attempt_create_pool(POOL_NAME, POOL_NODE, disks)


@pytest.fixture
def attempt_create_pool():
    def _method(name, node, disks):
        try:
            pool = ApiClient.pools_api().put_node_pool(
                node, name, CreatePoolBody(disks)
            )
            return pool
        except ApiException as e:
            return e

    yield _method
    PoolOps.delete_all()


def pool_creation_succeeds(result):
    # raise result for exception information
    assert isinstance(result, Pool)
    ApiClient.pools_api().get_pool(result.id)


def pool_creation_fails(result, expected_error_type):
    # raise result for exception information
    assert isinstance(result, expected_error_type)


def different_node(node_id):
    nodes = ApiClient.nodes_api().get_nodes()
    nodes = list(filter(lambda node: node.id != node_id, nodes))
    return nodes[0].id
