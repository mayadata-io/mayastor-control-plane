Feature: Readiness Probe

  Background:
    Given a running agent-core service
    And a running REST service with "--core-health-freq" set to "4s"

  Scenario: The REST API /ready service should not update its readiness status more than once in 4s
    Given agent-core service is available
    And the REST service returns a 200 status code for an HTTP GET request to the /ready endpoint
    When the agent-core service is brought down forcefully
    Then the REST service returns 200 for /ready endpoint for 2 more second
    And after a delay of 4s the REST service returns 503 for /ready endpoint for the following 5s
