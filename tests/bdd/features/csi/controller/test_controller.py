"""CSI Controller Identity RPC tests."""
from pytest_bdd import (
    given,
    scenario,
    then,
    when,
)

import pytest
import docker
import csi_pb2 as pb
import grpc
from time import sleep
import subprocess

from urllib.parse import urlparse, parse_qs
import json

from common import csi_topology_key, disk_pool_label
from common.apiclient import ApiClient
from common.csi import CsiHandle
from common.deployer import Deployer
from common.nvme import nvme_bin
from common.operations import Cluster
from openapi.model.create_pool_body import CreatePoolBody
from openapi.exceptions import NotFoundException


VOLUME1_UUID = "d01b8bfb-0116-47b0-a03a-447fcbdc0e99"
VOLUME2_UUID = "d8aab0f1-82f4-406c-89ee-14f08b004aea"
VOLUME3_UUID = "f29b8e73-67d0-4b32-a8ea-a1277d48ef07"
VOLUME4_UUID = "955a12c4-707e-4040-9c4d-e9682213588f"  # 2 replicas
NOT_EXISTING_VOLUME_UUID = "11111111-2222-3333-4444-555555555555"
PVC_VOLUME1_NAME = "pvc-%s" % VOLUME1_UUID
PVC_VOLUME2_NAME = "pvc-%s" % VOLUME2_UUID
PVC_VOLUME3_NAME = "pvc-%s" % VOLUME3_UUID
PVC_VOLUME4_NAME = "pvc-%s" % VOLUME4_UUID
POOL1_UUID = "ec176677-8202-4199-b461-2b68e53a055f"
POOL2_UUID = "bcabda21-9e66-4d81-8c75-bf9f3b687cdc"
NODE1 = "io-engine-1"
NODE2 = "io-engine-2"
VOLUME1_SIZE = 1024 * 1024 * 32
VOLUME2_SIZE = 1024 * 1024 * 22
VOLUME3_SIZE = 1024 * 1024 * 28
VOLUME4_SIZE = 1024 * 1024 * 32
OPENEBS_TOPOLOGY_KEY = csi_topology_key


@pytest.fixture(scope="module")
def setup():
    Deployer.start(2, csi_controller=True)

    # Create 2 pools.
    pool_labels = disk_pool_label
    pool_api = ApiClient.pools_api()
    pool_api.put_node_pool(
        NODE1,
        POOL1_UUID,
        CreatePoolBody(["malloc:///disk?size_mb=128"], labels=pool_labels),
    )
    pool_api.put_node_pool(
        NODE2,
        POOL2_UUID,
        CreatePoolBody(["malloc:///disk?size_mb=128"], labels=pool_labels),
    )
    yield
    if Cluster.env_cleanup():
        try:
            pool_api.del_pool(POOL1_UUID)
            pool_api.del_pool(POOL2_UUID)
        except:
            pass
    Deployer.stop()


def csi_rpc_handle():
    return CsiHandle("unix:///var/tmp/csi-controller.sock")


@scenario("controller.feature", "get controller capabilities")
def test_controller_capabilities(setup):
    """get controller capabilities"""


@scenario("controller.feature", "get overall storage capacity")
def test_overall_capacity(setup):
    """get overall capacity"""


@scenario("controller.feature", "get node storage capacity")
def test_node_capacity(setup):
    """get node capacity"""


@scenario("controller.feature", "create 1 replica nvmf volume")
def test_create_1_replica_nvmf_volume(setup):
    """create 1 replica nvmf volume"""


@scenario("controller.feature", "volume creation idempotency")
def test_volume_creation_idempotency(setup):
    """volume creation idempotency"""


@scenario("controller.feature", "remove existing volume")
def test_remove_existing_volume(setup):
    """remove existing volume"""


@scenario("controller.feature", "volume removal idempotency")
def test_volume_removal_idempotency(setup):
    """volume removal idempotency"""


@scenario("controller.feature", "list existing volumes")
def test_list_existing_volumes(setup):
    """list existing volumes"""


@scenario("controller.feature", "list existing volumes with pagination")
def test_list_existing_volumes_with_pagination(setup):
    """list existing volumes with pagination."""


@scenario(
    "controller.feature", "list existing volumes with pagination max entries set to 0"
)
def test_list_existing_volumes_with_pagination_max_entries_set_to_0(setup):
    """list existing volumes with pagination max entries set to 0."""


@scenario("controller.feature", "validate SINGLE_NODE_WRITER volume capability")
def test_validate_single_node_writer_capability(setup):
    """validate SINGLE_NODE_WRITER volume capability"""


@scenario(
    "controller.feature",
    "validate non-SINGLE_NODE_WRITER volume capability",
)
def test_validate_non_single_node_writer_capability(setup):
    """validate non-SINGLE_NODE_WRITER volume capability"""


@scenario("controller.feature", "publish volume over nvmf")
def test_publish_volume_over_nvmf(setup):
    """publish volume over nvmf"""


@scenario("controller.feature", "publish volume idempotency")
def test_publish_volume_idempotency(setup):
    """publish volume idempotency"""


@scenario("controller.feature", "unpublish volume")
def test_unpublish_volume(setup):
    """unpublish volume"""


@scenario("controller.feature", "unpublish volume from non-frontend node")
def test_unpublish_volume_from_nonfrontend_node(setup):
    """unpublish volume from non-frontend node."""


@scenario("controller.feature", "unpublish volume idempotency")
def test_unpublish_volume_idempotency(setup):
    """unpublish volume idempotency"""


@pytest.mark.skip("This test is no longer valid as local restriction is revoked")
@scenario("controller.feature", "unpublish volume from a different node")
def test_unpublish_volume_from_a_different_node(setup):
    """unpublish volume on a different node"""


@scenario("controller.feature", "unpublish not existing volume")
def test_unpublish_not_existing_volume(setup):
    """unpublish not existing volume"""


@scenario("controller.feature", "republish volume on a different node")
def test_republish_volume_on_a_different_node(setup):
    """republish volume on a different node"""


@scenario("controller.feature", "unpublish volume when nexus node is offline")
def test_unpublish_volume_with_offline_nexus_node(setup):
    """unpublish volume when nexus node is offline"""


@given("a running CSI controller plugin", target_fixture="csi_instance")
def a_csi_instance():
    return csi_rpc_handle()


@given("2 Io-Engine nodes with one pool on each node", target_fixture="two_pools")
def two_nodes_with_one_pool_each():
    pool_api = ApiClient.pools_api()
    pool1 = pool_api.get_pool(POOL1_UUID)
    pool2 = pool_api.get_pool(POOL2_UUID)
    return [pool1, pool2]


@given("2 existing volumes", target_fixture="two_volumes")
def two_existing_volumes(_create_2_volumes_1_replica):
    return _create_2_volumes_1_replica


@given("a 2 replica volume published on a node")
def populate_published_2_replica_volume(_create_2_replica_nvmf_volume):
    do_publish_volume(VOLUME4_UUID, NODE1)
    # Make sure volume is published.
    volume = ApiClient.volumes_api().get_volume(VOLUME4_UUID)
    assert (
        str(volume.spec.target.protocol) == "nvmf"
    ), "Protocol mismatches for published volume"
    return volume


@given("a non-existing volume")
def a_non_existing_volume():
    with pytest.raises(NotFoundException) as e:
        ApiClient.volumes_api().get_volume(NOT_EXISTING_VOLUME_UUID)


@given("a volume published on a node", target_fixture="populate_published_volume")
def populate_published_volume(_create_1_replica_nvmf_volume):
    do_publish_volume(VOLUME1_UUID, NODE1)

    # Make sure volume is published.
    volume = ApiClient.volumes_api().get_volume(VOLUME1_UUID)

    assert (
        str(volume.spec.target.protocol) == "nvmf"
    ), "Protocol mismatches for published volume"
    assert (
        volume.state.target["protocol"] == "nvmf"
    ), "Protocol mismatches for published volume"
    return volume


@pytest.fixture
def _create_2_replica_nvmf_volume():
    yield csi_create_2_replica_nvmf_volume4()
    if Cluster.fixture_cleanup():
        csi_delete_2_replica_nvmf_volume4()


@pytest.fixture
def populate_published_2_replica_volume(_create_2_replica_nvmf_volume):
    do_publish_volume(VOLUME4_UUID, NODE1)

    # Make sure volume is published.
    volume = ApiClient.volumes_api().get_volume(VOLUME4_UUID)
    assert (
        str(volume.spec.target.protocol) == "nvmf"
    ), "Protocol mismatches for published volume"
    assert (
        volume.state.target["protocol"] == "nvmf"
    ), "Protocol mismatches for published volume"
    return volume


@pytest.fixture
def start_stop_ms1():
    docker_client = docker.from_env()
    try:
        node1 = docker_client.containers.list(all=True, filters={"name": NODE1})[0]
    except docker.errors.NotFound:
        raise Exception("No Io-Engine instance found that hosts the nexus")
    # Stop the nexus node and wait till nexus offline status is also reflected in volume target info.
    # Wait at most 60 seconds.
    node1.stop()
    state_synced = False
    for i in range(12):
        vol = ApiClient.volumes_api().get_volume(VOLUME4_UUID)
        if getattr(vol.state, "target", None) is None:
            state_synced = True
            break
        sleep(5)
    assert state_synced, "Nexus failure is not reflected in volume target info"
    yield
    node1.start()


@when(
    "a node that hosts the nexus becomes offline", target_fixture="offline_nexus_node"
)
def offline_nexus_node(populate_published_2_replica_volume, start_stop_ms1):
    pass


@then("a ControllerUnpublishVolume request should succeed as if nexus node was online")
def check_unpublish_volume_for_offline_nexus_node(offline_nexus_node):
    do_unpublish_volume(VOLUME4_UUID, NODE1)


@then("volume should be successfully republished on the other node")
def check_republish_volume_for_offline_nexus_node(offline_nexus_node):
    do_publish_volume(VOLUME4_UUID, NODE2)


@when(
    "a CreateVolume request is sent to create a 1 replica local nvmf volume (local=true)",
    target_fixture="create_1_replica_local_nvmf_volume",
)
def create_1_replica_local_nvmf_volume(_create_1_replica_local_nvmf_volume):
    return _create_1_replica_local_nvmf_volume


@when(
    "a ControllerPublishVolume request is sent to CSI controller to re-publish volume on a different node",
    target_fixture="republish_volume_on_a_different_node",
)
def republish_volume_on_a_different_node(populate_published_volume):
    with pytest.raises(grpc.RpcError) as e:
        do_publish_volume(VOLUME1_UUID, NODE2)
    return e.value


@then("a new local volume of requested size should be successfully created")
def check_1_replica_local_nvmf_volume(create_1_replica_local_nvmf_volume):
    assert (
        create_1_replica_local_nvmf_volume.volume.capacity_bytes == VOLUME3_SIZE
    ), "Volume size mismatches"
    volume = ApiClient.volumes_api().get_volume(VOLUME3_UUID)
    assert volume.spec.num_replicas == 1, "Number of volume replicas mismatches"
    assert volume.spec.size == VOLUME3_SIZE, "Volume size mismatches"


@then(
    "a ControllerPublishVolume request should fail with FAILED_PRECONDITION error mentioning node mismatch"
)
def check_republish_volume_on_a_different_node(republish_volume_on_a_different_node):
    grpc_error = republish_volume_on_a_different_node

    assert (
        grpc_error.code() == grpc.StatusCode.FAILED_PRECONDITION
    ), "Unexpected gRPC error code: %s" + str(grpc_error.code())
    assert "is only accessible to nodes" in grpc_error.details(), (
        "Error message reflects a different failure: %s" % grpc_error.details()
    )


@when(
    "a ControllerUnpublishVolume request is sent to CSI controller to unpublish volume from a different node",
    target_fixture="unpublish_volume_on_a_different_node",
)
def unpublish_volume_on_a_different_node(populate_published_volume):
    with pytest.raises(grpc.RpcError) as e:
        do_unpublish_volume(VOLUME1_UUID, NODE2)
    return e.value


@when(
    "a ControllerUnpublishVolume request is sent to CSI controller to unpublish not existing volume",
    target_fixture="unpublish_not_existing_volume",
)
def unpublish_not_existing_volume():
    return do_unpublish_volume(NOT_EXISTING_VOLUME_UUID, NODE1)


@then("a ControllerUnpublishVolume request should fail with NOT_FOUND error")
def check_unpublish_volume_on_a_different_node(unpublish_volume_on_a_different_node):
    grpc_error = unpublish_volume_on_a_different_node

    assert (
        grpc_error.code() == grpc.StatusCode.NOT_FOUND
    ), "Unexpected gRPC error code: %s" + str(grpc_error.code())


@when(
    "a ControllerUnpublishVolume request is sent to CSI controller to unpublish volume from its node",
    target_fixture="unpublish_volume_from_its_node",
)
def unpublish_volume_from_its_node(populate_published_volume):
    uri = populate_published_volume.state.target["deviceUri"]
    # Make sure the volume is still published and is discoverable.
    assert check_nvmf_target(uri), "Volume is not discoverable over NVMF"
    do_unpublish_volume(VOLUME1_UUID, NODE1)
    return uri


@when(
    "a ControllerUnpublishVolume request is sent to CSI controller to unpublish not published volume"
)
def unpublish_not_published_volume_from_its_node(existing_volume):
    # Make sure the volume is not published and is discoverable.
    volume = ApiClient.volumes_api().get_volume(VOLUME1_UUID)
    assert "target" not in volume.spec, "Volume is still published"
    do_unpublish_volume(VOLUME1_UUID, NODE1)


@then("nvmf target which exposes the volume should be destroyed on specified node")
def check_unpublish_volume_from_its_node(unpublish_volume_from_its_node):
    assert (
        check_nvmf_target(unpublish_volume_from_its_node) is False
    ), "Unpublished volume is still discoverable over NVMF"


def do_unpublish_volume(volume_id, node_id):
    req = pb.ControllerUnpublishVolumeRequest(volume_id=volume_id, node_id=node_id)
    return csi_rpc_handle().controller.ControllerUnpublishVolume(req)


def do_publish_volume(volume_id, node_id, protocol=None):
    if protocol is None:
        protocol = "nvmf"

    req = pb.ControllerPublishVolumeRequest(
        volume_id=volume_id,
        node_id=node_id,
        volume_context={"protocol": protocol},
        volume_capability={
            "access_mode": pb.VolumeCapability.AccessMode(
                mode=pb.VolumeCapability.AccessMode.Mode.SINGLE_NODE_WRITER
            )
        },
    )
    return csi_rpc_handle().controller.ControllerPublishVolume(req)


@when(
    "a ControllerPublishVolume request is sent to CSI controller to publish volume on specified node",
    target_fixture="publish_nvmf_volume",
)
def publish_nvmf_volume():
    return do_publish_volume(VOLUME1_UUID, NODE1)


@when(
    "a ControllerPublishVolume request is sent to CSI controller to re-publish volume on the same node",
    target_fixture="publish_nvmf_volume",
)
def publish_nvmf_volume():
    return do_publish_volume(VOLUME1_UUID, NODE1)


@then("nvmf target which exposes the volume should be created on specified node")
def check_publish_nvmf_volume_on_node(publish_nvmf_volume):
    # Check that Nexus URI is returned.
    assert (
        "uri" in publish_nvmf_volume.publish_context
    ), "No URI provided for shared volume"
    uri = publish_nvmf_volume.publish_context["uri"]
    assert uri.startswith("nvmf://"), "Non-nvmf protocol scheme in share URI: " + uri
    assert check_nvmf_target(uri), "Volume is not discoverable over NVMF"


@when(
    "a ValidateVolumeCapabilities request with non-SINGLE_NODE_WRITER capability is sent to CSI controller",
    target_fixture="validate_non_snw_capability",
)
def validate_non_single_node_writer_capability():
    req = pb.ValidateVolumeCapabilitiesRequest(
        volume_id=VOLUME1_UUID,
        volume_capabilities=[
            {
                "access_mode": pb.VolumeCapability.AccessMode(
                    mode=pb.VolumeCapability.AccessMode.Mode.MULTI_NODE_SINGLE_WRITER
                )
            }
        ],
    )

    return csi_rpc_handle().controller.ValidateVolumeCapabilities(req)


@then("no capabilities should be confirmed for the volume")
@then("error message should be reported in ValidateVolumeCapabilitiesResponse")
def check_validate_non_single_node_writer_capability(validate_non_snw_capability):
    assert (
        len(validate_non_snw_capability.message) > 0
    ), "Error not reported for not supported capabilities"
    caps = validate_non_snw_capability.confirmed.volume_capabilities
    assert len(caps) == 0, "Not supported capabilities confirmed"


@when(
    "a ValidateVolumeCapabilities request for SINGLE_NODE_WRITER capability is sent to CSI controller",
    target_fixture="validate_snw_capability",
)
def validate_single_node_writer_capability():
    req = pb.ValidateVolumeCapabilitiesRequest(
        volume_id=VOLUME1_UUID,
        volume_capabilities=[
            {
                "access_mode": pb.VolumeCapability.AccessMode(
                    mode=pb.VolumeCapability.AccessMode.Mode.SINGLE_NODE_WRITER
                )
            }
        ],
    )

    return csi_rpc_handle().controller.ValidateVolumeCapabilities(req)


@then("SINGLE_NODE_WRITER capability should be confirmed for the volume")
@then("no error message should be reported in ValidateVolumeCapabilitiesResponse")
def check_validate_single_node_writer_capability(validate_snw_capability):
    assert (
        len(validate_snw_capability.message) == 0
    ), "Error reported for fully supported capability"
    caps = validate_snw_capability.confirmed.volume_capabilities
    access_mode = pb.VolumeCapability.AccessMode(
        mode=pb.VolumeCapability.AccessMode.Mode.SINGLE_NODE_WRITER
    )
    assert len(caps) == 1, "Wrong number of supported capabilities reported"
    assert caps[0].access_mode == access_mode, "Reported access mode does not match"


@when("a ListVolumesRequest is sent to CSI controller", target_fixture="list_2_volumes")
def list_all_volumes(two_volumes):
    vols = csi_rpc_handle().controller.ListVolumes(pb.ListVolumesRequest())
    return [two_volumes, vols.entries]


@when(
    "a ListVolumesRequest is sent to CSI controller with max_entries set to 0",
    target_fixture="paginated_volumes_list",
)
def a_listvolumesrequest_is_sent_to_csi_controller_with_max_entries_set_to_0(
    two_volumes,
):
    """a ListVolumesRequest is sent to CSI controller with max_entries set to 0."""
    vols = csi_rpc_handle().controller.ListVolumes(pb.ListVolumesRequest(max_entries=0))
    return [two_volumes, vols.entries, vols.next_token]


@when(
    "a ListVolumesRequest is sent to CSI controller with max_entries set to 1",
    target_fixture="paginated_volumes_list",
)
def a_listvolumesrequest_is_sent_to_csi_controller_with_max_entries_set_to_1(
    two_volumes,
):
    """a ListVolumesRequest is sent to CSI controller with max_entries set to 1."""
    vols = csi_rpc_handle().controller.ListVolumes(
        pb.ListVolumesRequest(max_entries=1, starting_token="0")
    )
    return [two_volumes, vols.entries, vols.next_token]


@then("all 2 volumes are listed")
def check_list_all_volumes(list_2_volumes):
    created_volumes = sorted(list_2_volumes[0], key=lambda v: v.volume.volume_id)
    listed_volumes = sorted(list_2_volumes[1], key=lambda v: v.volume.volume_id)

    for i in range(2):
        vol1 = created_volumes[i].volume
        vol2 = created_volumes[i].volume

        assert vol1.volume_id == vol2.volume_id, "Volumes have different UUIDs"
        assert (
            vol1.capacity_bytes == vol2.capacity_bytes
        ), "Volumes have different sizes"

        check_volume_context(vol1.volume_context, vol2.volume_context)


@then("all volumes should be returned")
def all_volumes_should_be_returned(paginated_volumes_list):
    """all volumes should be returned."""
    created_volumes = paginated_volumes_list[0]
    listed_volumes = paginated_volumes_list[1]
    assert len(listed_volumes) == len(created_volumes)


@then(
    "a subsequent ListVolumesRequest using the next token should return the next volume"
)
def a_subsequent_listvolumesrequest_using_the_next_token_should_return_the_next_volume(
    paginated_volumes_list,
):
    """a subsequent ListVolumesRequest using the next token should return the next volume."""
    created_volumes = paginated_volumes_list[0]
    next_token = paginated_volumes_list[2]
    vols = csi_rpc_handle().controller.ListVolumes(
        pb.ListVolumesRequest(max_entries=1, starting_token=next_token)
    )
    assert len(created_volumes) == 2
    assert len(vols.entries) == 1
    # The returned volume ID should match the ID of the second created volume.
    assert created_volumes[1].volume.volume_id == vols.entries[0].volume.volume_id


@when(
    "a ControllerGetCapabilities request is sent to CSI controller",
    target_fixture="get_caps_request",
)
def get_controller_capabilities(csi_instance):
    return csi_instance.controller.ControllerGetCapabilities(
        pb.ControllerGetCapabilitiesRequest()
    )


@when("the ControllerUnpublishVolume request arrives from a non-frontend node")
def unpublish_from_non_frontend_node():
    """the ControllerUnpublishVolume request arrives from a non-frontend node."""
    do_unpublish_volume(VOLUME4_UUID, NODE2)


@then("CSI controller should report all its capabilities")
def check_get_controller_capabilities(get_caps_request):
    all_capabilities = [
        pb.ControllerServiceCapability.RPC.Type.CREATE_DELETE_VOLUME,
        pb.ControllerServiceCapability.RPC.Type.PUBLISH_UNPUBLISH_VOLUME,
        pb.ControllerServiceCapability.RPC.Type.LIST_VOLUMES,
        pb.ControllerServiceCapability.RPC.Type.GET_CAPACITY,
        pb.ControllerServiceCapability.RPC.Type.CREATE_DELETE_SNAPSHOT,
        pb.ControllerServiceCapability.RPC.Type.LIST_SNAPSHOTS,
        pb.ControllerServiceCapability.RPC.Type.EXPAND_VOLUME,
    ]

    reported_capabilities = [c.rpc.type for c in get_caps_request.capabilities]

    assert len(reported_capabilities) == len(
        all_capabilities
    ), "Wrong amount of plugin capabilities reported"

    for c in all_capabilities:
        assert c in reported_capabilities, "Capability is missing: %s" % str(c)


@when(
    "a GetCapacity request is sent to the controller",
    target_fixture="get_overall_capacity",
)
def get_overall_capacity(two_pools):
    capacity = csi_rpc_handle().controller.GetCapacity(pb.GetCapacityRequest())
    return [
        two_pools[0].state.capacity + two_pools[1].state.capacity,
        capacity.available_capacity,
    ]


@then(
    "CSI controller should report overall capacity equal to aggregated sizes of the pools"
)
def check_get_overall_capacity(get_overall_capacity):
    assert (
        get_overall_capacity[0] == get_overall_capacity[1]
    ), "Overall capacity does not match pool sizes"


@when(
    "GetCapacity request with node name is sent to the controller",
    target_fixture="get_nodes_capacity",
)
def get_node_capacity(two_pools):
    capacity = []

    for n in [NODE1, NODE2]:
        topology = pb.Topology(segments=[[OPENEBS_TOPOLOGY_KEY, n]])
        cap = csi_rpc_handle().controller.GetCapacity(
            pb.GetCapacityRequest(accessible_topology=topology)
        )
        capacity.append(cap.available_capacity)
    return capacity


@then("CSI controller should report capacity for target node")
def check_get_node_capacity(get_nodes_capacity):
    pool_api = ApiClient.pools_api()

    for i, p in enumerate([POOL1_UUID, POOL2_UUID]):
        pool = pool_api.get_pool(p)

        assert (
            pool.state.capacity == get_nodes_capacity[i]
        ), "Node pool size does not match reported node capacity"


def csi_create_1_replica_nvmf_volume1():
    capacity = pb.CapacityRange(required_bytes=VOLUME1_SIZE, limit_bytes=0)
    parameters = {
        "protocol": "nvmf",
        "ioTimeout": "30",
        "repl": "1",
        "local": "true",
    }

    req = pb.CreateVolumeRequest(
        name=PVC_VOLUME1_NAME, capacity_range=capacity, parameters=parameters
    )

    return csi_rpc_handle().controller.CreateVolume(req)


def csi_create_2_replica_nvmf_volume4():
    capacity = pb.CapacityRange(required_bytes=VOLUME4_SIZE, limit_bytes=0)
    parameters = {
        "protocol": "nvmf",
        "ioTimeout": "30",
        "repl": "2",
        "local": "true",
    }

    req = pb.CreateVolumeRequest(
        name=PVC_VOLUME4_NAME, capacity_range=capacity, parameters=parameters
    )

    return csi_rpc_handle().controller.CreateVolume(req)


def csi_create_1_replica_local_nvmf_volume():
    capacity = pb.CapacityRange(required_bytes=VOLUME3_SIZE, limit_bytes=0)
    parameters = {"protocol": "nvmf", "ioTimeout": "30", "repl": "1", "local": "true"}

    req = pb.CreateVolumeRequest(
        name=PVC_VOLUME3_NAME, capacity_range=capacity, parameters=parameters
    )

    return csi_rpc_handle().controller.CreateVolume(req)


def csi_create_1_replica_nvmf_volume2():
    capacity = pb.CapacityRange(required_bytes=VOLUME2_SIZE, limit_bytes=0)
    parameters = {
        "protocol": "nvmf",
        "ioTimeout": "30",
        "repl": "1",
        "local": "true",
    }

    req = pb.CreateVolumeRequest(
        name=PVC_VOLUME2_NAME, capacity_range=capacity, parameters=parameters
    )

    try:
        return csi_rpc_handle().controller.CreateVolume(req)
    except grpc.RpcError as e:
        return e


def csi_create_1_replica_nvmf_volume_local(local_set=False, local=False):
    capacity = pb.CapacityRange(required_bytes=VOLUME2_SIZE, limit_bytes=0)
    parameters = {
        "protocol": "nvmf",
        "ioTimeout": "30",
        "repl": "1",
    }
    if local_set and local:
        parameters["local"] = "true"
    elif local_set and not local:
        parameters["local"] = "false"

    req = pb.CreateVolumeRequest(
        name=PVC_VOLUME2_NAME, capacity_range=capacity, parameters=parameters
    )

    try:
        return csi_rpc_handle().controller.CreateVolume(req)
    except grpc.RpcError as e:
        return e


def check_nvmf_target(uri):
    """Check whether NVMF target is discoverable via target URI"""
    # Make sure URI represents nvmf target.
    assert uri, "URI must not be empty"
    assert uri.startswith("nvmf://"), "Non-nvmf protocol scheme in share URI: " + uri

    u = urlparse(uri)
    port = u.port
    host = u.hostname
    nqn = u.path[1:]
    hostnqn = parse_qs(u.query)["hostnqn"][0]
    hostid = "b7097711-a6d7-425f-a76a-9cfcf83f31ba"

    command = f"sudo {nvme_bin} discover -t tcp -s {port} -a {host} -I {hostid} -q {hostnqn} -o json"
    status = subprocess.run(
        command, shell=True, check=True, text=True, capture_output=True
    )

    # Make sure nvmf target exists.
    try:
        records = json.loads(status.stdout)
    except:
        # In case no targets are discovered, no parseable JSON exists.
        return False

    for r in records["records"]:
        if r["subnqn"] == nqn:
            return True
    return False


def csi_delete_1_replica_nvmf_volume1():
    csi_rpc_handle().controller.DeleteVolume(
        pb.DeleteVolumeRequest(volume_id=VOLUME1_UUID)
    )


def csi_delete_2_replica_nvmf_volume4():
    csi_rpc_handle().controller.DeleteVolume(
        pb.DeleteVolumeRequest(volume_id=VOLUME1_UUID)
    )


def csi_delete_1_replica_local_nvmf_volume1():
    csi_rpc_handle().controller.DeleteVolume(
        pb.DeleteVolumeRequest(volume_id=VOLUME3_UUID)
    )


def csi_delete_1_replica_nvmf_volume_local():
    csi_rpc_handle().controller.DeleteVolume(
        pb.DeleteVolumeRequest(volume_id=VOLUME2_UUID)
    )


def csi_delete_1_replica_nvmf_volume2():
    csi_rpc_handle().controller.DeleteVolume(
        pb.DeleteVolumeRequest(volume_id=VOLUME2_UUID)
    )


@pytest.fixture
def _create_1_replica_nvmf_volume():
    yield csi_create_1_replica_nvmf_volume1()
    if Cluster.fixture_cleanup():
        csi_delete_1_replica_nvmf_volume1()


@pytest.fixture
def _create_1_replica_local_nvmf_volume():
    csi_delete_1_replica_local_nvmf_volume1()
    yield csi_create_1_replica_local_nvmf_volume()
    if Cluster.fixture_cleanup():
        csi_delete_1_replica_local_nvmf_volume1()


@pytest.fixture
def _create_1_replica_nvmf_volume_local_false(context):
    csi_delete_1_replica_nvmf_volume_local()
    result = csi_create_1_replica_nvmf_volume_local(True, False)
    context["create_result"] = result
    yield result
    if Cluster.fixture_cleanup():
        csi_delete_1_replica_nvmf_volume_local()


@pytest.fixture
def _create_1_replica_nvmf_volume_local_unset(context):
    csi_delete_1_replica_nvmf_volume_local()
    result = csi_create_1_replica_nvmf_volume_local(False)
    context["create_result"] = result
    yield result
    if Cluster.fixture_cleanup():
        csi_delete_1_replica_nvmf_volume_local()


@pytest.fixture
def _create_2_volumes_1_replica():
    vol1 = csi_create_1_replica_nvmf_volume1()
    vol2 = csi_create_1_replica_nvmf_volume2()

    yield [vol1, vol2]

    if Cluster.fixture_cleanup():
        csi_delete_1_replica_nvmf_volume1()
        csi_delete_1_replica_nvmf_volume2()


@pytest.fixture(scope="function")
def context():
    return {}


@when(
    "a CreateVolume request is sent to create a 1 replica nvmf volume",
    target_fixture="create_1r_nvmf_volume",
)
def create_1_replica_nvmf_volume(_create_1_replica_nvmf_volume):
    return _create_1_replica_nvmf_volume


@then("a new volume of requested size should be successfully created")
def check_create_1_replica_nvmf_volume(create_1r_nvmf_volume):
    assert (
        create_1r_nvmf_volume.volume.capacity_bytes == VOLUME1_SIZE
    ), "Volume size mismatches"
    volume = ApiClient.volumes_api().get_volume(VOLUME1_UUID)
    assert volume.spec.num_replicas == 1, "Number of volume replicas mismatches"
    assert volume.spec.size == VOLUME1_SIZE, "Volume size mismatches"


def check_volume_context(volume_ctx, target_ctx):
    for k, v in target_ctx.items():
        assert k in volume_ctx, "Context key is missing: " + k
        assert volume_ctx[k] == v, "Context item mismatches"


@then("volume context should reflect volume creation parameters")
def check_create_1_replica_nvmf_volume_context(create_1r_nvmf_volume):
    check_volume_context(
        create_1r_nvmf_volume.volume.volume_context,
        {"protocol": "nvmf", "repl": "1", "ioTimeout": "30"},
    )


@given("an existing unpublished volume", target_fixture="existing_volume")
def an_existing_volume(_create_1_replica_nvmf_volume):
    return _create_1_replica_nvmf_volume


@given("an existing unpublished local volume", target_fixture="existing_local_volume")
def an_existing_volume(_create_1_replica_local_nvmf_volume):
    return _create_1_replica_local_nvmf_volume


@then("no topology restrictions should be imposed to volumes")
def check_no_topology_restrictions_for_volume(list_2_volumes):
    """Non local volumes not supported at the moment"""
    vols = [v for v in list_2_volumes[1] if v.volume.volume_id != VOLUME3_UUID]
    assert len(vols) == 2, "Invalid number of volumes reported"
    for v in vols:
        assert (
            len(v.volume.accessible_topology) == 1
        ), "Volume has topology restrictions"


@when(
    "a CreateVolume request is sent to create a volume identical to existing volume",
    target_fixture="create_the_same_volume",
)
def create_the_same_volume_again(existing_volume):
    return [existing_volume, csi_create_1_replica_nvmf_volume1()]


def check_volume_specs(volume1, volume2):
    assert (
        volume1.capacity_bytes == volume2.capacity_bytes
    ), "Volumes have different capacity"
    assert volume1.volume_id == volume2.volume_id, "Volumes have different UUIDs"
    assert (
        volume1.volume_context == volume2.volume_context
    ), "Volumes have different contexts"

    topology1 = sorted(
        volume1.accessible_topology, key=lambda t: t.segments[OPENEBS_TOPOLOGY_KEY]
    )

    topology2 = sorted(
        volume2.accessible_topology, key=lambda t: t.segments[OPENEBS_TOPOLOGY_KEY]
    )

    assert topology1 == topology2, "Volumes have different topologies"


@then("a CreateVolume request for the identical volume should succeed")
@then("volume context should be identical to the context of existing volume")
def check_identical_volume_creation(create_the_same_volume):
    check_volume_specs(
        create_the_same_volume[0].volume, create_the_same_volume[1].volume
    )


@then("nvmf target which exposes the volume should not be destroyed")
def volume_target_should_not_be_destroyed():
    """nvmf target which exposes the volume should not be destroyed."""
    vol = ApiClient.volumes_api().get_volume(VOLUME4_UUID)
    assert str(vol.spec.target.protocol) == "nvmf", "Volume protocol mismatches"
    assert vol.state.target["device_uri"].startswith(
        ("nvmf://", "nvmf+tcp://", "nvmf+rdma+tcp://")
    ), "Volume share URI mismatches"


@then("only a single volume should be returned")
def only_a_single_volume_should_be_returned(paginated_volumes_list):
    """only a single volume should be returned."""
    created_volumes = paginated_volumes_list[0]
    listed_volumes = paginated_volumes_list[1]
    assert len(created_volumes) == 2
    assert len(listed_volumes) == 1
    # The returned volume ID should match the ID of the first created volume.
    assert created_volumes[0].volume.volume_id == listed_volumes[0].volume.volume_id


@then("the next token should be empty")
def the_next_token_should_be_empty(paginated_volumes_list):
    """the next token should be empty."""
    next_token = paginated_volumes_list[2]
    assert next_token == ""


@when("a DeleteVolume request is sent to CSI controller to delete existing volume")
def delete_existing_volume():
    # Make sure volume does exist before removing it.
    ApiClient.volumes_api().get_volume(VOLUME1_UUID)

    csi_rpc_handle().controller.DeleteVolume(
        pb.DeleteVolumeRequest(volume_id=VOLUME1_UUID)
    )


@then("existing volume should be deleted")
def check_delete_existing_volume():
    # Make sure volume does not exist after being removed.
    with pytest.raises(NotFoundException) as e:
        ApiClient.volumes_api().get_volume(VOLUME1_UUID)


@when("a DeleteVolume request is sent to CSI controller to delete not existing volume")
def remove_not_existing_volume():
    csi_rpc_handle().controller.DeleteVolume(
        pb.DeleteVolumeRequest(volume_id=NOT_EXISTING_VOLUME_UUID)
    )


@then("a DeleteVolume request should succeed as if target volume existed")
def check_remove_not_existing_volume():
    with pytest.raises(NotFoundException) as e:
        ApiClient.volumes_api().get_volume(NOT_EXISTING_VOLUME_UUID)


@then(
    "a ControllerUnpublishVolume request should succeed as if target volume was published"
)
def check_unpublish_not_published_volume():
    pass


@then(
    "a ControllerUnpublishVolume request should succeed as if not existing volume was published"
)
def check_unpublish_not_existing_volume(unpublish_not_existing_volume):
    response = pb.ControllerUnpublishVolumeResponse()
    assert (
        response == unpublish_not_existing_volume
    ), "Volume unpuplishing succeeded with unexpected response"


@then("volume should remain as published")
def volume_remains_published():
    """volume should remain as published."""
    pass


@then("volume should report itself as published")
def check_volume_status_published():
    vol = ApiClient.volumes_api().get_volume(VOLUME1_UUID)
    assert str(vol.spec.target.protocol) == "nvmf", "Volume protocol mismatches"
    assert vol.state.target["protocol"] == "nvmf", "Volume protocol mismatches"
    assert vol.state.target["deviceUri"].startswith(
        "nvmf://"
    ), "Volume share URI mismatches"


@then("volume should report itself as not published")
def check_volume_status_not_published():
    vol = ApiClient.volumes_api().get_volume(VOLUME1_UUID)
    assert "target" not in vol.spec, "Volume still published"
