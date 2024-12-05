"""Readiness Probe feature tests."""

import time

import pytest
from common.deployer import Deployer
from common.docker import Docker
from pytest_bdd import given, scenario, then, when
from requests import get as http_get
from retrying import retry

READINESS_API_ENDPOINT = "http://localhost:8081/ready"


@pytest.fixture(scope="module")
def setup():
    Deployer.start(io_engines=1, rest_core_health_freq="4s")
    yield
    Deployer.stop()


@scenario(
    "readiness_probe.feature",
    "The REST API /ready service should not update its readiness status more than once in 4s",
)
def test_the_rest_api_ready_service_should_not_update_its_readiness_status_more_than_once_in_4s(
    setup,
):
    """The REST API /ready service should not update its readiness status more than once in 4s."""


@given('a running REST service with "--core-health-freq" set to "4s"')
def a_running_rest_service(setup):
    """a running REST service with "--core-health-freq" set to "4s"."""


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

    # 5 minute retry.
    @retry(
        stop_max_attempt_number=1500,
        wait_fixed=200,
    )
    def rest_is_ready():
        response = http_get(READINESS_API_ENDPOINT)
        assert response.status_code == 200

    rest_is_ready()


@when("the agent-core service is brought down forcefully")
def the_agent_core_service_is_brought_down_forcefully(setup):
    """the agent-core service is brought down forcefully."""
    Docker.kill_container("core")


@then("the REST service returns 200 for /ready endpoint for 2 more second")
def the_rest_service_returns_200_for_ready_endpoint_for_2_more_second(setup):
    """the REST service returns 200 for /ready endpoint for 2 more second."""
    start_time = time.time()
    while time.time() - start_time < 2:
        response = http_get(READINESS_API_ENDPOINT)
        if response.status_code != 200:
            raise ValueError(
                "Expected Readiness probe to return 200 for this duration of 2s"
            )


@then(
    "after a delay of 4s the REST service returns 503 for /ready endpoint for the following 5s"
)
def after_a_delay_of_4s_the_rest_service_returns_503_for_ready_endpoint_for_the_following_5s(
    setup,
):
    """after a delay of 4s the REST service returns 503 for /ready endpoint for the following 5s."""
    time.sleep(4)

    start_time = time.time()
    while time.time() - start_time < 5:
        response = http_get(READINESS_API_ENDPOINT)
        if response.status_code != 503:
            raise ValueError(
                "Expected Readiness probe to return 503 for this duration of 5s"
            )
