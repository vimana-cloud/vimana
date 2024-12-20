from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from functools import partial
from http.server import BaseHTTPRequestHandler, HTTPServer
from multiprocessing import Process
from random import randrange
from re import compile as compile_re
from subprocess import Popen
from tempfile import NamedTemporaryFile
from time import sleep

import grpc

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

class WorkdTester:
    """ Manager for the system under test.
    
    Fires up a real `workd` server
    hooked up to a fake image registry and OCI runtime,
    and provides clients to communicate with it.
    """

    def __init__(self):
        # Fire up an OCI runtime, image registry, and workd instance and wire them up.
        self._ociRuntime, ociSocket = startOciRuntime()
        self._imageRegistry, imageRegistryPort = startImageRegistry()
        self._workd, self._workdSocket = startWorkd(ociSocket, imageRegistryPort)

        # Wait a bit for the `workd` server to be ready, then open clients to it.
        sleep(0.25)  # TODO: "Wait" more robustly.
        self._runtimeChannel = self.channel()
        self._imageChannel = self.channel()
        self.runtimeService = RuntimeServiceStub(self._runtimeChannel)
        self.imageService = ImageServiceStub(self._imageChannel)

    def channel(self):
        # Set authority: https://github.com/grpc/grpc/issues/34305.
        return grpc.insecure_channel(
            f'unix://{self._workdSocket}',
            options=[('grpc.default_authority', 'localhost')],
        )
    
    def __del__(self):
        try:
            self._runtimeChannel.close()
            self._imageChannel.close()
        finally:
            try:
                self._workd.terminate()
                self._workd.wait(5)
            finally:
                try:
                    self._imageRegistry.terminate()
                    self._imageRegistry.join(5)
                finally:
                    self._ociRuntime.stop(5)


# Path to the `workd` binary in the runfiles.
_workd_path = 'work/runtime/workd'

def startWorkd(ociRuntimeSocket: str, imageRegistryPort: int) -> tuple[Popen, str]:
    """ Start a background process running the work node daemon.
    
    Return the running process and the UNIX socket path where it's listening.
    """
    socket = _tmpName()
    registry = f'http://localhost:{imageRegistryPort}'
    process = Popen([_workd_path, socket, ociRuntimeSocket, registry])
    return (process, socket)

def startImageRegistry() -> tuple[Process, int]:
    """ Start a background process running a "fake" image registry server.
    
    Return the running process and the port number where it's listening.
    """
    # Pick a random port in the ephemeral range: [49152–65536).
    port = randrange(49152, 65536)
    process = Process(target=_runImageRegistry, args=('localhost', port))
    process.start()
    return (process, port)

def _runImageRegistry(host, port):
    """ Run a fake image registry on the given host and port.
    
    Never return.
    """
    state = ImageRegistryState.new()  # A single shared object holds all state.
    handler = partial(ImageRegistryHttpHandler, state)
    with HTTPServer((host, port), handler) as registry:
        registry.serve_forever()

# Regular expressions used in the fake image registry
# are pre-compiled globally for performance.
_manifest_regex = compile_re(r'/v2/(?P<name>.*)/manifests/(?P<reference>.*)')
_manifest_template = '/v2/{}/manifests/{}'
_blob_pull_regex = compile_re(r'^/v2/(?P<name>.*)/blobs/(?P<digest>.*)$')
_blob_pull_template = '/v2/{}/blobs/{}'
_blob_post_regex = compile_re(r'^/v2/(?P<name>.*)/blobs/uploads/$')
_blob_put_regex = compile_re(r'^/~upload-blob/(?P<name>.*)?digest=(?P<digest>.*)$')
_blob_put_template = '/~upload-blob/{}'

class ImageRegistryHttpHandler(BaseHTTPRequestHandler):
    """ Fake image registry. """
    
    def __init__(self, state):
        self.state = state

    def do_GET(self):
        content = None
        # https://specs.opencontainers.org/distribution-spec/#pulling-manifests.
        if (match := _manifest_regex.match(self.path)) is not None:
            name = match.group('name')
            reference = match.group('reference')
            content = self.state.manifests[(name, reference)]
            if content is None:
                self.send_response(404)
            else:
                self.send_response(200)
                self.send_header('Docker-Content-Digest', 'TODO')
        # https://specs.opencontainers.org/distribution-spec/#pulling-blobs.
        elif (match := _blob_pull_regex.match(self.path)) is not None:
            name = match.group('name')
            digest = match.group('digest')
            content = self.state.blobs[(name, digest)]
            if content is None:
                self.send_response(404)
            else:
                self.send_response(200)
                self.send_header('Docker-Content-Digest', digest)
        # Default to 400 if the path does not make sense.
        else:
            self.send_response(400)
        self.end_headers()
        # Any content must be written after the headers.
        if content is not None:
            self.wfile.write(content)

    def do_POST(self):
        # First half of https://specs.opencontainers.org/distribution-spec/#post-then-put.
        if (match := _blob_post_regex.match(self.path)) is not None:
            name = match.group('name')
            self.send_response(202)
            self.send_header('Location', _blob_put_template.format(name))
        # Default to 400 if the path does not make sense.
        else:
            self.send_response(400)
        self.end_headers()
    
    def do_PUT(self):
        # https://specs.opencontainers.org/distribution-spec/#pushing-manifests.
        if (match := _manifest_regex.match(self.path)) is not None:
            name = match.group('name')
            reference = match.group('reference')
            content_type = self.headers['Content-Type']
            if content_type != 'application/vnd.oci.image.manifest.v1+json':
                self.send_response(400)
            else:
                # TODO: Check that all blobs referenced in manifest exist.
                self.state.manifests[(name, reference)] = self.rfile.read()
                self.send_response(201)
                self.send_header('Location', _manifest_template.format(name, reference))
        # Second half of https://specs.opencontainers.org/distribution-spec/#post-then-put.
        elif (match := _blob_put_regex.match(self.path)) is not None:
            name = match.group('name')
            digest = match.group('digest')
            content_length = self.headers['Content-Length']
            content_type = self.headers['Content-Type']
            content = self.rfile.read()
            if content_length != len(content) or content_type != 'application/octet-stream':
                self.send_response(400)
            else:
                # TODO: Check digest against content.
                self.state.blobs[(name, digest)] = content
                self.send_response(201)
                self.send_header('Location', _blob_pull_template.format(name, digest))
        # Default to 400 if the path does not make sense.
        else:
            self.send_response(400)
        self.end_headers()

@dataclass
class ImageRegistryState:
    """ Persistent object that defines the entire state of a fake image registry. """

    # Mapping from (name, reference) to manifest.
    manifests: dict[tuple[str, str], str]
    # Mapping from (name, digest) to blob.
    blobs: dict[tuple[str, str], bytes]

    @classmethod
    def new(cls):
        """ Return an empty state object ready to use in a new registry. """
        return cls(manifests={}, blobs={})

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

def _tmpName() -> str:
    """ Return a unique name for a hypothetical temporary file that does not exist. """
    f = NamedTemporaryFile()
    name = f.name
    f.close()  # Delete the file.
    return name