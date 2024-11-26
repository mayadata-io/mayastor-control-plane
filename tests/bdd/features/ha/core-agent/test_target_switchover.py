"""Target Switchover test feature tests."""
import os
import subprocess
from urllib.parse import urlparse

import grpc
import pytest
from pytest_bdd import (
    given,
    scenario,
    then,
    when,
    parsers,
)

from common.apiclient import ApiClient
from common.deployer import Deployer
from common.fio import Fio
from common.operations import Cluster

from openapi.model.create_pool_body import CreatePoolBody
from openapi.model.create_volume_body import CreateVolumeBody
from openapi.model.protocol import Protocol
from openapi.model.volume_policy import VolumePolicy
from openapi.model.publish_volume_body import PublishVolumeBody

POOL_UUID_1 = "4cc6ee64-7232-497d-a26f-38284a444980"
POOL_UUID_2 = "22e1d15f-4dfd-4bf5-a98f-74e4aebf9e62"
VOLUME_UUID = "5cd5378e-3f05-47f1-a830-a0f5873a1449"
NODE_NAME_1 = "io-engine-1"
NODE_NAME_2 = "io-engine-2"
VOLUME_CTX_KEY = "volume"
VOLUME_SIZE = 10485761
DEPLOYER_NETWORK_INTERFACE = "mayabridge0"
NVME_SVC_PORT = 8420
IO_ENGINE_GRPC_PORT = 10124
IO_ENGINE_1_IP = "10.1.0.7"
RULE_APPEND = "sudo iptables -t filter -A OUTPUT -o {} -d {} -p tcp --dport {} -j DROP -m comment --comment 'added by bdd test'"
RULE_REMOVE = "sudo iptables -t filter -D OUTPUT -o {} -d {} -p tcp --dport {} -j DROP -m comment --comment 'added by bdd test'"


@pytest.fixture
def disks():
    yield Deployer.create_disks(2, size=100 * 1024 * 1024)
    Deployer.cleanup_disks(2)


@pytest.fixture(scope="module")
def init():
    Deployer.start(
        io_engines=3,
        reconcile_period="300ms",
        cache_period="200ms",
        io_engine_coreisol=True,
        fio_spdk=True,
    )
    yield
    Deployer.stop(True)


@scenario(
    "target_switchover.feature",
    "R/W access to older target should be restricted after switchover",
)
def test_rw_access_to_older_target_should_be_restricted_after_switchover():
    """R/W access to older target should be restricted after switchover."""


@scenario(
    "target_switchover.feature",
    "R/W access to older target should be restricted if io-path and grpc connection is established",
)
def test_rw_access_to_older_target_should_be_restricted_if_iopath_and_grpc_connection_is_established():
    """R/W access to older target should be restricted if io-path and grpc connection is established."""


@scenario(
    "target_switchover.feature",
    "R/W access to older target should be restricted if io-path is established",
)
def test_rw_access_to_older_target_should_be_restricted_if_iopath_is_established():
    """R/W access to older target should be restricted if io-path is established."""


@scenario(
    "target_switchover.feature",
    "continuous switchover and older target destruction should be seamless with io-path and grpc connection broken",
)
def test_continuous_switchover_and_older_target_destruction_should_be_seamless_with_iopath_and_grpc_connection_broken():
    """continuous switchover and older target destruction should be seamless with io-path and grpc connection broken."""


@scenario(
    "target_switchover.feature",
    "continuous switchover and older target destruction should be seamless with io-path broken",
)
def test_continuous_switchover_and_older_target_destruction_should_be_seamless_with_iopath_broken():
    """continuous switchover and older target destruction should be seamless with io-path broken."""


@scenario("target_switchover.feature", "node offline should not fail the switchover")
def test_node_offline_should_not_fail_the_switchover():
    """node offline should not fail the switchover."""


@given("a control plane, two Io-Engine instances, two pools")
def a_control_plane_two_ioengine_instances_two_pools(init, disks):
    """a control plane, two Io-Engine instances, two pools."""
    pytest.reuse_existing = False
    ApiClient.pools_api().put_node_pool(
        NODE_NAME_1, POOL_UUID_1, CreatePoolBody([disks[0]])
    )
    ApiClient.pools_api().put_node_pool(
        NODE_NAME_2, POOL_UUID_2, CreatePoolBody([disks[1]])
    )
    yield
    cleanup_iptable_rules(IO_ENGINE_1_IP)
    Cluster.cleanup()


@given("a published volume with two replicas")
def a_published_volume_with_two_replicas():
    """a published volume with two replicas."""
    ApiClient.volumes_api().put_volume(
        VOLUME_UUID, CreateVolumeBody(VolumePolicy(False), 2, VOLUME_SIZE, False)
    )
    volume = ApiClient.volumes_api().put_volume_target(
        VOLUME_UUID,
        publish_volume_body=PublishVolumeBody(
            {}, Protocol("nvmf"), node=NODE_NAME_1, frontend_node="app-node-1"
        ),
    )
    pytest.older_target_uri = volume["state"]["target"]["deviceUri"]


@when("the destroy shutdown target call has succeeded")
def the_destroy_shutdown_target_call_has_succeeded():
    """the destroy shutdown target call has succeeded."""
    try:
        ApiClient.volumes_api().del_volume_shutdown_targets(VOLUME_UUID)
    except grpc.RpcError:
        pytest.fail("Volume Republish Failed")


@when("the node hosting the nexus has grpc server connection broken")
def the_node_hosting_the_nexus_has_grpc_server_connection_broken():
    """the node hosting the nexus has grpc server connection broken."""
    simulate_network_failure(IO_ENGINE_1_IP, IO_ENGINE_GRPC_PORT)


@when("the node hosting the nexus has grpc server connection established")
def the_node_hosting_the_nexus_has_grpc_server_connection_established():
    """the node hosting the nexus has grpc server connection established."""
    remove_network_failure(IO_ENGINE_1_IP, IO_ENGINE_GRPC_PORT)


@when("the node hosting the nexus has io-path broken")
def the_node_hosting_the_nexus_has_iopath_broken():
    """the node hosting the nexus has io-path broken."""
    simulate_network_failure(IO_ENGINE_1_IP, NVME_SVC_PORT)


@when("the node hosting the older nexus has io-path established")
def the_node_hosting_the_older_nexus_has_iopath_established():
    """the node hosting the older nexus has io-path established."""
    remove_network_failure(IO_ENGINE_1_IP, NVME_SVC_PORT)


@when("the node hosting the nexus is killed")
def the_node_hosting_the_nexus_is_killed():
    """the node hosting the nexus is killed."""
    pytest.reuse_existing = True
    kill_docker_container(NODE_NAME_1)


@when("the node hosting the nexus is brought back")
def the_node_hosting_the_nexus_is_brought_back():
    """the node hosting the nexus is brought back"""
    start_docker_container(NODE_NAME_1)


@when(
    parsers.parse(
        "the volume republish and the destroy shutdown target call has succeeded for {n:d} times"
    )
)
def the_volume_republish_and_the_destroy_shutdown_target_call_has_succeeded_for_n_times(
    n,
):
    """the volume republish and the destroy shutdown target call has succeeded for `n` times."""
    for i in range(n):
        try:
            ApiClient.volumes_api().put_volume_target(
                VOLUME_UUID,
                publish_volume_body=PublishVolumeBody(
                    {},
                    Protocol("nvmf"),
                    republish=True,
                    reuse_existing=pytest.reuse_existing,
                    frontend_node="app-node-1",
                ),
            )
        except grpc.RpcError:
            pytest.fail("Volume republish call failed")
        try:
            ApiClient.volumes_api().del_volume_shutdown_targets(VOLUME_UUID)
        except grpc.RpcError:
            pytest.fail("Volume destroy shutdown target call failed")


@when("the volume republish on another node has succeeded")
def the_volume_republish_on_another_node_has_succeeded():
    """the volume republish on another node has succeeded."""
    try:
        ApiClient.volumes_api().put_volume_target(
            VOLUME_UUID,
            publish_volume_body=PublishVolumeBody(
                {},
                Protocol("nvmf"),
                reuse_existing=pytest.reuse_existing,
                node=NODE_NAME_2,
                republish=True,
                frontend_node="app-node-1",
            ),
        )
    except grpc.RpcError:
        pytest.fail("Volume Republish Failed")


@then("the newer target should have R/W access to the replicas")
def the_newer_target_should_have_rw_access_to_the_replicas():
    """the newer target should have R/W access to the replicas."""
    newer_target = get_newer_target()
    uri = urlparse(newer_target["deviceUri"])

    fio = Fio(name="job", rw="randrw", size="50M", uri=uri)

    try:
        code = fio.run().returncode
        assert code == 0, "Fio is expected to execute successfully"
    except subprocess.CalledProcessError:
        assert False, "FIO is not expected to be errored out"


@then("the older target should not have R/W access the replicas")
def the_older_target_should_not_have_rw_access_the_replicas():
    """the older target should not have R/W access the replicas."""
    uri = urlparse(pytest.older_target_uri)

    fio = Fio(name="job", rw="randrw", size="50M", uri=uri)

    try:
        code = fio.run().returncode
        assert code != 0, "Fio is not expected to execute successfully"
    except subprocess.CalledProcessError:
        assert True, "FIO is not expected to be errored out"


# HELPER FUNCTIONS


def get_newer_target():
    volume = ApiClient.volumes_api().get_volume(VOLUME_UUID)
    return volume.state["target"]


def kill_docker_container(name):
    command = "docker kill {}".format(name)
    try:
        subprocess.run(command, shell=True, check=True)
    except subprocess.CalledProcessError:
        assert False, "Could not kill the docker container"


def start_docker_container(name):
    command = "docker start {}".format(name)
    try:
        subprocess.run(command, shell=True, check=True)
    except subprocess.CalledProcessError:
        assert False, "Could not kill the docker container"


def simulate_network_failure(io_engine_ip, port):
    command = RULE_APPEND.format(DEPLOYER_NETWORK_INTERFACE, io_engine_ip, port)
    try:
        subprocess.run(command, shell=True, check=True)
    except subprocess.CalledProcessError:
        assert False, "Error while adding IP table rule"


def remove_network_failure(io_engine_ip, port):
    command = RULE_REMOVE.format(DEPLOYER_NETWORK_INTERFACE, io_engine_ip, port)
    try:
        subprocess.run(command, shell=True, check=True)
    except subprocess.CalledProcessError:
        assert False, "Error while adding IP table rule"


def cleanup_iptable_rules(io_engine_ip):
    # Remove grpc failure rules
    while True:
        command = RULE_REMOVE.format(
            DEPLOYER_NETWORK_INTERFACE, io_engine_ip, IO_ENGINE_GRPC_PORT
        )
        try:
            subprocess.run(command, shell=True, check=True)
        except subprocess.CalledProcessError:
            break

    # Remove io failure rules
    while True:
        command = RULE_REMOVE.format(
            DEPLOYER_NETWORK_INTERFACE, io_engine_ip, NVME_SVC_PORT
        )
        try:
            subprocess.run(command, shell=True, check=True)
        except subprocess.CalledProcessError:
            break
