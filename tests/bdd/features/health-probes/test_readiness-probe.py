"""Readiness Probe feature tests."""

import logging
import time

import pytest
import requests
from common.deployer import Deployer
from common.docker import Docker
from pytest_bdd import given, scenario, then, when
from retrying import retry

logger = logging.getLogger(__name__)


def ready_http_get(context_msg_prefix: str):
    try:
        response = requests.get("http://localhost:8081/ready", timeout=(0.3, 0.3))
        logger.info(
            f"{context_msg_prefix}: response.status_code: {response.status_code}"
        )
        return response
    except requests.exceptions.Timeout as e:
        logger.error(f"{context_msg_prefix}: the request timed out")
        raise e

    except requests.exceptions.RequestException as e:
        logger.error(f"{context_msg_prefix}: an error occurred: {e}")
        raise e


@pytest.fixture(scope="module")
def setup():
    Deployer.start(io_engines=1, rest_core_health_freq="800ms", request_timeout="50ms")
    yield
    Deployer.stop()


@scenario(
    "readiness-probe.feature",
    "The REST API /ready service should not update its readiness status more than once in the cache refresh period",
)
def test_the_rest_api_ready_service_should_not_update_its_readiness_status_more_than_once_in_the_cache_refresh_period(
    setup,
):
    """The REST API /ready service should not update its readiness status more than once in the cache refresh period."""


@given('a running REST service with the cache refresh period set to "800ms"')
def a_running_rest_service(setup):
    """a running REST service with the cache refresh period set to "800ms"."""


@given("a running agent-core service")
def a_running_agent_core_service(setup):
    """a running agent-core service."""


@given("agent-core service is available")
def agent_core_service_is_available(setup):
    """agent-core service is available."""


@given(
    "the REST service returns a 200 status code for an HTTP GET request to the /ready endpoint"
)
def the_rest_service_returns_a_200_status_code_for_an_http_get_request_to_the_ready_endpoint(
    setup,
):
    """the REST service returns a 200 status code for an HTTP GET request to the /ready endpoint."""
    logger.info(f"Initial request: expected status: 200")

    # 5 minute retry.
    @retry(
        stop_max_attempt_number=1500,
        wait_fixed=200,
    )
    def rest_is_ready():
        response = ready_http_get(context_msg_prefix="Initial request")
        assert response.status_code == 200

    rest_is_ready()


@when("the agent-core service is brought down forcefully")
def the_agent_core_service_is_brought_down_forcefully(setup):
    """the agent-core service is brought down forcefully."""
    Docker.kill_container("core")


@then(
    "the REST service return changes from 200 to 503 within double of the cache refresh period"
)
def the_rest_service_return_changes_from_200_to_503_within_double_of_the_cache_refresh_period(
    setup,
):
    """the REST service return changes from 200 to 503 within double of the cache refresh period."""
    logger.info(f"Request after killing core: expected status: 503")

    @retry(wait_fixed=50, stop_max_delay=1600)
    def rest_is_not_ready():
        response = ready_http_get(context_msg_prefix="Request after killing core")
        assert response.status_code == 503

    rest_is_not_ready()


@then("it keeps returning 503 at least for the cache refresh period")
def it_keeps_returning_503_at_least_for_the_cache_refresh_period(
    setup,
):
    """it keeps returning 503 at least for the cache refresh period."""
    logger.info(f"Request after cache refresh: expected status: 503")

    start_time = time.time()
    while time.time() - start_time < 0.8:
        response = ready_http_get(context_msg_prefix="Request after cache refresh")
        if response.status_code != 503:
            raise ValueError(
                "Expected Readiness probe to return 503 for this duration of 800ms"
            )
