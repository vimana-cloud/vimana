"""Test harness and helper functions for the work runtime."""

from collections import defaultdict
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
from datetime import datetime, timedelta
from functools import partial
from hashlib import sha256
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, HTTPServer
from ipaddress import IPv4Address, IPv6Address
from itertools import chain
from json import loads as parseJson
from os import chmod, getpid, stat, walk
from os.path import exists, join
from queue import Empty, Queue, ShutDown
from random import randrange
from re import Match
from re import compile as compileRegex
from shlex import quote
from socket import AF_INET, SOCK_STREAM, socket
from stat import S_IEXEC, S_IREAD
from subprocess import PIPE, Popen
from sys import stderr
from tempfile import NamedTemporaryFile, TemporaryDirectory
from threading import Thread
from time import sleep
from typing import Any, TextIO
from unittest import TestCase
from uuid import uuid4

import grpc

from work.runtime.tests.api_pb2 import (
    AttachResponse,
    CheckpointContainerResponse,
    ContainerEventResponse,
    ContainerStatsResponse,
    ContainerStatusResponse,
    CreateContainerResponse,
    ExecResponse,
    ExecSyncResponse,
    ImageFsInfoRequest,
    ImageFsInfoResponse,
    ImageSpec,
    ImageStatusResponse,
    ListContainersResponse,
    ListContainerStatsResponse,
    ListImagesResponse,
    ListMetricDescriptorsResponse,
    ListPodSandboxMetricsResponse,
    ListPodSandboxResponse,
    ListPodSandboxStatsResponse,
    PodSandboxConfig,
    PodSandboxStatsResponse,
    PodSandboxStatusResponse,
    PortForwardResponse,
    PullImageRequest,
    PullImageResponse,
    RemoveContainerResponse,
    RemoveImageResponse,
    RemovePodSandboxResponse,
    ReopenContainerLogResponse,
    RunPodSandboxResponse,
    RuntimeConfigResponse,
    StartContainerResponse,
    StatusResponse,
    StopContainerResponse,
    StopPodSandboxResponse,
    UpdateContainerResourcesResponse,
    UpdateRuntimeConfigResponse,
    VersionResponse,
)
from work.runtime.tests.api_pb2_grpc import (
    ImageServiceServicer,
    ImageServiceStub,
    RuntimeServiceServicer,
    RuntimeServiceStub,
    add_ImageServiceServicer_to_server,
    add_RuntimeServiceServicer_to_server,
)

# Path to the `workd` binary in the runfiles.
_workdPath = 'work/runtime/workd'
# Path to the `host-local` IPAM emulator.
_ipamPath = 'work/runtime/tests/ipam'
# Path to the `vimana-push` binary which uploads Wasm containers to the registry.
_pushImagePath = 'bootstrap/push-image'

# Generally wait up to 5 seconds for things to happen asynchronously.
_timeout = timedelta(seconds=5)

# Create a temporary IPAM database file
# and "wrap" the IPAM executable with a temporary script
# that passes the database path as an argument.
# This allows tests running in parallel to have independent IPAM systems.
#
# There should be precisely one IPAM system per Bazel test target
# because the network namespace is partitioned by Bazel,
# hence using global variables to manage the temporary file lifecycle.
_ipamDatabase = NamedTemporaryFile()
_ipamWrapper = NamedTemporaryFile(mode='w', delete_on_close=False)
_ipamWrapper.write(f"""#!/usr/bin/env bash
exec {quote(_ipamPath)} {quote(_ipamDatabase.name)}
""")
_ipamWrapper.close()
chmod(_ipamWrapper.name, S_IEXEC | S_IREAD)

# The name of the Vimana Work runtime.
RUNTIME_NAME = 'workd'


class WorkdTestCase(TestCase):
    @classmethod
    def setUpClass(cls):
        # A single, long-running runtime instance is available to all tests.
        # Otherwise, any test that requires isolation can simply spin up it's own `WorkdTester`.
        cls.tester = WorkdTester().__enter__()
        # Set up convenient aliases for fields in `tester`.
        cls.runtimeService = cls.tester.runtimeService
        cls.imageService = cls.tester.imageService
        cls.setupImage = cls.tester.setupImage
        cls.imageId = cls.tester.imageId

    @classmethod
    def tearDownClass(cls):
        # Shut down the various servers and subprocesses.
        cls.tester.__exit__(None, None, None)

    def setUp(self):
        self.verifyFsUsage = partial(self.tester.verifyFsUsage, self)

    def tearDown(self):
        self.tester.printWorkdLogs(self)


class WorkdTester:
    """Manager for the system under test.

    Fires up a real `workd` server hooked up to dependencies:
    - A mock container image registry
      that should act like the [reference implementation](https://hub.docker.com/_/registry).
    - A fake OCI runtime that does nothing.
    - An emulator for the host-local IPAM plugin.

    Also provides clients to communicate with the `workd` server.
    """

    def __init__(self):
        # Fire up an image registry, OCI runtime, and workd instance and wire them up.
        self._imageRegistry, self._imageRegistryPort = startImageRegistry()
        try:
            # Start a fake downstream OCI runtime (normally, this would be containerd).
            self._ociRuntime, ociSocket = startOciRuntime()
            try:
                # Wait for both the image registry and oci runtime to become connectable
                # before starting workd.
                _waitFor(
                    lambda: exists(ociSocket)
                    and not _isPortAvailable(self._imageRegistryPort),
                )
                self._imageStore = TemporaryDirectory()
                self._workd, self._workdSocket = startWorkd(
                    ociSocket,
                    self._imageRegistryPort,
                    self._imageStore.name,
                    _ipamWrapper.name,
                )
                try:
                    # We need a separate thread just to collect the logs:
                    # https://stackoverflow.com/a/4896288/5712883.
                    self._workdLogQueue = Queue()
                    Thread(
                        target=_collectLogs,
                        args=(self._workd.stdout, self._workdLogQueue),
                        daemon=True,  # Shut down the thread if the parent process exits.
                    ).start()
                    try:
                        # Wait for workd to become connectable before opening client channels.
                        _waitFor(lambda: exists(self._workdSocket))
                        self._runtimeChannel = self._channel()
                        self._imageChannel = self._channel()
                        self.runtimeService = RuntimeServiceStub(self._runtimeChannel)
                        self.imageService = ImageServiceStub(self._imageChannel)
                    except:
                        self._workdLogQueue.shutdown()
                        raise
                except:
                    self._workd.terminate()
                    self._workd.wait(_timeout.seconds)
                    raise
            except:
                self._ociRuntime.stop(_timeout.seconds)
                raise
        except:
            self._imageRegistry.server_close()
            raise

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
                self._workd.wait(_timeout.seconds)
            finally:
                try:
                    self._imageRegistry.server_close()
                finally:
                    try:
                        self._ociRuntime.stop(_timeout.seconds)
                    finally:
                        self._workdLogQueue.shutdown()

    def pushImage(
        self, domain: str, service: str, version: str, module: str, metadata: str
    ):
        """Push a Vimana Wasm "container" image to the running container registry

        Args:
            domain:    e.g. `1234567890abcdef1234567890abcdef`
            service:   e.g. `some.package.FooService`
            version:   e.g. `1.0.0-release`
            module:    Path to compiled Wasm component byte code file.
            metadata:  Path to serialized gRPC service metadata file.
        """
        command = [
            _pushImagePath,
            f'http://localhost:{self._imageRegistryPort}',
            domain,
            service,
            version,
            module,
            metadata,
        ]
        status = Popen(command).wait(_timeout.seconds)
        if status != 0:
            raise RuntimeError(f'Failed to push image (status={status}).')

    def setupImage(
        self,
        service: str,
        version: str,
        module: str,
        metadata: str,
        domain: str = None,
    ) -> tuple[str, str, str, str, dict[str, str], ImageSpec]:
        """
        Boilerplate to create a component name,
        push the given module and metadata as an image to the registry,
        and pull that same image into the runtime.

        If the domain is not supplied, use a random domain.
        """
        domain = domain or hexUuid()
        componentName = f'{domain}:{service}@{version}'
        labels = {
            'vimana.host/domain': domain,
            'vimana.host/service': service,
            'vimana.host/version': version,
        }
        self.pushImage(domain, service, version, module, metadata)
        imageSpec = ImageSpec(
            image=self.imageId(domain, service, version),
            runtime_handler=RUNTIME_NAME,
        )
        self.imageService.PullImage(
            PullImageRequest(
                image=imageSpec,
                sandbox_config=PodSandboxConfig(labels=labels),
            ),
        )
        return (domain, service, version, componentName, labels, imageSpec)

    def imageId(self, domain: str, service: str, version: str) -> str:
        # TODO: Rewrite the image pusher in Python and use it to construct the URL for pulling.
        serviceHex = service.encode().hex()
        serviceHex = ''.join(l + u for l, u in zip(serviceHex[1::2], serviceHex[0::2]))
        return f'localhost:{self._imageRegistryPort}/{domain}/{serviceHex}:{version}'

    def verifyFsUsage(self, testCase: TestCase) -> (int, int):
        """
        Exercise `ImageService.ImageFsInfo`
        and compare the results to an independent calculation.
        Return (`used-bytes`, `inodes-used`).
        """
        response = self.imageService.ImageFsInfo(ImageFsInfoRequest())
        # Expect the Vimana usage information to be first in the results.
        reportedUsage = response.image_filesystems[0]

        testCase.assertEqual(reportedUsage.fs_id.mountpoint, self._imageStore.name)

        usedBytes = 0
        inodesUsed = 0
        for directory, _, filenames in walk(self._imageStore.name):
            # The runtime does not count the root directory when counting inodes.
            if directory != self._imageStore.name:
                inodesUsed += 1
            for filename in filenames:
                inodesUsed += 1
                usedBytes += stat(join(directory, filename)).st_size

        testCase.assertEqual(reportedUsage.used_bytes.value, usedBytes)
        testCase.assertEqual(reportedUsage.inodes_used.value, inodesUsed)

        return (usedBytes, inodesUsed)

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
                logs.append(self._workdLogQueue.get(block=False))
            except (Empty, ShutDown):
                return logs

    def printWorkdLogs(self, testCase: TestCase):
        """Print collected workd logs to standard error, if there are any."""
        logs = self.workdLogs()
        if len(logs) > 0:
            testName = testCase.id().split('.')[-1]
            header = f'\nWorkd logs for {testName}:\n'
            message = '> '.join(chain((header,), logs))
            print(message, file=stderr)


def startWorkd(
    ociRuntimeSocket: str,
    imageRegistryPort: int,
    imageStorePath: str,
    ipamPath: str,
) -> tuple[Popen, str]:
    """Start a background process running the work node daemon.

    Return the running process and the UNIX socket path where it's listening.
    """
    socket = _tmpName()
    insecureRegistry = f'localhost:{imageRegistryPort}'
    networkInterface = 'lo'  # Loopback device.
    podIps = _uniquePidBasedCidr()
    command = [
        _workdPath,
        f'--incoming={socket}',
        f'--downstream={ociRuntimeSocket}',
        f'--image-store={imageStorePath}',
        f'--insecure-registries={insecureRegistry}',
        f'--ipam-plugin={ipamPath}',
        f'--network-interface={networkInterface}',
        f'--pod-ips={podIps}',
    ]
    # Open a line-buffered text-mode pipe for stdout
    # and convert all CR/LF sequences to plain LF.
    process = Popen(command, stdout=PIPE, text=True, bufsize=1)
    return (process, socket)


def _uniquePidBasedCidr():
    """Return a unique IPv6 address range based on the current PID.

    Per-process unique ranges allow tests running in parallel to share a network device,
    in case the test has to run in a weaker sandbox
    (i.e. Bazel's `processwrapper-sandbox` instead of `linux-sandbox`,
    as occurs within a containerized CI workflow).
    """
    # Format the current PID as an 8-character hex string.
    # In case the PID is greater than 2^32, use only the least-significant digits.
    # That should never happen in practice;
    # the default maximum PID on 64-bit Linux is usually 2^22.
    pidHex = f'{getpid():08x}'[8::-1]
    # Use the PID as part of a unique 48-bit address prefix,
    # allowing space for 2^80 pods.
    return f'fc00:{pidHex[:4]}:{pidHex[4:]}::/48'


def startImageRegistry() -> tuple[HTTPServer, int]:
    """Start a mock container image registry on some available port.

    Return the running server and the port number where it's listening.
    """
    port = _findAvailablePort()
    server = MockImageRegistryServer(port)
    Thread(
        target=server.serve_forever,
        daemon=True,  # Shut down the thread if the parent process exits.
    ).start()
    return (server, port)


def startOciRuntime() -> tuple[grpc.Server, str]:
    """Start a background process running a "fake" OCI container runtime.

    Return the running server and the UNIX socket path where it's listening.
    """
    socket = _tmpName()
    server = grpc.server(ThreadPoolExecutor(max_workers=1))
    add_RuntimeServiceServicer_to_server(FakeRuntimeService(), server)
    add_ImageServiceServicer_to_server(FakeImageService(), server)
    server.add_insecure_port(f'unix://{socket}')
    server.start()
    return (server, socket)


def hexUuid() -> str:
    return uuid4().hex


def ipHostName(address: IPv4Address | IPv6Address) -> str:
    """Return an IP address in a string form that can be used as a hostname.

    IPv6 addresses must be wrapped in square brackets.
    """
    return f'[{address}]' if isinstance(address, IPv6Address) else str(address)


def _findAvailablePort(attempts: int = 5) -> int:
    """Find an available TCP port by random probing."""
    for i in range(attempts):
        # Pick a random port in the ephemeral range: [49152–65536).
        port = randrange(49152, 65536)
        if _isPortAvailable(port):
            return port
    raise RuntimeError(f'Could not find an open port after {attempts} attempts.')


def _collectLogs(stdout: TextIO, queue: Queue):
    """Read all lines from the `stdout` pipe, adding each line to the queue."""
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


def _waitFor(predicate: Callable[[], bool]):
    start = datetime.now()
    while not predicate():
        if datetime.now() - start > _timeout:
            raise RuntimeError('Timed out polling for condition')
        sleep(1 / 32)  # ~30ms


def _readFile(path: str) -> bytes:
    with open(path, 'rb') as f:
        return f.read()


def _tmpName() -> str:
    """Return a unique name for a hypothetical temporary file that does not exist."""
    f = NamedTemporaryFile()
    name = f.name
    f.close()  # Delete the file.
    return name


# Regular expressions used by the mock image registry.
# A real registry would support multiple digest algorithms,
# but the mock registry currently only supports SHA-256 for simplicity.
# Also leverage the knowledge that mock registry upload IDs are simply 36-character UUIDs.
_postBlobPath = compileRegex(r'^/v2/(.+)/blobs/uploads/$')
_putBlobPath = compileRegex(
    r'^/v2/(.+)/blobs/uploads/([-0-9a-f]{36})\?digest=sha256:([0-9a-f]{64})$'
)
_getBlobPath = compileRegex(r'^/v2/(.+)/blobs/sha256:([0-9a-f]{64})$')
_manifestPath = compileRegex(r'^/v2/(.+)/manifests/([^/]+)$')

# MIME types:
OCTET_STREAM_MIME_TYPE = 'application/octet-stream'
IMAGE_MANIFEST_MIME_TYPE = 'application/vnd.oci.image.manifest.v1+json'
IMAGE_CONFIG_MIME_TYPE = 'application/vnd.oci.image.config.v1+json'
WASM_MIME_TYPE = 'application/wasm'
PROTOBUF_MIME_TYPE = 'application/protobuf'


class MockImageRegistryServer(HTTPServer):
    def __init__(self, port):
        self.nameToUploadIds = defaultdict(set)
        self.nameToHashToBlob = defaultdict(dict)
        self.nameToReferenceToManifest = defaultdict(dict)
        super().__init__(('localhost', port), MockImageRegistryHandler)


class MockImageRegistryHandler(BaseHTTPRequestHandler):
    def do_POST(self):
        # Initiates an upload.
        if path := _postBlobPath.match(self.path):
            name = path.group(1)
            uploadId = str(uuid4())

            self.server.nameToUploadIds[name].add(uploadId)

            self.send_response(HTTPStatus.ACCEPTED.value)
            self.send_header('Location', f'/v2/{name}/blobs/uploads/{uploadId}')
            self.end_headers()

        else:
            self.send_error(HTTPStatus.BAD_REQUEST.value, message='invalid URL')

    def do_PUT(self):
        # Uploads actual data (either a blob or a manifest).
        if path := _putBlobPath.match(self.path):
            name = path.group(1)
            uploadId = path.group(2)
            blobSha256 = path.group(3)
            contentLength = int(self.headers['Content-Length'])
            blob = self.rfile.read(contentLength)

            if self.headers['Content-Type'] != OCTET_STREAM_MIME_TYPE:
                self.send_error(
                    HTTPStatus.BAD_REQUEST.value, message='bad content type'
                )
                return
            if _sha256(blob) != blobSha256:
                self.send_error(HTTPStatus.BAD_REQUEST.value, message='bad digest')
                return
            if uploadId not in self.server.nameToUploadIds[name]:
                self.send_error(HTTPStatus.NOT_FOUND.value)
                return

            self.server.nameToUploadIds[name].remove(uploadId)
            self.server.nameToHashToBlob[name][blobSha256] = blob

            self.send_response(HTTPStatus.CREATED.value)
            self.send_header('Location', f'/v2/{name}/blobs/sha256:{blobSha256}')
            self.end_headers()

        elif path := _manifestPath.match(self.path):
            name = path.group(1)
            reference = path.group(2)
            contentLength = int(self.headers['Content-Length'])
            manifestBytes = self.rfile.read(contentLength)

            if self.headers['Content-Type'] != IMAGE_MANIFEST_MIME_TYPE:
                self.send_error(
                    HTTPStatus.BAD_REQUEST.value, message='bad content type'
                )
                return
            manifest = parseJson(manifestBytes)
            manifestConditions = [
                manifest['schemaVersion'] == 2,
                self._validateDescriptor(
                    name, manifest['config'], IMAGE_CONFIG_MIME_TYPE
                ),
                len(manifest['layers']) == 2,
                self._validateDescriptor(name, manifest['layers'][0], WASM_MIME_TYPE),
                self._validateDescriptor(
                    name, manifest['layers'][1], PROTOBUF_MIME_TYPE
                ),
            ]
            if not all(manifestConditions):
                self.send_error(HTTPStatus.BAD_REQUEST.value, message='bad manifest')
                return

            self.server.nameToReferenceToManifest[name][reference] = manifestBytes

            self.send_response(HTTPStatus.CREATED.value)
            self.send_header(
                'Location', f'/v2/{name}/manifests/sha256:{_sha256(manifestBytes)}'
            )
            self.end_headers()

        else:
            self.send_error(HTTPStatus.BAD_REQUEST.value, message='invalid URL')

    def _validateDescriptor(
        self, name: str, descriptor: dict[str, Any], mediaType: str
    ) -> bool:
        """Validate a [descriptor][https://specs.opencontainers.org/image-spec/descriptor/].

        Check that the media types equals the expected value,
        and that the blob it refers to by digest exists under the given name,
        with the correct size.
        """
        # Remove the digest prefix to look it up in the map.
        blobSha256 = descriptor['digest'][len('sha256:') :]
        blob = self.server.nameToHashToBlob[name][blobSha256]
        return (
            isinstance(blob, bytes)
            and descriptor['mediaType'] == mediaType
            and descriptor['size'] == len(blob)
        )

    def do_GET(self):
        # Retrieve either a blob or a manifest.
        if path := _getBlobPath.match(self.path):
            self._getBoilerplate(path, self.server.nameToHashToBlob)
        elif path := _manifestPath.match(self.path):
            self._getBoilerplate(path, self.server.nameToReferenceToManifest)
        else:
            self.send_error(HTTPStatus.BAD_REQUEST.value, message='invalid URL')

    def _getBoilerplate(self, path: Match, table: dict[str, dict[str, bytes]]):
        """Common logic shared between blob-fetching and manifest-fetching."""
        name = path.group(1)
        digestOrReference = path.group(2)

        if digestOrReference not in table[name]:
            self.send_error(HTTPStatus.NOT_FOUND.value)
            return
        blobOrManifest = table[name][digestOrReference]

        self.send_response(HTTPStatus.OK.value)
        self.end_headers()
        self.wfile.write(blobOrManifest)

    def log_message(self, format, *args):
        pass  # Don't clutter up standard error.


def _sha256(data: bytes) -> str:
    hasher = sha256()
    hasher.update(data)
    return hasher.hexdigest()


class FakeRuntimeService(RuntimeServiceServicer):
    """Fake implementation of the CRI API's `RuntimeService` that does nothing."""

    def Version(self, request, context):
        return VersionResponse()

    def RunPodSandbox(self, request, context):
        return RunPodSandboxResponse()

    def StopPodSandbox(self, request, context):
        return StopPodSandboxResponse()

    def RemovePodSandbox(self, request, context):
        return RemovePodSandboxResponse()

    def PodSandboxStatus(self, request, context):
        return PodSandboxStatusResponse()

    def ListPodSandbox(self, request, context):
        return ListPodSandboxResponse()

    def CreateContainer(self, request, context):
        return CreateContainerResponse()

    def StartContainer(self, request, context):
        return StartContainerResponse()

    def StopContainer(self, request, context):
        return StopContainerResponse()

    def RemoveContainer(self, request, context):
        return RemoveContainerResponse()

    def ListContainers(self, request, context):
        return ListContainersResponse()

    def ContainerStatus(self, request, context):
        return ContainerStatusResponse()

    def UpdateContainerResources(self, request, context):
        return UpdateContainerResourcesResponse()

    def ReopenContainerLog(self, request, context):
        return ReopenContainerLogResponse()

    def ExecSync(self, request, context):
        return ExecSyncResponse()

    def Exec(self, request, context):
        return ExecResponse()

    def Attach(self, request, context):
        return AttachResponse()

    def PortForward(self, request, context):
        return PortForwardResponse()

    def ContainerStats(self, request, context):
        return ContainerStatsResponse()

    def ListContainerStats(self, request, context):
        return ListContainerStatsResponse()

    def PodSandboxStats(self, request, context):
        return PodSandboxStatsResponse()

    def ListPodSandboxStats(self, request, context):
        return ListPodSandboxStatsResponse()

    def UpdateRuntimeConfig(self, request, context):
        return UpdateRuntimeConfigResponse()

    def Status(self, request, context):
        return StatusResponse()

    def CheckpointContainer(self, request, context):
        return CheckpointContainerResponse()

    def GetContainerEvents(self, request, context):
        return ContainerEventResponse()

    def ListMetricDescriptors(self, request, context):
        return ListMetricDescriptorsResponse()

    def ListPodSandboxMetrics(self, request, context):
        return ListPodSandboxMetricsResponse()

    def RuntimeConfig(self, request, context):
        return RuntimeConfigResponse()


class FakeImageService(ImageServiceServicer):
    """Fake implementation of the CRI API's `ImageService` that does nothing."""

    def ListImages(self, request, context):
        return ListImagesResponse()

    def ImageStatus(self, request, context):
        return ImageStatusResponse()

    def PullImage(self, request, context):
        return PullImageResponse()

    def RemoveImage(self, request, context):
        return RemoveImageResponse()

    def ImageFsInfo(self, request, context):
        return ImageFsInfoResponse()
