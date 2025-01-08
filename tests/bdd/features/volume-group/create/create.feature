Feature: Anti-Affinity for Affinity Group

  Background:
    Given an io-engine cluster

  # Affinity Groups to have volumes with single replicas

  Scenario Outline: creating affinity group with 3 volumes with 1 replica each
    Given 5 pools, 2 on node A, 1 on node B, 2 on node C
    When the create volume request with affinity group and <replicas> and <thin> executed
    Then 3 volumes should be created with 1 replica each
    And each replica should reside on a different node
    Examples:
      | replicas | thin  |
      |     1    | True  |
      |     1    | False |

  Scenario Outline: creating affinity group with 3 volumes with 1 replica each with limited pools
    Given 2 pools, 1 on node A, 0 on node B, 1 on node C
    When the create volume request with affinity group and <replicas> and <thin> executed
    Then 2 volumes should be created with 1 replica each
    And each replica should reside on a different node
    And 1 volume creation should fail due to insufficient storage
    Examples:
      | replicas | thin  |
      |     1    | True  |
      |     1    | False |

  # Affinity Groups to have volumes with multiple replicas.

  Scenario Outline: creating affinity group with 3 volumes with 3 replica on 9 pools
    Given 9 pools, 3 on node A, 3 on node B, 3 on node C
    When the create volume request with affinity group and <replicas> and <thin> executed
    Then 3 volumes should be created with 3 replica each
    And each replica should reside on a different pool
    Examples:
      | replicas | thin  |
      |     3    | True  |
      |     3    | False |

  Scenario Outline: creating affinity group with 3 volumes with 3 replica on 5 pools
    Given 5 pools, 2 on node A, 1 on node B, 2 on node C
    When the create volume request with affinity group and <replicas> and <thin> executed
    Then 3 volumes should be created with 3 replica each
    And each replica of individual volume should be on a different node
    And pools can have more than one replicas all belonging to different volumes
    Examples:
      | replicas | thin  |
      |     3    | True  |
      |     3    | False |

  Scenario Outline: creating affinity group with 3 volumes with 3 replica on 3 pools
    Given 3 pools, 1 on node A, 1 on node B, 1 on node C
    When the create volume request with affinity group and <replicas> and <thin> executed
    Then 3 volumes should be created with 3 replica each
    And each replica of individual volume should be on a different node
    And pools can have more than one replicas all belonging to different volumes
    Examples:
      | replicas | thin  |
      |     3    | True  |
      |     3    | False |

