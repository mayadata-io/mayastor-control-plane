"""Switchover Robustness feature tests."""

import os
import time

from pytest_bdd import (
    given,
    scenario,
    then,
    when,
)
import pytest
import subprocess

from retrying import retry

import common.nvme
from common.deployer import Deployer
from common.apiclient import ApiClient
from common.docker import Docker
from common.etcd import Etcd
from common.nvme import (
    nvme_connect,
    nvme_disconnect,
    nvme_list_subsystems,
    nvme_set_reconnect_delay,
    nvme_find_subsystem_devices_all,
)
from common.operations import Cluster

from openapi.model.create_pool_body import CreatePoolBody
from openapi.model.create_volume_body import CreateVolumeBody
from openapi.model.publish_volume_body import PublishVolumeBody
from openapi.model.volume_policy import VolumePolicy
from openapi.model.volume_share_protocol import VolumeShareProtocol

VOLUME_UUID = "5cd5378e-3f05-47f1-a830-a0f5873a1449"
VOLUME_SIZE = int(20 * 1024 * 1024)
POOL_SIZE = 100 * 1024 * 1024
TARGET_NODE_1 = "io-engine-1"
TARGET_NODE_2 = "io-engine-2"
TARGET_NODE_3 = "io-engine-3"
DEPLOYER_NETWORK_INTERFACE = "mayabridge0"
NVME_SVC_PORT = 8420
RULE_APPEND = "sudo iptables -t filter -A OUTPUT -o {} -d {} -p tcp --dport {} -j DROP -m comment --comment 'added by bdd test'"
RULE_REMOVE = "sudo iptables -t filter -D OUTPUT -o {} -d {} -p tcp --dport {} -j DROP -m comment --comment 'added by bdd test'"
ETCD_CLIENT = Etcd()


@pytest.fixture(scope="module")
def init():
    Deployer.start(
        2,
        cache_period="100ms",
        reconcile_period="200ms",
        cluster_agent=True,
        cluster_agent_fast="100ms",
        node_agent=True,
        csi_node=True,
        io_engine_coreisol=True,
        io_engine_env="MAYASTOR_HB_INTERVAL_SEC=0",
        agents_env="DETECTION_PERIOD=100ms,SUBSYS_REFRESH_PERIOD=100ms",
    )
    yield
    Deployer.stop(True)


@pytest.fixture(autouse=True)
def init_scenario(init, disks):
    for disk_index in range(0, 2):
        node_index = disk_index + 1
        name = f"pool-{node_index}"
        node = f"io-engine-{node_index}"
        ApiClient.pools_api().put_node_pool(
            node, name, CreatePoolBody([disks[disk_index]])
        )
    yield
    if Cluster.fixture_cleanup():
        Docker.kill_container("agent-ha-cluster")
        Docker.kill_container("agent-ha-node")
        ETCD_CLIENT.del_all_switchover()
        common.nvme.nvme_disconnect_allours_wait()
        Docker.restart_container("agent-ha-cluster")
        Docker.restart_container("agent-ha-node")
        Cluster.cleanup()


@scenario("robustness.feature", "reconnecting the new target times out")
def test_reconnecting_the_new_target_times_out():
    """reconnecting the new target times out."""


@scenario("robustness.feature", "path failure with no free nodes")
def test_path_failure_with_no_free_nodes():
    """path failure with no free nodes."""


@scenario("robustness.feature", "temporary path failure with no other nodes")
def test_temporary_path_failure_with_no_other_nodes():
    """temporary path failure with no other nodes."""


@scenario("robustness.feature", "second failure during switchover with no other nodes")
def test_second_failure_during_switchover_with_no_other_nodes():
    """second failure during switchover with no other nodes."""


@given("a connected nvme initiator")
def a_connected_nvme_initiator(connect_to_first_path):
    """a connected nvme initiator."""
    volume = pytest.volume
    device_uri = volume.state["target"]["device_uri"]
    nvme_set_reconnect_delay(device_uri, 1)


@given("a deployer cluster")
def a_deployer_cluster(init):
    """a deployer cluster."""
    yield


@given("a reconnect_delay set to 15s")
def a_reconnect_delay_set_to_15s():
    """a reconnect_delay set to 15s."""
    volume = pytest.volume
    device_uri = volume.state["target"]["device_uri"]
    nvme_set_reconnect_delay(device_uri, 7)


@given("a single replica volume")
def a_single_replica_volume():
    """a single replica volume."""
    ApiClient.volumes_api().put_volume(
        VOLUME_UUID, CreateVolumeBody(VolumePolicy(True), 1, VOLUME_SIZE, False)
    )
    volume = ApiClient.volumes_api().put_volume_target(
        VOLUME_UUID,
        publish_volume_body=PublishVolumeBody(
            {}, VolumeShareProtocol("nvmf"), node=TARGET_NODE_1
        ),
    )
    pytest.volume = volume


@given("a 2 replica volume")
def a_2_replica_volume():
    """a 2 replica volume."""
    ApiClient.volumes_api().put_volume(
        VOLUME_UUID, CreateVolumeBody(VolumePolicy(True), 2, VOLUME_SIZE, False)
    )
    volume = ApiClient.volumes_api().put_volume_target(
        VOLUME_UUID,
        publish_volume_body=PublishVolumeBody(
            {}, VolumeShareProtocol("nvmf"), node=TARGET_NODE_1
        ),
    )
    pytest.volume = volume


@when("we cordon the non-target node")
def we_cordon_the_nontarget_node():
    """we cordon the non-target node."""
    ApiClient.nodes_api().put_node_cordon(TARGET_NODE_2, "d")
    wait_node_cordon(TARGET_NODE_2)
    yield
    try:
        ApiClient.nodes_api().delete_node_cordon(TARGET_NODE_2, "d")
    except:
        pass


@when("the ha clustering republishes")
def the_ha_clustering_republishes():
    """the ha clustering republishes."""
    time.sleep(1)


@when("we restart the volume target node")
def we_restart_the_volume_target_node():
    """we restart the volume target."""
    Docker.restart_container(TARGET_NODE_1)


@when("we stop the volume target node")
def we_stop_the_volume_target_node():
    """we stop the volume target."""
    Docker.stop_container(TARGET_NODE_1)
    yield
    Docker.restart_container(TARGET_NODE_1)


@when("we uncordon the non-target node")
def we_uncordon_the_nontarget_node():
    """we uncordon the non-target node."""
    ApiClient.nodes_api().delete_node_cordon(TARGET_NODE_2, "d")


@when("the ha clustering fails a few times")
def the_ha_clustering_fails_a_few_times():
    """the ha clustering fails a few times."""
    # we have no way of determining this? maybe through etcd?
    time.sleep(1)


@then("the path should be established")
def the_path_should_be_established(connect_to_first_path):
    """the path should be established."""
    wait_initiator_reconnect(connect_to_first_path)


@when("we restart the volume target node")
def we_restart_the_volume_target_node():
    """we restart the volume target node."""
    Docker.restart_container(TARGET_NODE_1)


@when("the ha clustering fails as there is no other node")
def the_ha_clustering_fails_as_there_is_no_other_node():
    """the ha clustering fails as there is no other node."""
    # we have no way of determining this? maybe through etcd?
    time.sleep(2)


@when("the volume target node has io-path broken")
def the_volume_target_node_has_iopath_broken():
    """the volume target node has io-path broken."""
    simulate_network_failure(TARGET_NODE_1, NVME_SVC_PORT)
    # Restart container to speed up the failure
    Docker.stop_container(TARGET_NODE_1)
    time.sleep(1)
    Docker.restart_container(TARGET_NODE_1)
    yield
    remove_network_failure(TARGET_NODE_1, NVME_SVC_PORT, False)


@when("the volume target node has io-path fixed")
def the_volume_target_node_has_iopath_fixed():
    """the volume target node has io-path fixed."""
    remove_network_failure(TARGET_NODE_1, NVME_SVC_PORT)


@pytest.fixture
def disks():
    pools = Deployer.create_disks(2, size=POOL_SIZE)
    yield pools
    Deployer.cleanup_disks(len(pools))


@pytest.fixture
def connect_to_first_path():
    volume = pytest.volume
    device_uri = volume.state["target"]["device_uri"]
    yield nvme_connect(device_uri)
    nvme_disconnect(device_uri)


@retry(wait_fixed=100, stop_max_attempt_number=200)
def wait_initiator_reconnect(connect_to_first_path):
    device = connect_to_first_path
    desc = nvme_list_subsystems(device)
    assert (
        len(desc["Subsystems"]) == 1
    ), "Must be exactly one NVMe subsystem for target nexus"
    subsystem = desc["Subsystems"][0]
    assert len(subsystem["Paths"]) == 1, "Must be exactly one I/O path to target nexus"
    assert subsystem["Paths"][0]["State"] == "live", "I/O path is not healthy"


@retry(wait_fixed=100, stop_max_attempt_number=20)
def wait_node_cordon(node):
    node = ApiClient.nodes_api().get_node(node)
    assert "cordonedstate" in node.spec.cordondrainstate


def simulate_network_failure(node_name, port):
    node_ip = Docker.container_ip(node_name)
    command = RULE_APPEND.format(DEPLOYER_NETWORK_INTERFACE, node_ip, port)
    try:
        subprocess.run(command, shell=True, check=True)
    except subprocess.CalledProcessError:
        assert False, "Error while adding IP table rule"


def remove_network_failure(node_name, port, assert_error=True):
    node_ip = Docker.container_ip(node_name)
    command = RULE_REMOVE.format(DEPLOYER_NETWORK_INTERFACE, node_ip, port)
    try:
        subprocess.run(command, shell=True, check=True)
    except subprocess.CalledProcessError:
        assert not assert_error, "Error while adding IP table rule"
