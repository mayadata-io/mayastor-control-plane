Feature: Anti-Affinity for Affinity Group

  Background:
    Given a deployer cluster

  Scenario: affinity group target distribution
    Given two pools, one on node A and another on node B
    And one unpublished affinity group volume with 2 replica
    And one affinity group volume with 2 replica published on node A
    And one non affinity group volume with 2 replica published on node B
    When the non published volume is published
    Then the publish should not fail
    And the target should land on node B

  Scenario: affinity group volume undergoes republish
    Given two pools, one on node A and another on node B
    And two published volumes with two replicas each
    And the volume targets are on different nodes
    When one volume is republished
    Then the republish should not fail
    And both the targets should be on the same node
