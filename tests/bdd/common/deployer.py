import os
import subprocess

from dataclasses import dataclass


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
        jaeger=True,
        max_rebuilds="",
        cluster_agent=False,
        cluster_agent_fast=None,
        node_agent=False,
        fio_spdk=False,
        io_engine_coreisol=False,
        io_engine_devices=[],
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
            jaeger=jaeger,
            max_rebuilds=max_rebuilds,
            ha_node_agent=node_agent,
            ha_cluster_agent=cluster_agent,
            ha_cluster_agent_fast=cluster_agent_fast,
            fio_spdk=fio_spdk,
            io_engine_coreisol=io_engine_coreisol,
            io_engine_devices=io_engine_devices,
        )
        Deployer.start_with_opts(options)

    # Start containers with the provided options.
    @staticmethod
    def start_with_opts(options: StartOptions):
        deployer_path = os.environ["ROOT_DIR"] + "/target/debug/deployer"
        subprocess.run([deployer_path, "start"] + options.args())

    # Stop containers
    @staticmethod
    def stop():
        clean = os.getenv("CLEAN")
        if clean is not None and clean.lower() in ("no", "false", "f", "0"):
            return
        deployer_path = os.environ["ROOT_DIR"] + "/target/debug/deployer"
        subprocess.run([deployer_path, "stop"])
