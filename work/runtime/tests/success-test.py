from unittest import TestCase, main

import grpc

from work.runtime.tests.util import WorkdTester, findAvailablePort, hexUuid
from work.runtime.tests.api_pb2 import (
    VersionRequest,
    RunPodSandboxRequest,
    PodSandboxConfig,
    PodSandboxMetadata,
    PortMapping,
    Protocol,
    CreateContainerRequest,
    ContainerConfig,
    ContainerMetadata,
    ImageSpec,
    KeyValue,
    StartContainerRequest,
)
from work.runtime.tests.components.adder_pb2 import AddFloatsRequest, AddFloatsResponse
from work.runtime.tests.components.adder_pb2_grpc import AdderServiceStub

class SuccessTest(TestCase):
    @classmethod
    def setUpClass(cls):
        # Use a single, long-running runtime instance for all tests.
        # Isolation should be enforced by using unique domains in each test.
        cls.tester = WorkdTester().__enter__()

    @classmethod
    def tearDownClass(cls):
        # Shut down the various servers and subprocesses.
        cls.tester.__exit__()

    def test_Version(self):
        request = VersionRequest()

        response = self.tester.runtimeService.Version(request)

        self.assertEqual(response.runtime_name, 'workd')
        self.assertEqual(response.runtime_api_version, 'v1')
        self.assertEqual(response.version, '0.1.0')

    def test_RunPodSandbox_NoHandlerToOci(self):
        request = RunPodSandboxRequest()

        response = self.tester.runtimeService.RunPodSandbox(request)

        self.assertTrue(response.pod_sandbox_id.startswith('O:'))

    def test_RunPodSandbox_DefaultHandlerToOci(self):
        request = RunPodSandboxRequest(runtime_handler='something')

        response = self.tester.runtimeService.RunPodSandbox(request)

        self.assertTrue(response.pod_sandbox_id.startswith('O:'))

    def test_SimpleContainer(self):
        domain = hexUuid()
        service = 'package.Serviss'
        version = '1.2.3-fureal'
        component = f'{domain}:{service}@{version}'
        labels = {
            'vimana.host/domain': domain,
            'vimana.host/service': service,
            'vimana.host/version': version,
        }
        self.tester.pushImageFromFiles(
            domain, service, version,
            'work/runtime/tests/components/adder-c.component.wasm',
            'work/runtime/tests/components/adder.binpb',
        )
        port = findAvailablePort()

        response = self.tester.runtimeService.RunPodSandbox(
            RunPodSandboxRequest(
                runtime_handler='workd',
                config=PodSandboxConfig(
                    metadata=PodSandboxMetadata(
                        name=f'{domain}-name',
                        uid=f'{domain}-uid',
                        namespace=f'{domain}-namespace',
                        attempt=6,
                    ),
                    hostname='simple-pod-hostname',
                    port_mappings=[PortMapping(
                        protocol=Protocol.TCP,
                        container_port=443,
                        host_port=port,
                    )],
                    labels=labels,
                ),
            ),
        )

        pod_sandbox_id = response.pod_sandbox_id
        self.assertTrue(pod_sandbox_id.startswith('W:'))

        response = self.tester.runtimeService.CreateContainer(
            CreateContainerRequest(
                pod_sandbox_id=response.pod_sandbox_id,
                config=ContainerConfig(
                    metadata=ContainerMetadata(
                        name=f'{domain}-container-name',
                        attempt=3,
                    ),
                    image=ImageSpec(
                        image=component,
                        runtime_handler='TODO-this-should-be-checked',
                    ),
                    envs=[KeyValue(key='some-key', value='some-value')],
                    labels=labels,
                ),
            ),
        )

        # The pod should always have the same ID as its one container.
        self.assertEqual(response.container_id, pod_sandbox_id)

        response = self.tester.runtimeService.StartContainer(
            StartContainerRequest(container_id=response.container_id),
        )

        client = AdderServiceStub(grpc.insecure_channel(f'localhost:{port}'))
        response = client.AddFloats(AddFloatsRequest(x=3.5, y=-1.2))

        self.assertEqual(response, AddFloatsResponse(result=2.3))

if __name__ == '__main__':
    main()