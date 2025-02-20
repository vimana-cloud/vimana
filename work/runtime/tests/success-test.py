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
    ListPodSandboxRequest,
    PodSandboxFilter,
    ContainerStatusRequest,
    ContainerState,
    ContainerResources,
    ContainerUser,
)
from work.runtime.tests.components.adder_pb2 import AddFloatsRequest, AddFloatsResponse
from work.runtime.tests.components.adder_pb2_grpc import AdderServiceStub

class SuccessTest(TestCase):
    @classmethod
    def setUpClass(cls):
        # A single, long-running runtime instance is available to all tests.
        # Any test that requires isolation can simply spin up it's own `WorkdTester`
        # (see `test_ListPodSandbox` for example).
        cls.tester = WorkdTester().__enter__()

    @classmethod
    def tearDownClass(cls):
        # Shut down the various servers and subprocesses.
        cls.tester.__exit__(None, None, None)

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

    def test_SimpleContainerLifecycle(self):
        domain, service, version, componentName, labels = self.setupImage(
            service = 'package.Serviss',
            version = '1.2.3-fureal',
            module = 'work/runtime/tests/components/adder-c.component.wasm',
            metadata = 'work/runtime/tests/components/adder.binpb',
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

        podSandboxId = response.pod_sandbox_id
        self.assertTrue(podSandboxId.startswith('W:'))

        response = self.tester.runtimeService.CreateContainer(
            CreateContainerRequest(
                pod_sandbox_id=podSandboxId,
                config=ContainerConfig(
                    metadata=ContainerMetadata(
                        name=f'{domain}-container-name',
                        attempt=3,
                    ),
                    image=ImageSpec(
                        image=componentName,
                        runtime_handler='workd',
                    ),
                    envs=[KeyValue(key='some-key', value='some-value')],
                    labels=labels,
                ),
            ),
        )

        # The pod should always have the same ID as its one container.
        self.assertEqual(response.container_id, podSandboxId)

        self.tester.runtimeService.StartContainer(
            StartContainerRequest(container_id=response.container_id),
        )

        # Finally, try exercising the data plane.
        client = AdderServiceStub(grpc.insecure_channel(f'localhost:{port}'))
        response = client.AddFloats(AddFloatsRequest(x=3.5, y=-1.2))

        self.assertEqual(response, AddFloatsResponse(result=2.3))

    def test_ListPodSandbox(self):
        # Use an isolated runtime instance so we don't get random shit in our list results.
        with WorkdTester() as tester:

            # Set up 2 images ("foo" and "bar").
            # They share the same Wasm module, but have different pod sandbox / container metadata.
            fooDomain, fooService, fooVersion, fooComponent, fooLabels = self.setupImage(
                service = 'foo.HelloWorld',
                version = '0.0.0',
                module = 'work/runtime/tests/components/adder-c.component.wasm',
                metadata = 'work/runtime/tests/components/adder.binpb',
            )
            fooPort = findAvailablePort()
            fooMetadata = PodSandboxMetadata(
                name=f'{fooDomain}-name',
                uid=f'{fooDomain}-uid',
                namespace=f'{fooDomain}-namespace',
                attempt=1,
            )
            fooSandboxId = tester.runtimeService.RunPodSandbox(
                RunPodSandboxRequest(
                    runtime_handler='workd',
                    config=PodSandboxConfig(
                        metadata=fooMetadata,
                        hostname='foobar',
                        port_mappings=[PortMapping(
                            protocol=Protocol.TCP,
                            container_port=443,
                            host_port=fooPort,
                        )],
                        labels=fooLabels,
                    ),
                ),
            ).pod_sandbox_id

            barDomain, barService, barVersion, barComponent, barLabels = self.setupImage(
                service = 'bar.GoodbyeWorld',
                version = '6.6.6',
                module = 'work/runtime/tests/components/adder-c.component.wasm',
                metadata = 'work/runtime/tests/components/adder.binpb',
            )
            barPort = findAvailablePort()
            barMetadata = PodSandboxMetadata(
                name=f'{barDomain}-name',
                uid=f'{barDomain}-uid',
                namespace=f'{barDomain}-namespace',
                attempt=0,
            )
            barSandboxId = tester.runtimeService.RunPodSandbox(
                RunPodSandboxRequest(
                    runtime_handler='workd',
                    config=PodSandboxConfig(
                        metadata=barMetadata,
                        hostname='barbar',
                        port_mappings=[PortMapping(
                            protocol=Protocol.TCP,
                            container_port=443,
                            host_port=barPort,
                        )],
                        labels=barLabels,
                    ),
                ),
            ).pod_sandbox_id

            # Listing with no filter means we want all the pod sandboxes.
            response = tester.runtimeService.ListPodSandbox(ListPodSandboxRequest())

            self.assertEqual(len(response.items), 2)
            # Results could be returned in any order.
            # Since a collection of 2 items only has 2 possible orderings,
            # just check the first item.
            fooIndex = 0 if response.items[0].id == fooSandboxId else 1
            barIndex = 1 - fooIndex
            self.assertEqual(response.items[fooIndex].id, fooSandboxId)
            self.assertEqual(response.items[fooIndex].metadata, fooMetadata)
            self.assertEqual(response.items[fooIndex].labels['vimana.host/domain'], fooDomain)
            self.assertEqual(response.items[fooIndex].labels['vimana.host/service'], fooService)
            self.assertEqual(response.items[fooIndex].labels['vimana.host/version'], fooVersion)
            self.assertEqual(response.items[fooIndex].runtime_handler, 'workd')
            self.assertEqual(response.items[barIndex].id, barSandboxId)
            self.assertEqual(response.items[barIndex].metadata, barMetadata)
            self.assertEqual(response.items[barIndex].labels['vimana.host/domain'], barDomain)
            self.assertEqual(response.items[barIndex].labels['vimana.host/service'], barService)
            self.assertEqual(response.items[barIndex].labels['vimana.host/version'], barVersion)
            self.assertEqual(response.items[barIndex].runtime_handler, 'workd')

            # Look for a single pod by ID.
            response = tester.runtimeService.ListPodSandbox(
                ListPodSandboxRequest(
                    filter = PodSandboxFilter(
                        id = fooSandboxId,
                    ),
                ),
            )

            self.assertEqual(len(response.items), 1)
            self.assertEqual(response.items[0].id, fooSandboxId)
            self.assertEqual(response.items[0].metadata, fooMetadata)
            self.assertEqual(response.items[0].labels['vimana.host/domain'], fooDomain)
            self.assertEqual(response.items[0].labels['vimana.host/service'], fooService)
            self.assertEqual(response.items[0].labels['vimana.host/version'], fooVersion)
            self.assertEqual(response.items[0].runtime_handler, 'workd')

            # Look for a single pod by labels.
            response = tester.runtimeService.ListPodSandbox(
                ListPodSandboxRequest(
                    filter = PodSandboxFilter(
                        label_selector = {
                            'vimana.host/domain': barDomain,
                            'vimana.host/service': barService,
                            'vimana.host/version': barVersion,
                        },
                    ),
                ),
            )

            self.assertEqual(len(response.items), 1)
            self.assertEqual(response.items[0].id, barSandboxId)
            self.assertEqual(response.items[0].metadata, barMetadata)
            self.assertEqual(response.items[0].labels['vimana.host/domain'], barDomain)
            self.assertEqual(response.items[0].labels['vimana.host/service'], barService)
            self.assertEqual(response.items[0].labels['vimana.host/version'], barVersion)
            self.assertEqual(response.items[0].runtime_handler, 'workd')

            # Look by labels with no results (because it mixes "foo" labels with "bar" labels).
            response = tester.runtimeService.ListPodSandbox(
                ListPodSandboxRequest(
                    filter = PodSandboxFilter(
                        label_selector = {
                            'vimana.host/domain': barDomain,
                            'vimana.host/service': barService,
                            'vimana.host/version': fooVersion,
                        },
                    ),
                ),
            )

            self.assertEqual(len(response.items), 0)

    def test_ContainerStatus(self):
        domain, service, version, componentName, labels = self.setupImage(
            service = 'some.Service',
            version = '1.2.3',
            module = 'work/runtime/tests/components/adder-c.component.wasm',
            metadata = 'work/runtime/tests/components/adder.binpb',
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

        podSandboxId = response.pod_sandbox_id
        containerMetadata = ContainerMetadata(
            name=f'{domain}-container-name',
            attempt=1,
        )
        imageSpec = ImageSpec(
            image=componentName,
            runtime_handler='workd',
        )

        response = self.tester.runtimeService.CreateContainer(
            CreateContainerRequest(
                pod_sandbox_id=podSandboxId,
                config=ContainerConfig(
                    metadata=containerMetadata,
                    image=imageSpec,
                    labels=labels,
                ),
            ),
        )

        containerId = response.container_id

        response = self.tester.runtimeService.ContainerStatus(
            ContainerStatusRequest(container_id=containerId),
        )

        # Extra info should be empty because `ContainerStatusRequest.verbose` was false.
        self.assertEqual(len(response.info), 0)
        self.assertEqual(response.status.id, containerId)
        self.assertEqual(response.status.metadata, containerMetadata)
        self.assertEqual(response.status.state, ContainerState.CONTAINER_CREATED)
        # Of the three timestamps, only the *created* one should be set.
        self.assertGreater(response.status.created_at, 0)
        self.assertEqual(response.status.started_at, 0)
        self.assertEqual(response.status.finished_at, 0)

        self.assertEqual(response.status.exit_code, 0)
        self.assertEqual(response.status.image, imageSpec)
        self.assertEqual(response.status.image_ref, 'TODO')
        self.assertEqual(response.status.reason, 'TODO')
        self.assertEqual(response.status.message, 'TODO')
        self.assertEqual(response.status.labels, labels)
        self.assertEqual(len(response.status.annotations), 0)
        self.assertEqual(len(response.status.mounts), 0)
        self.assertEqual(response.status.log_path, '')
        self.assertEqual(response.status.resources, ContainerResources())
        self.assertEqual(response.status.image_id, 'TODO')
        self.assertEqual(response.status.user, ContainerUser())

    def setupImage(self, service, version, module, metadata):
        """ Boilerplate to create consistent metadata with a random domain. """
        domain = hexUuid()
        componentName = f'{domain}:{service}@{version}'
        labels = {
            'vimana.host/domain': domain,
            'vimana.host/service': service,
            'vimana.host/version': version,
        }
        self.tester.pushImage(domain, service, version, module, metadata)
        return (domain, service, version, componentName, labels)

if __name__ == '__main__':
    main()