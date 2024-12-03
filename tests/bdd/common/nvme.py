from datetime import datetime
from shutil import which
from urllib.parse import urlparse, parse_qs, ParseResult
from retrying import retry

import common
from common.command import run_cmd_async_at

import subprocess
import json

nvme_bin = which("nvme")


def product_uuid():
    command = "sudo -E cat /sys/class/dmi/id/product_uuid"
    return subprocess.run(
        command, check=True, shell=True, capture_output=True, encoding="utf-8"
    ).stdout.rstrip()


hostid = product_uuid()


async def nvme_remote_connect(remote, uri):
    """Connect to the remote nvmf target on this host."""
    u = urlparse(uri)
    port = u.port
    host = u.hostname
    nqn = u.path[1:]
    hostnqn = nvme_hostnqn_arg(u)

    command = f"sudo {nvme_bin} connect -t tcp -s {port} -a {host} -n {nqn} -I {hostid}{hostnqn}"

    await run_cmd_async_at(remote, command)
    return wait_nvme_find_device(uri)


async def nvme_remote_disconnect(remote, uri):
    """Disconnect the given URI on this host."""
    u = urlparse(uri)
    nqn = u.path[1:]

    command = f"sudo {nvme_bin} disconnect -n {nqn}"
    await run_cmd_async_at(remote, command)


async def nvme_remote_discover(remote, uri):
    """Discover target."""
    u = urlparse(uri)
    port = u.port
    host = u.hostname
    hostnqn = nvme_hostnqn_arg(u)

    command = (
        f"sudo {nvme_bin} discover -t tcp -s {port} -a {host} -I {hostid}{hostnqn}"
    )
    output = await run_cmd_async_at(remote, command).stdout
    if not u.path[1:] in str(output.stdout):
        raise ValueError("uri {} is not discovered".format(u.path[1:]))


def nvme_hostnqn_arg(uri: ParseResult):
    uri_query = parse_qs(uri.query)
    if uri_query.keys().__contains__("hostnqn"):
        return " -q {}".format(uri_query["hostnqn"][0])
    else:
        return ""


def nvme_connect(uri):
    u = urlparse(uri)
    port = u.port
    host = u.hostname
    nqn = u.path[1:]
    hostnqn = nvme_hostnqn_arg(u)

    command = f"sudo {nvme_bin} connect -t tcp -s {port} -a {host} -I {hostid}{hostnqn} -n {nqn}"
    print(command)
    try:
        subprocess.run(
            command, check=True, shell=True, capture_output=True, encoding="utf-8"
        )
    except subprocess.CalledProcessError as e:
        # todo: handle this better!
        if "already connected\n" in e.stderr or "already connnected\n" in e.stderr:
            print(f"{datetime.now()} {uri} is already connected")
            pass
        else:
            raise e

    try:
        return wait_nvme_find_device(uri)
    except Exception as e:
        print(
            f"{datetime.now()} Failed to wait for connected device {uri}, disconnecting..."
        )
        nvme_disconnect(uri)
        raise e


def nvme_id_ctrl(device):
    """Identify controller."""
    command = f"sudo {nvme_bin} id-ctrl {device} -I {hostid} -o json"
    id_ctrl = json.loads(
        subprocess.run(
            command, shell=True, check=True, text=True, capture_output=True
        ).stdout
    )

    return id_ctrl


def nvme_resv_report(device):
    """Reservation report."""
    command = f"sudo {nvme_bin} resv-report {device} -c 1 -I {hostid} -o json"
    resv_report = json.loads(
        subprocess.run(
            command, shell=True, check=True, text=True, capture_output=True
        ).stdout
    )

    return resv_report


def nvme_discover(uri):
    """Discover target."""
    u = urlparse(uri)
    port = u.port
    host = u.hostname
    hostnqn = nvme_hostnqn_arg(u)

    command = (
        f"sudo {nvme_bin} discover -t tcp -s {port} -a {host} -I {hostid}{hostnqn}"
    )
    output = subprocess.run(
        command, check=True, shell=True, capture_output=True, encoding="utf-8"
    )
    if not u.path[1:] in str(output.stdout):
        raise ValueError("uri {} is not discovered".format(u.path[1:]))


@retry(wait_fixed=100, stop_max_attempt_number=50)
def nvme_wait_allours_disconnected():
    devs = nvme_find_subsystem_devices(common.nvme_nqn_prefix)
    log = f"{datetime.now()} => Existing devices:\n{devs}"
    assert len(devs) == 0, log


def nvme_find_subsystem(uri, nqn=None):
    if nqn is None:
        u = urlparse(uri)
        nqn = u.path[1:]

    devs = nvme_find_subsystem_devices(nqn)
    log = f"{datetime.now()} => {uri}:\n{devs}"

    # we should only have one connection
    assert len(devs) == 1, log
    dev = devs[0]

    subsystems = dev.get("Subsystems")
    assert len(subsystems) == 1, log

    return subsystems[0]


def nvme_find_subsystem_devices_all():
    command = f"sudo {nvme_bin} list -v -o json"
    discover = json.loads(
        subprocess.run(
            command, shell=True, check=True, text=True, capture_output=True
        ).stdout
    )
    return discover.get("Devices")


def nvme_find_subsystem_devices(nqn):
    command = f"sudo {nvme_bin} list -v -o json"
    discover = json.loads(
        subprocess.run(
            command, shell=True, check=True, text=True, capture_output=True
        ).stdout
    )

    devs = list()
    for dev in discover.get("Devices"):
        subsystems = list()
        for subsystem in dev.get("Subsystems"):
            if nqn in subsystem.get("SubsystemNQN"):
                if (
                    subsystem.get("Controllers") is not None
                    and len(subsystem.get("Controllers")) > 0
                ):
                    subsystems.append(subsystem)
        if len(subsystems) > 0:
            dev["Subsystems"] = subsystems
            devs.append(dev)

    return devs


def nvme_find_device(uri):
    subsystem = nvme_find_subsystem(uri)
    log = f"{datetime.now()} => {uri}:\n{subsystem}"

    namespaces = subsystem["Namespaces"]
    assert len(namespaces) == 1, log

    return "/dev/{}".format(namespaces[0].get("NameSpace"))


def nvme_find_controller(uri):
    subsystem = nvme_find_subsystem(uri)
    log = f"{datetime.now()} => {uri}:\n{subsystem}"

    controllers = subsystem["Controllers"]
    assert len(controllers) == 1, log
    return controllers[0]


def nvme_find_controllers(nqn):
    subsystem = nvme_find_subsystem(None, nqn)
    return subsystem["Controllers"]


def nvme_find_device_path(uri):
    controller = nvme_find_controller(uri)
    log = f"{datetime.now()} => {uri}:\n{controller}"

    paths = controller.get("Paths")
    assert len(paths) == 1, log
    path = paths[0]

    return path.get("Path")


def nvme_disconnect(uri):
    """Disconnect the given URI on this host."""
    u = urlparse(uri)
    nqn = u.path[1:]

    nvme_disconnect_nqn(nqn)


def nvme_disconnect_nqn(nqn):
    command = f"sudo {nvme_bin} disconnect -n {nqn}"
    print(command)
    subprocess.run(command, check=True, shell=True, capture_output=True)


def nvme_disconnect_all():
    """Disconnect from all connected nvme subsystems"""
    command = f"sudo {nvme_bin} disconnect-all"
    subprocess.run(command, check=True, shell=True, capture_output=True)


def nvme_disconnect_allours():
    devs = nvme_find_subsystem_devices(common.nvme_nqn_prefix)
    for dev in devs:
        for subsystem in dev.get("Subsystems"):
            nqn = subsystem.get("SubsystemNQN")
            nvme_set_reconnect_delay_all(nqn, delay=1)
            nvme_disconnect_nqn(nqn)


def nvme_disconnect_allours_wait():
    nvme_disconnect_allours()
    nvme_wait_allours_disconnected()


def nvme_disconnect_controller(name):
    """Disconnect the given NVMe controller on this host."""
    command = f"sudo {nvme_bin} disconnect -d {name}"
    subprocess.run(command, check=True, shell=True, capture_output=True)


def nvme_list_subsystems(device):
    """Retrieve information for NVMe subsystems"""
    command = f"sudo {nvme_bin} list-subsys {device} -o json"
    subsystems = json.loads(
        subprocess.run(
            command, check=True, shell=True, capture_output=True, encoding="utf-8"
        ).stdout
    )
    return list(filter(lambda s: len(s["Subsystems"]) == 1, subsystems))[0]


NS_PROPS = ["nguid", "eui64"]


def identify_namespace(device):
    """Get properties of a namespace on this host"""
    command = f"sudo {nvme_bin} id-ns {device}"
    output = subprocess.run(
        command, check=True, shell=True, capture_output=True, encoding="utf-8"
    )
    props = output.stdout.strip().split("\n")[1:]
    ns = {}
    for p in props:
        v = [v.strip() for v in p.split(":") if p.count(":") == 1]
        if len(v) == 2 and v[0] in NS_PROPS:
            ns[v[0]] = v[1]
    return ns


def nvme_set_reconnect_delay_all(nqn, delay=10):
    for controller in nvme_find_controllers(nqn):
        nvme_controller_set_param(controller, "reconnect_delay", delay)


def nvme_set_reconnect_delay(uri, delay=10):
    controller = nvme_find_controller(uri)
    nvme_controller_set_param(controller, "reconnect_delay", delay)


def nvme_set_ctrl_loss_tmo(uri, ctrl_loss_tmo=10):
    controller = nvme_find_controller(uri)
    nvme_controller_set_param(controller, "ctrl_loss_tmo", ctrl_loss_tmo)


def nvme_controller_set_param(controller, name: str, value: int):
    controller = controller.get("Controller")
    command = f"echo {value} | sudo tee -a /sys/class/nvme/{controller}/{name}"
    print(command)
    subprocess.run(command, check=True, shell=True, capture_output=True)


@retry(wait_fixed=100, stop_max_attempt_number=40)
def wait_nvme_find_device(uri):
    return nvme_find_device(uri)


@retry(wait_fixed=100, stop_max_attempt_number=40)
def wait_nvme_gone_device(uri, nqn=None):
    if nqn is None:
        u = urlparse(uri)
        nqn = u.path[1:]
    devs = nvme_find_subsystem_devices(nqn)
    assert len(devs) == 0
