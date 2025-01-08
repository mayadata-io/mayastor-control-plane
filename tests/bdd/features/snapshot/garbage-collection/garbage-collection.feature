Feature: Volume Snapshot garbage collection

  Background:
    Given a deployer cluster

  Scenario Outline: Garbage collection for failed transactions
    Given we have a single replica <publish_status> volume
    And the node hosting the replica is paused
    When we attempt to create a snapshot for the volume
    Then the snapshot creation should fail
    When the node hosting the replica is unpaused
    And we attempt to create a snapshot with the same id for the volume
    Then the snapshot creation should be successful
    When a get snapshot call is made for the snapshot
    Then the response should not contain the failed transactions
    Examples:
      | publish_status |
      | published      |
      | unpublished    |


  Scenario Outline: Garbage collection for stuck creating snapshots when source is deleted
    Given we have a single replica <publish_status> volume
    And the node hosting the replica is paused
    When we attempt to create a snapshot for the volume
    Then the snapshot creation should fail
    When the node hosting the replica is unpaused
    And the source volume is deleted
    Then the corresponding snapshot stuck in creating state should be deleted
    Examples:
      | publish_status |
      | published      |
      | unpublished    |
