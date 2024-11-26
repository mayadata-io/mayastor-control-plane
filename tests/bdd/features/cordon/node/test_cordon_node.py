"""Cordoning feature tests."""

from pytest_bdd import (
    given,
    scenario,
    then,
    when,
)

import os
import pytest
import requests

from common.apiclient import ApiClient
from common.operations import Cluster
from common.deployer import Deployer
from common.docker import Docker

from openapi.model.create_pool_body import CreatePoolBody
from openapi.model.create_volume_body import CreateVolumeBody
from openapi.model.volume_policy import VolumePolicy

VOLUME_UUID_1 = "5cd5378e-3f05-47f1-a830-111111111111"
VOLUME_UUID_2 = "5cd5378e-3f05-47f1-a830-222222222222"

VOLUME_SIZE = 1048
NUM_VOLUME_REPLICAS = 1
CREATE_REQUEST_KEY = "create_request"

NODE_NAME_1 = "io-engine-1"
NODE_NAME_2 = "io-engine-2"

POOL_UUID_1 = "4cc6ee64-7232-497d-a26f-111111111111"

CORDON_LABEL_1 = "cordon_label_1"
CORDON_LABEL_2 = "cordon_label_2"


def pytest_configure():
    pytest.command_failed = False


@pytest.fixture(scope="module")
def init():
    Deployer.start(2)
    yield
    Deployer.stop()


@pytest.fixture(autouse=True)
def init_scenario(init, disks):
    pytest.command_failed = False
    ApiClient.pools_api().put_node_pool(
        NODE_NAME_1, POOL_UUID_1, CreatePoolBody([f"{disks[0]}"])
    )
    yield
    Docker.restart_container("core")
    Cluster.cleanup()
    remove_all_cordons(NODE_NAME_1)
    remove_all_cordons(NODE_NAME_2)


@pytest.fixture(scope="module")
def disks():
    yield Deployer.create_disks(2, size=100 * 1024 * 1024)
    Deployer.cleanup_disks(2)


# Fixture used to pass the volume create request between test steps.
@pytest.fixture(scope="function")
def create_request():
    return {}


###### utility functions ######


def cordon_node(node_name, label):
    ApiClient.nodes_api().put_node_cordon(node_name, label)
    assert is_cordoned(node_name) == True


def create_volume(vol_uuid):
    request = CreateVolumeBody(
        VolumePolicy(False), NUM_VOLUME_REPLICAS, VOLUME_SIZE, False
    )
    ApiClient.volumes_api().put_volume(vol_uuid, request)
    # Check that the volume was created.
    volumes = ApiClient.volumes_api().get_volume(vol_uuid)


def create_volume_and_fail(vol_uuid):
    request = CreateVolumeBody(
        VolumePolicy(False), NUM_VOLUME_REPLICAS, VOLUME_SIZE, False
    )
    try:
        ApiClient.volumes_api().put_volume(vol_uuid, request)
        assert False
    except Exception as e:
        print(f"creation of volume {vol_uuid} failed, and is expected")
    # Check that the volume wasn't created.
    try:
        volumes = ApiClient.volumes_api().get_volume(vol_uuid)
        assert False
    except Exception as e:
        print(f"volume {vol_uuid} not found, as expected")


def delete_volume(vol_uuid):
    ApiClient.volumes_api().del_volume(vol_uuid)


def delete_all_volumes():
    volumes = ApiClient.volumes_api().get_volumes()
    for vol in volumes.entries:
        ApiClient.volumes_api().del_volume(vol.spec.uuid)


def get_volumes():
    return ApiClient.volumes_api().get_volumes()


def is_cordoned(node_name):
    node = ApiClient.nodes_api().get_node(node_name)
    present = False
    try:
        assert node.spec.cordondrainstate["cordonedstate"]["cordonlabels"] != []
        return True
    except AttributeError as e:
        return False


def remove_all_cordons(node_name):
    if is_cordoned(node_name):
        node = ApiClient.nodes_api().get_node(node_name)
        for label in node.spec.cordondrainstate["cordonedstate"]["cordonlabels"]:
            ApiClient.nodes_api().delete_node_cordon(node_name, label)


def uncordon_node(node_name, label):
    try:
        ApiClient.nodes_api().delete_node_cordon(node_name, label)
        pytest.command_failed = False
    except Exception as e:
        pytest.command_failed = True


############### scenarios ##################


@scenario("feature.feature", "Cordoning a cordoned node")
def test_cordoning_a_cordoned_node():
    """Cordoning a cordoned node."""


@scenario("feature.feature", "Cordoning a node")
def test_cordoning_a_node():
    """Cordoning a node."""


@pytest.mark.skip(
    "Disabled because the BDD relates to kubectl idempotent behaviour which differs from REST-layer behaviour - which will fail."
)
@scenario("feature.feature", "Cordoning a node with an existing label")
def test_cordoning_a_node_with_an_existing_label():
    """Cordoning a node with an existing label."""


@scenario("feature.feature", "Cordoning a node with existing resources")
def test_cordoning_a_node_with_existing_resources():
    """Cordoning a node with existing resources."""


@scenario("feature.feature", "Deleting resources on a cordoned node")
def test_deleting_resources_on_a_cordoned_node():
    """Deleting resources on a cordoned node."""


@pytest.mark.skip(
    "Disabled because nexus node placement seems to need to be specified at this level."
)
@scenario(
    "feature.feature", "Nexus placement on a cluster with one or more cordoned nodes"
)
def test_nexus_placement_on_a_cluster_with_one_or_more_cordoned_nodes():
    """Nexus placement on a cluster with one or more cordoned nodes."""


@scenario(
    "feature.feature", "Node should be cordoned if there is at least one cordon applied"
)
def test_node_should_be_cordoned_if_there_is_at_least_one_cordon_applied():
    """Node should be cordoned if there is at least one cordon applied."""


@scenario(
    "feature.feature", "Node should be uncordoned when all cordons have been removed"
)
def test_node_should_be_uncordoned_when_all_cordons_have_been_removed():
    """Node should be uncordoned when all cordons have been removed."""


@scenario("feature.feature", "Uncordoning a node")
def test_uncordoning_a_node():
    """Uncordoning a node."""


@scenario("feature.feature", "Uncordoning a node with an unknown label")
def test_uncordoning_a_node_with_an_unknown_label():
    """Uncordoning a node with an unknown label."""


@scenario("feature.feature", "Uncordoning a node with existing resources")
def test_uncordoning_a_node_with_existing_resources():
    """Uncordoning a node with existing resources."""


@scenario("feature.feature", "Uncordoning an uncordoned node")
def test_uncordoning_an_uncordoned_node():
    """Uncordoning an uncordoned node."""


@pytest.mark.skip(
    "Disabled because there is currently no support code to degrade a volume"
)
@scenario("feature.feature", "Unschedulable replicas due to node cordon")
def test_unschedulable_replicas_due_to_node_cordon():
    """Unschedulable replicas due to node cordon."""


###### given clauses ######


@given("Multiple uncordoned nodes")
def multiple_uncordoned_nodes():
    """Multiple uncordoned nodes."""
    assert is_cordoned(NODE_NAME_1) == False
    assert is_cordoned(NODE_NAME_2) == False


@given("a cordoned node")
def a_cordoned_node():
    """a cordoned node."""
    vols = get_volumes().entries
    assert len(vols) == 0
    cordon_node(NODE_NAME_1, CORDON_LABEL_1)
    assert is_cordoned(NODE_NAME_1) == True


@given("a cordoned node with multiple cordons")
def a_cordoned_node_with_multiple_cordons():
    """a cordoned node with multiple cordons."""
    vols = get_volumes().entries
    assert len(vols) == 0
    cordon_node(NODE_NAME_1, CORDON_LABEL_1)
    cordon_node(NODE_NAME_1, CORDON_LABEL_2)
    assert is_cordoned(NODE_NAME_1) == True


@given("a cordoned node with resources")
def a_cordoned_node_with_resources():
    """a cordoned node with resources."""
    create_volume(VOLUME_UUID_1)
    vols = get_volumes().entries
    assert len(vols) == 1
    cordon_node(NODE_NAME_1, CORDON_LABEL_1)


@given("a published volume with multiple replicas")
def a_published_volume_with_multiple_replicas():
    """a published volume with multiple replicas."""
    raise NotImplementedError


@given("an uncordoned node")
def an_uncordoned_node():
    """an uncordoned node."""
    vols = get_volumes().entries
    assert len(vols) == 0
    assert is_cordoned(NODE_NAME_1) == False


@given("an uncordoned node with resources")
def an_uncordoned_node_with_resources():
    """an uncordoned node with resources."""
    vols = get_volumes().entries
    assert len(vols) == 0
    assert is_cordoned(NODE_NAME_1) == False
    create_volume(VOLUME_UUID_1)
    vols = get_volumes().entries
    assert len(vols) == 1


@given("one or more cordoned nodes")
def one_or_more_cordoned_nodes():
    """one or more cordoned nodes."""
    vols = get_volumes().entries
    assert len(vols) == 0
    cordon_node(NODE_NAME_1, CORDON_LABEL_1)


@when("a volume is created and published")
def a_volume_is_created_and_published():
    """a volume is created and published."""
    raise NotImplementedError


@when("all cordons have been removed")
def all_cordons_have_been_removed():
    """all cordons have been removed."""
    remove_all_cordons(NODE_NAME_1)
    remove_all_cordons(NODE_NAME_2)


@when("the control plane attempts to delete resources on a cordoned node")
def the_control_plane_attempts_to_delete_resources_on_a_cordoned_node():
    """the control plane attempts to delete resources on a cordoned node."""
    delete_volume(VOLUME_UUID_1)


@when("the user issues a cordon command to the node with a new label")
def the_user_issues_a_cordon_command_to_the_node_with_a_new_label():
    """the user issues a cordon command to the node with a new label."""
    cordon_node(NODE_NAME_1, CORDON_LABEL_2)


@when("the user issues a cordon command with a label to the node")
def the_user_issues_a_cordon_command_with_a_label_to_the_node():
    """the user issues a cordon command with a label to the node."""
    cordon_node(NODE_NAME_1, CORDON_LABEL_1)


@when("the user issues a cordon command with the same label to the node")
def the_user_issues_a_cordon_command_with_the_same_label_to_the_node():
    """the user issues a cordon command with the same label to the node."""
    raise NotImplementedError


@when("the user issues an uncordon command with a label to the node")
def the_user_issues_an_uncordon_command_with_a_label_to_the_node():
    """the user issues an uncordon command with a label to the node."""
    uncordon_node(NODE_NAME_1, CORDON_LABEL_1)


@when("the user issues an uncordon command with an unknown cordon label to the node")
def the_user_issues_an_uncordon_command_with_an_unknown_cordon_label_to_the_node():
    """the user issues an uncordon command with an unknown cordon label to the node."""
    uncordon_node(NODE_NAME_1, "not_there")


@when("the volume becomes degraded")
def the_volume_becomes_degraded():
    """the volume becomes degraded."""
    raise NotImplementedError


@when("there are insufficient uncordoned nodes to accommodate new replicas")
def there_are_insufficient_uncordoned_nodes_to_accommodate_new_replicas():
    """there are insufficient uncordoned nodes to accommodate new replicas."""
    raise NotImplementedError


@then("existing resources remain unaffected")
def existing_resources_remain_unaffected():
    """existing resources remain unaffected."""
    vols = get_volumes().entries
    assert len(vols) == 1
    assert vols[0].spec.uuid == VOLUME_UUID_1


@then(
    "new resources deployable by the control-plane can be scheduled on the uncordoned node"
)
def new_resources_deployable_by_the_controlplane_can_be_scheduled_on_the_uncordoned_node():
    """new resources deployable by the control-plane can be scheduled on the uncordoned node."""
    create_volume(VOLUME_UUID_2)


@then(
    "new resources deployable by the control-plane cannot be scheduled on the cordoned node"
)
def new_resources_deployable_by_the_controlplane_cannot_be_scheduled_on_the_cordoned_node():
    """new resources deployable by the control-plane cannot be scheduled on the cordoned node."""
    create_volume_and_fail(VOLUME_UUID_2)


@then("the command will fail")
def the_command_will_fail():
    """the command will fail."""
    assert pytest.command_failed == True


@then("the command will succeed")
def the_command_will_succeed():
    """the command will succeed."""
    assert pytest.command_failed == False


@then("the cordoned node should remain cordoned")
def the_cordoned_node_should_remain_cordoned():
    """the cordoned node should remain cordoned."""
    assert is_cordoned(NODE_NAME_1)


@then("the node should be uncordoned")
def the_node_should_be_uncordoned():
    """the node should be uncordoned."""
    assert is_cordoned(NODE_NAME_1) == False


@then("the resources should be deleted")
def the_resources_should_be_deleted():
    """the resources should be deleted."""
    vols = get_volumes().entries
    assert len(vols) == 0


@then("the uncordoned node should remain uncordoned")
def the_uncordoned_node_should_remain_uncordoned():
    """the uncordoned node should remain uncordoned."""
    assert is_cordoned(NODE_NAME_1) == False


@then("the volume target node selected will not be a cordoned node")
def the_volume_target_node_selected_will_not_be_a_cordoned_node():
    """the volume target node selected will not be a cordoned node."""
    raise NotImplementedError


@then("the volume will remain in a degraded state")
def the_volume_will_remain_in_a_degraded_state():
    """the volume will remain in a degraded state."""
    raise NotImplementedError
