Feature: Volume resize

    Background:
        Given a deployer cluster

    Scenario Outline: Expand an unpublished volume
        Given an unpublished volume with <repl_count> replicas and all are healthy
        When we issue a volume expand request
        Then the request should succeed
        And all the replicas of the volume should be resized to the new capacity
        And the volume is expanded to the new capacity
        Examples:
        | repl_count |
        | 1          |
        | 3          |


    Scenario: Expand an unpublished volume and then make it published
        Given an unpublished volume with more than one replica and all are healthy
        When we issue a volume expand request
        Then the request should succeed
        And all the replicas of the volume should be resized to the new capacity
        And the volume is expanded to the new capacity
        When the volume is published
        Then the volume should get published with expanded capacity
        And the new capacity should be available for the application to use
        When the volume replica count is increased by 1
        Then the new replica should have expanded size


    Scenario: Expand a published volume
        Given a published volume with more than one replica and all are healthy
        When we issue a volume expand request
        Then the volume expand status should be failure
        And the failure reason should be volume-in-use precondition


    Scenario: Expand a published volume after unpublishing it while having an offline replica
        Given a published volume with more than one replica and all are healthy
        When the volume is receiving IO
        And one of the replica goes offline
        And the volume is unpublished
        And the replica comes online again
        And we issue a volume expand request
        Then the request should succeed
        And all the replicas of the volume should be resized to the new capacity
        When the volume is republished
        Then the volume should get published with expanded capacity
        And the onlined replica should be rebuilt


    Scenario: Expand an unpublished volume with an offline replica
        Given an unpublished volume with more than one replica
        And one of the replica is not in online state
        When we issue a volume expand request
        Then the volume expand status should be failure


    # Volume shrink/downsize isn't supported by csi.
    Scenario: Shrink an unpublished volume
        Given an unpublished volume with more than one replica and all are healthy
        When we issue a volume shrink request
        Then the volume resize status should be failure
        And the failure reason should be invalid arguments