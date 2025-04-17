"""'Happy path' unit tests."""

from ipaddress import ip_address
from unittest import main

from grpc import RpcError, StatusCode, insecure_channel

from work.runtime.tests.api_pb2 import (
    ContainerConfig,
    ContainerMetadata,
    ContainerResources,
    ContainerState,
    ContainerStatusRequest,
    ContainerUser,
    CreateContainerRequest,
    ImageSpec,
    KeyValue,
    PodSandboxConfig,
    PodSandboxMetadata,
    PodSandboxStatusRequest,
    RemoveContainerRequest,
    RemovePodSandboxRequest,
    RunPodSandboxRequest,
    StartContainerRequest,
    StopContainerRequest,
    StopPodSandboxRequest,
    VersionRequest,
)
from work.runtime.tests.components.adder_pb2 import AddFloatsRequest, AddFloatsResponse
from work.runtime.tests.components.adder_pb2_grpc import AdderServiceStub
from work.runtime.tests.util import WorkdTestCase, WorkdTester, ipHostName


class SuccessTest(WorkdTestCase):
    def test_Version(self):
        request = VersionRequest()

        response = self.runtimeService.Version(request)

        self.assertEqual(response.runtime_name, 'workd')
        self.assertEqual(response.runtime_api_version, 'v1')
        self.assertEqual(response.version, '0.1.0')

    def test_RunPodSandbox_NoHandlerToOci(self):
        request = RunPodSandboxRequest()

        response = self.runtimeService.RunPodSandbox(request)

        self.assertTrue(response.pod_sandbox_id.startswith('O:'))

    def test_RunPodSandbox_DefaultHandlerToOci(self):
        request = RunPodSandboxRequest(runtime_handler='something')

        response = self.runtimeService.RunPodSandbox(request)

        self.assertTrue(response.pod_sandbox_id.startswith('O:'))

    def test_SimpleContainerLifecycle(self):
        domain, service, version, componentName, labels = self.setupImage(
            service='package.Serviss',
            version='1.2.3-fureal',
            module='work/runtime/tests/components/adder-c.component.wasm',
            metadata='work/runtime/tests/components/adder.binpb',
        )

        response = self.runtimeService.RunPodSandbox(
            RunPodSandboxRequest(
                runtime_handler='workd',
                config=PodSandboxConfig(
                    metadata=PodSandboxMetadata(
                        name=f'{domain}-name',
                        uid=f'{domain}-uid',
                        namespace=f'{domain}-namespace',
                        attempt=6,
                    ),
                    hostname='TODO',
                    labels=labels,
                ),
            ),
        )

        podSandboxId = response.pod_sandbox_id
        expectedIdPrefix = f'W:{domain}:{service}@{version}#'
        self.assertTrue(podSandboxId.startswith(expectedIdPrefix))
        try:
            int(podSandboxId[len(expectedIdPrefix) :])
        except ValueError:
            self.fail('Pod sandbox ID must end with a pod ID (integer)')

        response = self.runtimeService.PodSandboxStatus(
            PodSandboxStatusRequest(pod_sandbox_id=podSandboxId),
        )

        ipAddress = ip_address(response.status.network.ip)

        response = self.runtimeService.CreateContainer(
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

        self.runtimeService.StartContainer(
            StartContainerRequest(container_id=podSandboxId),
        )

        # Finally, try exercising the data plane.
        client = AdderServiceStub(insecure_channel(f'{ipHostName(ipAddress)}:80'))
        response = client.AddFloats(AddFloatsRequest(x=3.5, y=-1.2))

        self.assertEqual(response, AddFloatsResponse(result=2.3))

        self.runtimeService.StopContainer(
            StopContainerRequest(container_id=podSandboxId, timeout=1),
        )

        try:
            client.AddFloats(AddFloatsRequest(x=3.5, y=-1.2))
        except RpcError as error:
            self.assertEqual(error.code(), StatusCode.UNAVAILABLE)
        else:
            self.fail(
                'Expected the server to be unavailable after stopping the container'
            )

        # This is a no-op but we'll exercise it anyway.
        self.runtimeService.RemoveContainer(
            RemoveContainerRequest(container_id=podSandboxId),
        )

        # TODO: Verify that the IP address is not yet freed.

        self.runtimeService.StopPodSandbox(
            StopPodSandboxRequest(pod_sandbox_id=podSandboxId),
        )

        # TODO: Verify that the IP address is freed.

        self.runtimeService.RemovePodSandbox(
            RemovePodSandboxRequest(pod_sandbox_id=podSandboxId),
        )

    def test_ContainerStatus(self):
        domain, service, version, componentName, labels = self.setupImage(
            service='some.Service',
            version='1.2.3',
            module='work/runtime/tests/components/adder-c.component.wasm',
            metadata='work/runtime/tests/components/adder.binpb',
        )
        # Set different labels on the pod vs. the container
        # so we can verify that the correct set is returned.
        podLabels = labels | {'only-for-pod': 'uh huh'}
        containerLabels = labels | {'only-for-container': 'fersher'}

        response = self.runtimeService.RunPodSandbox(
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

        response = self.runtimeService.CreateContainer(
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

        response = self.runtimeService.ContainerStatus(
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
