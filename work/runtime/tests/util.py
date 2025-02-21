from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
from datetime import datetime, timedelta
from os import system
from os.path import exists
from queue import Queue, Empty, ShutDown
from random import randrange
from subprocess import Popen, PIPE
from socket import socket, AF_INET, SOCK_STREAM
from tempfile import NamedTemporaryFile
from threading  import Thread
from typing import TextIO
from time import sleep
from uuid import uuid4

import docker
from docker.models.containers import Container as DockerContainer
import grpc
import requests

from work.runtime.tests.api_pb2_grpc import (
    ImageServiceServicer, ImageServiceStub,
    RuntimeServiceServicer, RuntimeServiceStub,
    add_ImageServiceServicer_to_server,
    add_RuntimeServiceServicer_to_server,
)
from work.runtime.tests.api_pb2 import (
    VersionResponse,
    RunPodSandboxResponse,
    StopPodSandboxResponse,
    RemovePodSandboxResponse,
    PodSandboxStatusResponse,
    ListPodSandboxResponse,
    CreateContainerResponse,
    StartContainerResponse,
    StopContainerResponse,
    RemoveContainerResponse,
    ListContainersResponse,
    ContainerStatusResponse,
    UpdateContainerResourcesResponse,
    ReopenContainerLogResponse,
    ExecSyncResponse,
    ExecResponse,
    AttachResponse,
    PortForwardResponse,
    ContainerStatsResponse,
    ListContainerStatsResponse,
    PodSandboxStatsResponse,
    ListPodSandboxStatsResponse,
    UpdateRuntimeConfigResponse,
    StatusResponse,
    CheckpointContainerResponse,
    ContainerEventResponse,
    ListMetricDescriptorsResponse,
    ListPodSandboxMetricsResponse,
    RuntimeConfigResponse,
    ListImagesResponse,
    ImageStatusResponse,
    PullImageResponse,
    RemoveImageResponse,
    ImageFsInfoResponse,
)

# Path to the `workd` binary in the runfiles.
_workd_path = 'work/runtime/workd'
# Path to the `vimana-push` binary which uploads Wasm containers to the registry.
_vimana_push_path = '../rules_k8s+/vimana-push'

class WorkdTester:
    """ Manager for the system under test.

    Fires up a real `workd` server
    hooked up to the [reference container registry](https://hub.docker.com/_/registry)
    and a fake OCI runtime,
    and provides clients to communicate with it.
    """

    def __init__(self):
        # Fire up an image registry, OCI runtime, and workd instance and wire them up.
        self._imageRegistry, self._imageRegistryPort = startImageRegistry()
        self._ociRuntime, ociSocket = startOciRuntime()

        # Wait for both the image registry and oci runtime to become connectable
        # before starting workd.
        _waitFor(lambda: not _isPortAvailable(self._imageRegistryPort) and exists(ociSocket))
        self._workd, self._workdSocket = startWorkd(ociSocket, self._imageRegistryPort)

        # We need a separate thread just to collect the logs:
        # https://stackoverflow.com/a/4896288/5712883.
        self._workdLogQueue = Queue()
        logThread = Thread(target=_collectLogs, args=(self._workd.stdout, self._workdLogQueue))
        logThread.daemon = True  # Shut down the thread along with the parent process.
        logThread.start()

        # Wait for workd to become connectable before opening client channels.
        _waitFor(lambda: exists(self._workdSocket))
        self._runtimeChannel = self._channel()
        self._imageChannel = self._channel()
        self.runtimeService = RuntimeServiceStub(self._runtimeChannel)
        self.imageService = ImageServiceStub(self._imageChannel)

    def _channel(self):
        # Set authority: https://github.com/grpc/grpc/issues/34305.
        return grpc.insecure_channel(
            f'unix://{self._workdSocket}',
            options=[('grpc.default_authority', 'localhost')],
        )

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_value, traceback):
        try:
            self._runtimeChannel.close()
            self._imageChannel.close()
        finally:
            try:
                self._workd.terminate()
                self._workd.wait(5)
            finally:
                try:
                    self._imageRegistry.stop()
                    self._imageRegistry.wait(timeout=5)
                finally:
                    try:
                        self._ociRuntime.stop(5)
                    finally:
                        self._workdLogQueue.shutdown()

    def pushImage(self, domain: str, service: str, version: str, component: str, metadata: str):
        """ Push a Vimana Wasm "container" image to the running container registry

        Args:
            domain:     e.g. `1234567890abcdef1234567890abcdef`
            service:    e.g. `some.package.FooService`
            version:    e.g. `1.0.0-release`
            component:  Path to compiled Wasm component byte code file.
            metadata:   Path to serialized gRPC service metadata file.
        """
        command = [
            _vimana_push_path,
            f'http://localhost:{self._imageRegistryPort}',
            domain,
            service,
            version,
            component,
            metadata,
        ]
        status = system(' '.join(map(_bashQuote, command)))
        if status != 0:
            raise RuntimeError(f'Failed to push image (status={status}).')

    def workdLogs(self) -> list[str]:
        """
        Return the list of available log lines that have been written by `workd`
        since last invocation.
        """
        # `sleep(0)` yields the GIL
        # so the background log collector thread can run if it needs to.
        sleep(0)
        logs = []
        while True:
            try:
                logs.append(self._workdLogQueue.get(block = False))
            except (Empty, ShutDown):
                return logs

def startWorkd(ociRuntimeSocket: str, imageRegistryPort: int) -> tuple[Popen, str]:
    """ Start a background process running the work node daemon.

    Return the running process and the UNIX socket path where it's listening.
    """
    socket = _tmpName()
    registry = f'http://localhost:{imageRegistryPort}'
    process = Popen(
        [_workd_path, socket, ociRuntimeSocket, registry],
        # Open a line-buffered text-mode pipe for stdout
        # and convert all CR/LF sequences to just LF.
        stdout=PIPE, text=True, bufsize=1,
    )
    return (process, socket)

def startImageRegistry() -> tuple[DockerContainer, int]:
    """ Start a docker container running the reference image registry implementation.

    Return the running container handler and the port number where it's listening.
    """
    try:
        containers = docker.from_env().containers
    except docker.errors.DockerException as error:
        raise RuntimeError('Failed to connect to Docker daemon. Is Docker running?') from error

    port = findAvailablePort()
    container = containers.run(
        'registry:latest',
        ports={'5000/tcp': port},
        detach=True,
    )
    return (container, port)

def startOciRuntime() -> tuple[grpc.Server, str]:
    """ Start a background process running a "fake" OCI container runtime.

    Return the running server and the UNIX socket path where it's listening.
    """
    socket = _tmpName()
    server = grpc.server(ThreadPoolExecutor(max_workers=1))
    add_RuntimeServiceServicer_to_server(FakeRuntimeService(), server)
    add_ImageServiceServicer_to_server(FakeImageService(), server)
    server.add_insecure_port(f'unix://{socket}')
    server.start()
    return (server, socket)

def findAvailablePort(attempts: int = 5) -> int:
    """ Find an available TCP port by random probing. """
    for i in range(attempts):
        # Pick a random port in the ephemeral range: [49152–65536).
        port = randrange(49152, 65536)
        if _isPortAvailable(port):
            return port
    raise RuntimeError(f'Could not find an open port after {attempts} attempts.')

def _collectLogs(stdout: TextIO, queue: Queue):
    """ Read all lines from the `stdout` pipe, adding each line to the queue. """
    # Invoke `readline` iteratively until EOF is indicated by the sentinel value `b''`.
    for line in iter(stdout.readline, b''):
        try:
            queue.put(line)
        except ShutDown:
            # If the test is shutting down, nobody wants the remaining logs.
            break
    stdout.close()

def _isPortAvailable(port: int) -> bool:
    with closing(socket(AF_INET, SOCK_STREAM)) as sock:
        errno = sock.connect_ex(('localhost', port))
        # Error codes 111 (on Linux) and 61 (on Mac)
        # seem to indicate connection refused (the port is available).
        if errno == 111 or errno == 61:
            return True
    return False

def _waitFor(predicate: Callable[[], bool], timeout: timedelta = timedelta(seconds=5)):
    start = datetime.now()
    while not predicate():
        if datetime.now() - start > timeout:
            raise RuntimeError('Timed out waiting for condition')
        sleep(1 / 32)  # ~30ms

def _readFile(path: str) -> bytes:
    with open(path, 'rb') as f:
        return f.read()

def _tmpName() -> str:
    """ Return a unique name for a hypothetical temporary file that does not exist. """
    f = NamedTemporaryFile()
    name = f.name
    f.close()  # Delete the file.
    return name

def _bashQuote(word: str) -> str:
    return f"'{word.replace("'", "'\"'\"'")}'"

def hexUuid() -> str:
    return uuid4().hex

class FakeRuntimeService(RuntimeServiceServicer):
    """ Fake implementation of the CRI API's `RuntimeService` that does nothing. """

    def Version(self, request, context): return VersionResponse()
    def RunPodSandbox(self, request, context): return RunPodSandboxResponse()
    def StopPodSandbox(self, request, context): return StopPodSandboxResponse()
    def RemovePodSandbox(self, request, context): return RemovePodSandboxResponse()
    def PodSandboxStatus(self, request, context): return PodSandboxStatusResponse()
    def ListPodSandbox(self, request, context): return ListPodSandboxResponse()
    def CreateContainer(self, request, context): return CreateContainerResponse()
    def StartContainer(self, request, context): return StartContainerResponse()
    def StopContainer(self, request, context): return StopContainerResponse()
    def RemoveContainer(self, request, context): return RemoveContainerResponse()
    def ListContainers(self, request, context): return ListContainersResponse()
    def ContainerStatus(self, request, context): return ContainerStatusResponse()
    def UpdateContainerResources(self, request, context): return UpdateContainerResourcesResponse()
    def ReopenContainerLog(self, request, context): return ReopenContainerLogResponse()
    def ExecSync(self, request, context): return ExecSyncResponse()
    def Exec(self, request, context): return ExecResponse()
    def Attach(self, request, context): return AttachResponse()
    def PortForward(self, request, context): return PortForwardResponse()
    def ContainerStats(self, request, context): return ContainerStatsResponse()
    def ListContainerStats(self, request, context): return ListContainerStatsResponse()
    def PodSandboxStats(self, request, context): return PodSandboxStatsResponse()
    def ListPodSandboxStats(self, request, context): return ListPodSandboxStatsResponse()
    def UpdateRuntimeConfig(self, request, context): return UpdateRuntimeConfigResponse()
    def Status(self, request, context): return StatusResponse()
    def CheckpointContainer(self, request, context): return CheckpointContainerResponse()
    def GetContainerEvents(self, request, context): return ContainerEventResponse()
    def ListMetricDescriptors(self, request, context): return ListMetricDescriptorsResponse()
    def ListPodSandboxMetrics(self, request, context): return ListPodSandboxMetricsResponse()
    def RuntimeConfig(self, request, context): return RuntimeConfigResponse()

class FakeImageService(ImageServiceServicer):
    """ Fake implementation of the CRI API's `ImageService` that does nothing. """

    def ListImages(self, request, context): return ListImagesResponse()
    def ImageStatus(self, request, context): return ImageStatusResponse()
    def PullImage(self, request, context): return PullImageResponse()
    def RemoveImage(self, request, context): return RemoveImageResponse()
    def ImageFsInfo(self, request, context): return ImageFsInfoResponse()
