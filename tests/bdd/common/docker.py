from datetime import datetime
import docker
import os


class Docker(object):
    # Determines if a container with the given name is running.
    @staticmethod
    def check_container_running(container_name):
        docker_client = docker.from_env()
        try:
            container = docker_client.containers.get(container_name)
        except docker.errors.NotFound as exc:
            raise Exception("{} container not found", container_name)
        else:
            container_state = container.attrs["State"]
            if container_state["Status"] != "running":
                raise Exception("{} container not running", container_name)

    # Get the status of the container with the given name
    @staticmethod
    def container_status(container_name):
        docker_client = docker.from_env()
        try:
            container = docker_client.containers.get(container_name)
        except docker.errors.NotFound as exc:
            raise Exception("{} container not found", container_name)
        else:
            container_state = container.attrs["State"]
            return container_state["Status"]

    @staticmethod
    def container_ip(container_name):
        docker_client = docker.from_env()
        try:
            container = docker_client.containers.get(container_name)
        except docker.errors.NotFound as exc:
            raise Exception("{} container not found", container_name)
        else:
            return container.attrs["NetworkSettings"]["Networks"]["cluster"][
                "IPAddress"
            ]

    # Kill a container with the given name.
    @staticmethod
    def kill_container(name):
        docker_client = docker.from_env()
        container = docker_client.containers.get(name)
        container.kill()

    # Stop a container with the given name.
    @staticmethod
    def stop_container(name):
        docker_client = docker.from_env()
        container = docker_client.containers.get(name)
        container.stop()

    # Pause a container with the given name.
    @staticmethod
    def pause_container(name):
        docker_client = docker.from_env()
        container = docker_client.containers.get(name)
        container.pause()

    # Unpause a container with the given name.
    @staticmethod
    def unpause_container(name):
        docker_client = docker.from_env()
        container = docker_client.containers.get(name)
        container.unpause()

    @staticmethod
    def execute(name, commands):
        docker_client = docker.from_env()
        container = docker_client.containers.get(name)
        return container.exec_run(commands)

    # Restart a container with the given name.
    def restart_container(name):
        docker_client = docker.from_env()
        container = docker_client.containers.get(name)
        container.restart()

    @staticmethod
    def log_containers():
        failed_logs_var = "FAILED_DOCKER_LOGS"
        if failed_logs_var in os.environ and os.environ[failed_logs_var]:
            docker_client = docker.from_env()
            current_test = os.environ.get("PYTEST_CURRENT_TEST")
            logs = os.environ.get(failed_logs_var)
            with open(logs, "a") as log_file:
                log_file.write(f"{datetime.now()}: Logs for Test {current_test}:\n")
                log_file.write("-" * 40 + "\n\n")
                for container in docker_client.containers.list():
                    log_file.write(f"Logs for container {container.name}:\n")
                    log_file.write("-" * 40 + "\n")
                    logs = container.logs().decode("utf-8")
                    log_file.write(logs)
                    log_file.write("\n\n")
                log_file.write(f"End of Logs for Test {current_test}:\n")
                log_file.write("-" * 40 + "\n\n\n")
