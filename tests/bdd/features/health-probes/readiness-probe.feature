Feature: Readiness Probe

  Background:
    Given a running agent-core service
    And a running REST service with the cache refresh period set to "800ms"

  Scenario: The REST API /ready service should not update its readiness status more than once in the cache refresh period
    Given agent-core service is available
    And the REST service returns a 200 status code for an HTTP GET request to the /ready endpoint
    When the agent-core service is brought down forcefully
    Then the REST service return changes from 200 to 503 within double of the cache refresh period
    And it keeps returning 503 at least for the cache refresh period
