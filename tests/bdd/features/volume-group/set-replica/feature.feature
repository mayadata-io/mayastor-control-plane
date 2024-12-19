Feature: Anti-Affinity for Affinity Group

  Background:
    Given an io-engine cluster

  Scenario Outline: affinity group volumes are scaled up
    Given 3 pools, 1 on node A, 1 on node B, 1 on node C
    And 3 volumes belonging to an affinity group with <replicas> each and <thin>
    When the 3 volumes are scaled up by one
    Then 3 new replicas should be created
    Examples:
      | replicas | thin  |
      | 2        | True  |
      | 2        | False |

  Scenario Outline: affinity group volumes are scaled down
    Given 3 pools, 1 on node A, 1 on node B, 1 on node C
    And 3 volumes belonging to an affinity group with <replicas> each and <thin>
    When the 3 volumes are scaled down to 2
    Then 3 replicas should be removed
    And each node should have a maximum of 2 replicas from the affinityGroup
    Examples:
      | replicas | thin  |
      | 3        | True  |
      | 3        | False |

  Scenario Outline: affinity group volumes are scaled down below 2 replicas
    Given 3 pools, 1 on node A, 1 on node B, 1 on node C
    And 3 volumes belonging to an affinity group with <replicas> each and <thin>
    When the 3 volumes are scaled down to 1
    Then the scale down operation should fail
    And no volume should have less than 2 replicas
    Examples:
      | replicas | thin  |
      | 2        | True  |
      | 2        | False |

  Scenario Outline: scale up and subsequent creation
    Given 2 pools, 1 on node A, 0 on node B, 1 on node C
    When the volume belonging to an affinity group with one replica and <thin> is created
    And the volume is scaled up by one
    And the volume belonging to an affinity group with one replica and <thin> is created
    Then the creation should fail
    When a new pool on node B is created
    And the volume belonging to an affinity group with one replica and <thin> is created
    Then the creation should not fail
    Examples:
      | thin  |
      | True  |
      | False |
