Feature: Controller gRPC API for CSI Controller

Background:
    Given a running CSI controller plugin

Scenario: get controller capabilities
    When a ControllerGetCapabilities request is sent to CSI controller
    Then CSI controller should report all its capabilities

Scenario: get overall storage capacity
    Given 2 Io-Engine nodes with one pool on each node
    When a GetCapacity request is sent to the controller
    Then CSI controller should report overall capacity equal to aggregated sizes of the pools

Scenario: get node storage capacity
    Given 2 Io-Engine nodes with one pool on each node
    When GetCapacity request with node name is sent to the controller
    Then CSI controller should report capacity for target node

Scenario: create 1 replica nvmf volume
    Given 2 Io-Engine nodes with one pool on each node
    When a CreateVolume request is sent to create a 1 replica nvmf volume
    Then a new volume of requested size should be successfully created
    And volume context should reflect volume creation parameters

Scenario: volume creation idempotency
    Given an existing unpublished volume
    When a CreateVolume request is sent to create a volume identical to existing volume
    Then a CreateVolume request for the identical volume should succeed
    And volume context should be identical to the context of existing volume

Scenario: remove existing volume
    Given an existing unpublished volume
    When a DeleteVolume request is sent to CSI controller to delete existing volume
    Then existing volume should be deleted

Scenario: volume removal idempotency
    Given a non-existing volume
    When a DeleteVolume request is sent to CSI controller to delete not existing volume
    Then a DeleteVolume request should succeed as if target volume existed

Scenario: list existing volumes
    Given 2 existing volumes
    When a ListVolumesRequest is sent to CSI controller
    Then all 2 volumes are listed

Scenario: validate SINGLE_NODE_WRITER volume capability
    Given an existing unpublished volume
    When a ValidateVolumeCapabilities request for SINGLE_NODE_WRITER capability is sent to CSI controller
    Then SINGLE_NODE_WRITER capability should be confirmed for the volume
    And no error message should be reported in ValidateVolumeCapabilitiesResponse

Scenario: validate non-SINGLE_NODE_WRITER volume capability
    Given an existing unpublished volume
    When a ValidateVolumeCapabilities request with non-SINGLE_NODE_WRITER capability is sent to CSI controller
    Then no capabilities should be confirmed for the volume
    And error message should be reported in ValidateVolumeCapabilitiesResponse

Scenario: publish volume over nvmf
    Given an existing unpublished volume
    When a ControllerPublishVolume request is sent to CSI controller to publish volume on specified node
    Then nvmf target which exposes the volume should be created on specified node
    And volume should report itself as published

Scenario: publish volume idempotency
    Given a volume published on a node
    When a ControllerPublishVolume request is sent to CSI controller to re-publish volume on the same node
    Then nvmf target which exposes the volume should be created on specified node
    And volume should report itself as published

Scenario: unpublish volume
    Given a volume published on a node
    When a ControllerUnpublishVolume request is sent to CSI controller to unpublish volume from its node
    Then nvmf target which exposes the volume should be destroyed on specified node
    And volume should report itself as not published

Scenario: unpublish volume idempotency
    Given an existing unpublished volume
    When a ControllerUnpublishVolume request is sent to CSI controller to unpublish not published volume
    Then a ControllerUnpublishVolume request should succeed as if target volume was published

Scenario: unpublish volume from a different node
    Given a volume published on a node
    When a ControllerUnpublishVolume request is sent to CSI controller to unpublish volume from a different node
    Then a ControllerUnpublishVolume request should fail with NOT_FOUND error
    And volume should report itself as published

Scenario: unpublish not existing volume
    Given a non-existing volume
    When a ControllerUnpublishVolume request is sent to CSI controller to unpublish not existing volume
    Then a ControllerUnpublishVolume request should succeed as if not existing volume was published

Scenario: republish volume on a different node
    Given a volume published on a node
    When a ControllerPublishVolume request is sent to CSI controller to re-publish volume on a different node
    Then a ControllerPublishVolume request should fail with FAILED_PRECONDITION error mentioning node mismatch
    And volume should report itself as published

Scenario: unpublish volume when nexus node is offline
    Given a volume published on a node
    When a node that hosts the nexus becomes offline
    Then a ControllerUnpublishVolume request should succeed as if nexus node was online
    And volume should be successfully republished on the other node

Scenario: list existing volumes with pagination
    Given 2 existing volumes
    When a ListVolumesRequest is sent to CSI controller with max_entries set to 1
    Then only a single volume should be returned
    And a subsequent ListVolumesRequest using the next token should return the next volume

Scenario: list existing volumes with pagination max entries set to 0
    Given 2 existing volumes
    When a ListVolumesRequest is sent to CSI controller with max_entries set to 0
    Then all volumes should be returned
    And the next token should be empty

Scenario: unpublish volume from non-frontend node
    Given a 2 replica volume published on a node
    When the ControllerUnpublishVolume request arrives from a non-frontend node
    Then nvmf target which exposes the volume should not be destroyed
    And volume should remain as published
