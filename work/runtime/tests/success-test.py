""" 'Happy path' unit tests. """

from unittest import TestCase, main
from ipaddress import IPv6Address, ip_address

from grpc import insecure_channel, RpcError, StatusCode

from work.runtime.tests.util import WorkdTester, ipHostName
from work.runtime.tests.api_pb2 import (
    VersionRequest,
    RunPodSandboxRequest,
    PodSandboxConfig,
    PodSandboxMetadata,
    PodSandboxStatusRequest,
    PortMapping,
    Protocol,
    CreateContainerRequest,
    ContainerConfig,
    ContainerMetadata,
    ImageSpec,
    KeyValue,
    StartContainerRequest,
    StopContainerRequest,
    RemoveContainerRequest,
    StopPodSandboxRequest,
    RemovePodSandboxRequest,
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

    def tearDown(self):
        self.tester.printWorkdLogs(self)

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
        domain, service, version, componentName, labels = self.tester.setupImage(
            service = 'package.Serviss',
            version = '1.2.3-fureal',
            module = 'work/runtime/tests/components/adder-c.component.wasm',
            metadata = 'work/runtime/tests/components/adder.binpb',
        )

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
                    labels=labels,
                ),
            ),
        )

        podSandboxId = response.pod_sandbox_id
        expectedIdPrefix = f'W:{domain}:{service}@{version}#'
        self.assertTrue(podSandboxId.startswith(expectedIdPrefix))
        try:
            int(podSandboxId[len(expectedIdPrefix):])
        except ValueError:
            self.fail('Pod sandbox ID must end with a pod ID (integer)')

        response = self.tester.runtimeService.PodSandboxStatus(
            PodSandboxStatusRequest(pod_sandbox_id=podSandboxId),
        )

        ipAddress = ip_address(response.status.network.ip)

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
            StartContainerRequest(container_id=podSandboxId),
        )

        # Finally, try exercising the data plane.
        client = AdderServiceStub(insecure_channel(f'{ipHostName(ipAddress)}:80'))
        response = client.AddFloats(AddFloatsRequest(x=3.5, y=-1.2))

        self.assertEqual(response, AddFloatsResponse(result=2.3))

        self.tester.runtimeService.StopContainer(
            StopContainerRequest(container_id=podSandboxId, timeout=1),
        )

        try:
            client.AddFloats(AddFloatsRequest(x=3.5, y=-1.2))
        except RpcError as error:
            self.assertEqual(error.code(), StatusCode.UNAVAILABLE)
        else:
            self.fail('Expected the server to be unavailable after stopping the container')

        # This is a no-op but we'll exercise it anyway.
        self.tester.runtimeService.RemoveContainer(
            RemoveContainerRequest(container_id=podSandboxId),
        )

        # TODO: Verify that the IP address is not yet freed.

        self.tester.runtimeService.StopPodSandbox(
            StopPodSandboxRequest(pod_sandbox_id=podSandboxId),
        )

        # TODO: Verify that the IP address is freed.

        self.tester.runtimeService.RemovePodSandbox(
            RemovePodSandboxRequest(pod_sandbox_id=podSandboxId),
        )

    def test_ListPodSandbox(self):
        # Use an isolated runtime instance so we don't get random shit in our list results.
        with WorkdTester() as tester:

            # Set up 2 images ("foo" and "bar").
            # They share the same Wasm module, but have different pod sandbox / container metadata.
            fooDomain, fooService, fooVersion, fooComponent, fooLabels = tester.setupImage(
                service = 'foo.HelloWorld',
                version = '0.0.0',
                module = 'work/runtime/tests/components/adder-c.component.wasm',
                metadata = 'work/runtime/tests/components/adder.binpb',
            )
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
                        labels=fooLabels,
                    ),
                ),
            ).pod_sandbox_id

            barDomain, barService, barVersion, barComponent, barLabels = tester.setupImage(
                service = 'bar.GoodbyeWorld',
                version = '6.6.6',
                module = 'work/runtime/tests/components/adder-c.component.wasm',
                metadata = 'work/runtime/tests/components/adder.binpb',
            )
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
                        labels=barLabels,
                    ),
                ),
            ).pod_sandbox_id

            # Listing with no filter means we want all the pod sandboxes.
            response = tester.runtimeService.ListPodSandbox(ListPodSandboxRequest())

            self.assertEqual(len(response.items), 2)
            # Results could be returned in any order,
            # but a collection of 2 items only has 2 possible orders.
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
        domain, service, version, componentName, labels = self.tester.setupImage(
            service = 'some.Service',
            version = '1.2.3',
            module = 'work/runtime/tests/components/adder-c.component.wasm',
            metadata = 'work/runtime/tests/components/adder.binpb',
        )
        # Set different labels on the pod vs. the container
        # so we can verify that the correct set is returned.
        podLabels = labels | {'only-for-pod': 'uh huh'}
        containerLabels = labels | {'only-for-container': 'fersher'}

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
                    labels=podLabels,
                ),
            ),
        )

        podSandboxId = response.pod_sandbox_id
        containerName = f'{domain}-container-name'
        containerMetadata = ContainerMetadata(
            name=containerName,
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
                    labels=containerLabels,
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
        self.assertEqual(response.status.labels, containerLabels)
        self.assertEqual(len(response.status.annotations), 0)
        self.assertEqual(len(response.status.mounts), 0)
        self.assertEqual(response.status.log_path, '')
        self.assertEqual(response.status.resources, ContainerResources())
        self.assertEqual(response.status.image_id, 'TODO')
        self.assertEqual(response.status.user, ContainerUser())

    # TODO: Test a container that's stopped then re-started without stopping the pod.

if __name__ == '__main__':
    main()
