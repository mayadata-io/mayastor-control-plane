import os
import subprocess
from datetime import datetime

import pytest
from dataclasses import dataclass

import common
from common.docker import Docker
from common.nvme import nvme_disconnect_allours_wait


@dataclass
class StartOptions:
    io_engines: int = 1
    wait: str = "10s"
    csi_controller: bool = False
    csi_node: bool = False
    reconcile_period: str = ""
    faulted_child_wait_period: str = ""
    cache_period: str = ""
    io_engine_env: str = ""
    agents_env: str = ""
    node_deadline: str = ""
    node_conn_timeout: str = ""
    jaeger: bool = True
    cluster_uid: str = "bdd"
    extra_args: [str] = ()
    rest_env: str = ""
    max_rebuilds: str = ""
    ha_node_agent: bool = False
    ha_cluster_agent: bool = False
    ha_cluster_agent_fast: str = None
    fio_spdk: bool = False
    io_engine_coreisol: bool = False
    io_engine_devices: [str] = ()
    request_timeout: str = ""
    no_min_timeouts: bool = False

    def args(self):
        args = [
            "--io-engines",
            str(self.io_engines),
            "--wait-timeout",
            self.wait,
        ]
        if self.csi_controller:
            args.append("--csi-controller")
        if self.csi_node:
            args.append("--csi-node")
        if self.jaeger:
            args.append("--jaeger")
        if len(self.reconcile_period) > 0:
            args.append(f"--reconcile-period={self.reconcile_period}")
            args.append(f"--reconcile-idle-period={self.reconcile_period}")
        if len(self.faulted_child_wait_period) > 0:
            args.append(f"--faulted-child-wait-period={self.faulted_child_wait_period}")
        if len(self.cache_period) > 0:
            args.append(f"--cache-period={self.cache_period}")
        if len(self.node_deadline) > 0:
            args.append(f"--node-deadline={self.node_deadline}")
        if len(self.node_conn_timeout) > 0:
            args.append(f"--node-conn-timeout={self.node_conn_timeout}")
        if len(self.io_engine_env) > 0:
            args.append(f"--io-engine-env={self.io_engine_env}")
        if len(self.agents_env) > 0:
            args.append(f"--agents-env={self.agents_env}")
        if len(self.cluster_uid) > 0:
            args.append(f"--cluster-uid={self.cluster_uid}")
        if len(self.rest_env) > 0:
            args.append(f"--rest-env={self.rest_env}")
        if len(self.extra_args) > 0:
            args.append(self.extra_args)
        if len(self.max_rebuilds) > 0:
            args.append(f"--max-rebuilds={self.max_rebuilds}")
        if self.fio_spdk:
            args.append("--fio-spdk")
        if self.io_engine_coreisol:
            args.append("--io-engine-isolate")
        for device in self.io_engine_devices:
            args.append(f"--io-engine-devices={device}")
        if len(self.request_timeout) > 0:
            args.append(f"--request-timeout={self.request_timeout}")
        if self.no_min_timeouts:
            args.append(f"--no-min-timeouts")

        agent_arg = "--agents=Core"
        if self.ha_node_agent:
            agent_arg += ",HaNode"
        if self.ha_cluster_agent:
            agent_arg += ",HaCluster"
            if self.ha_cluster_agent_fast is not None:
                args.append(f"--cluster-fast-requeue={self.ha_cluster_agent_fast}")
        args.append(agent_arg)

        return args


class Deployer(object):
    # Start containers with the provided arguments
    @staticmethod
    def start(
        io_engines=2,
        wait="10s",
        csi_controller=False,
        csi_node=False,
        reconcile_period="",
        faulted_child_wait_period="",
        cache_period="",
        io_engine_env="",
        agents_env="",
        rest_env="",
        node_deadline="",
        node_conn_timeout="",
        jaeger=True,
        max_rebuilds="",
        cluster_agent=False,
        cluster_agent_fast=None,
        node_agent=False,
        fio_spdk=False,
        io_engine_coreisol=False,
        io_engine_devices=[],
        request_timeout="",
        no_min_timeouts=False,
    ):
        options = StartOptions(
            io_engines,
            wait,
            csi_controller=csi_controller,
            csi_node=csi_node,
            reconcile_period=reconcile_period,
            faulted_child_wait_period=faulted_child_wait_period,
            cache_period=cache_period,
            io_engine_env=io_engine_env,
            agents_env=agents_env,
            rest_env=rest_env,
            node_deadline=node_deadline,
            node_conn_timeout=node_conn_timeout,
            jaeger=jaeger,
            max_rebuilds=max_rebuilds,
            ha_node_agent=node_agent,
            ha_cluster_agent=cluster_agent,
            ha_cluster_agent_fast=cluster_agent_fast,
            fio_spdk=fio_spdk,
            io_engine_coreisol=io_engine_coreisol,
            io_engine_devices=io_engine_devices,
            request_timeout=request_timeout,
            no_min_timeouts=no_min_timeouts,
        )
        pytest.deployer_options = options
        Deployer.start_with_opts(options)

    # Start containers with the provided options.
    @staticmethod
    def start_with_opts(options: StartOptions):
        print(f"DeployerStart: {datetime.now()}")
        deployer_path = os.environ["ROOT_DIR"] + "/target/debug/deployer"
        # todo: get logs out to specific location
        subprocess.run([deployer_path, "start"] + options.args(), check=True)

    # Stop containers
    @staticmethod
    def stop(disconnect_nvme=False):
        print(f"DeployerStop: {datetime.now()}")
        clean = os.getenv("CLEAN")
        if clean is not None and clean.lower() in ("no", "false", "f", "0"):
            return
        if disconnect_nvme:
            nvme_disconnect_allours_wait()
        deployer_path = os.environ["ROOT_DIR"] + "/target/debug/deployer"
        subprocess.run([deployer_path, "stop"])

    @staticmethod
    def node_name(id: int):
        assert id >= 0
        return f"io-engine-{id + 1}"

    @staticmethod
    def create_disks(len=1, size=100 * 1024 * 1024):
        host_tmp = workspace_tmp()
        disks = list(map(lambda x: f"disk_{x}.img", range(1, len + 1)))
        for disk in disks:
            disk = f"{host_tmp}/{disk}"
            if os.path.exists(disk):
                os.remove(disk)
            with open(disk, "w") as file:
                file.truncate(size)
        # /tmp is mapped into /host/tmp within the io-engine containers
        return list(map(lambda file: f"/host/tmp/{file}", disks))

    @staticmethod
    def delete_disks(len=1):
        host_tmp = workspace_tmp()
        disks = list(map(lambda x: f"{host_tmp}/disk_{x}.img", range(1, len + 1)))
        for disk in disks:
            if os.path.exists(disk):
                os.remove(disk)

    @staticmethod
    def cleanup_disks(len=1):
        if common.env_cleanup():
            Deployer.delete_disks(len)

    @staticmethod
    def cache_period():
        return pytest.deployer_options.cache_period

    @staticmethod
    def restart_node(node_name):
        Docker.restart_container(node_name)


def workspace_tmp():
    root = os.getenv("WORKSPACE_ROOT")
    path = f"{root}/.tmp"
    os.makedirs(path, exist_ok=True)
    return path
