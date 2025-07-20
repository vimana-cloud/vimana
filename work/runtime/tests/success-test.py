"""'Happy path' unit tests."""

from ipaddress import ip_address
from unittest import main

from grpc import RpcError, StatusCode, insecure_channel

from work.runtime.tests.api_pb2 import (
    ContainerConfig,
    ContainerMetadata,
    ImageFsInfoResponse,
    ContainerResources,
    ContainerState,
    ContainerStatusRequest,
    ContainerUser,
    CreateContainerRequest,
    ImageSpec,
    ImageStatusRequest,
    KeyValue,
    PodSandboxConfig,
    PodSandboxMetadata,
    PodSandboxStatusRequest,
    RemoveContainerRequest,
    RemoveImageRequest,
    RemovePodSandboxRequest,
    RunPodSandboxRequest,
    RunPodSandboxResponse,
    StartContainerRequest,
    StopContainerRequest,
    StopPodSandboxRequest,
    VersionRequest,
)
from work.runtime.tests.components.adder_pb2 import AddFloatsRequest, AddFloatsResponse
from work.runtime.tests.components.adder_pb2_grpc import AdderServiceStub
from work.runtime.tests.util import RUNTIME_NAME, WorkdTestCase, ipHostName


class SuccessTest(WorkdTestCase):
    def test_Version(self):
        request = VersionRequest()

        response = self.runtimeService.Version(request)

        self.assertEqual(response.runtime_name, RUNTIME_NAME)
        self.assertEqual(response.runtime_api_version, 'v1')
        self.assertEqual(response.version, '0.1.0')

    def test_RunPodSandbox_NoHandlerToOci(self):
        request = RunPodSandboxRequest()
        downstreamResponse = RunPodSandboxResponse(pod_sandbox_id='from downstream!')
        self.downstreamRuntimeService.returnNext('RunPodSandbox', downstreamResponse)

        response = self.runtimeService.RunPodSandbox(request)

        self.assertEqual(response, downstreamResponse)

    def test_RunPodSandbox_DefaultHandlerToOci(self):
        request = RunPodSandboxRequest(runtime_handler='something')
        downstreamResponse = RunPodSandboxResponse(pod_sandbox_id='🥲')
        self.downstreamRuntimeService.returnNext('RunPodSandbox', downstreamResponse)

        response = self.runtimeService.RunPodSandbox(request)

        self.assertEqual(response, downstreamResponse)

    def test_ImageStatus_NotFound(self):
        response = self.imageService.ImageStatus(
            ImageStatusRequest(
                image=ImageSpec(
                    image=self.imageId(
                        'a4f7b91e3c0d8e5a2f9c6d4b7e1a3c8f',
                        'this.should.never.be.Found',
                        '10.10.10',
                    ),
                    runtime_handler=RUNTIME_NAME,
                ),
            ),
        )

        # An absent image indicates to Kubelet that it must be pulled.
        self.assertFalse(response.HasField('image'))

    def test_ImageFsUsage(self):
        self.downstreamImageService.returnNext(
            'ImageFsInfo', ImageFsInfoResponse(), count=5
        )
        noneUsedBytes, noneInodesUsed = self.verifyFsUsage()

        domain, _, _, _, _, firstImageSpec = self.setupImage(
            service='just.some.Image',
            version='1.2.3',
            module='work/runtime/tests/components/adder-c.component.wasm',
            metadata='work/runtime/tests/components/adder.binpb',
        )

        singleUsedBytes, singleInodesUsed = self.verifyFsUsage()
        self.assertGreater(singleUsedBytes, noneUsedBytes)
        self.assertEqual(singleInodesUsed, noneInodesUsed + 5)

        # Push a new version. This should share a service directory.
        _, _, _, _, _, secondImageSpec = self.setupImage(
            service='just.some.Image',
            version='4.5.6',
            module='work/runtime/tests/components/adder-c.component.wasm',
            metadata='work/runtime/tests/components/adder.binpb',
            domain=domain,
        )

        doubleUsedBytes, doubleInodesUsed = self.verifyFsUsage()
        self.assertEqual(
            doubleUsedBytes - singleUsedBytes,
            singleUsedBytes - noneUsedBytes,
        )
        self.assertEqual(doubleInodesUsed, singleInodesUsed + 3)

        # Remove the first image, leaving only the second image.
        self.imageService.RemoveImage(RemoveImageRequest(image=firstImageSpec))

        secondUsedBytes, secondInodesUsed = self.verifyFsUsage()
        self.assertEqual(secondUsedBytes, singleUsedBytes)
        self.assertEqual(secondInodesUsed, singleInodesUsed)

        # Remove the second image as well.
        self.imageService.RemoveImage(RemoveImageRequest(image=secondImageSpec))

        removedUsedBytes, removedInodesUsed = self.verifyFsUsage()
        self.assertEqual(removedUsedBytes, noneUsedBytes)
        self.assertEqual(removedInodesUsed, noneInodesUsed)

    def test_SimpleContainerLifecycle(self):
        domain, service, version, componentName, labels, imageSpec = self.setupImage(
            service='package.Serviss',
            version='1.2.3-fureal',
            module='work/runtime/tests/components/adder-c.component.wasm',
            metadata='work/runtime/tests/components/adder.binpb',
        )

        response = self.runtimeService.RunPodSandbox(
            RunPodSandboxRequest(
                runtime_handler=RUNTIME_NAME,
                config=PodSandboxConfig(
                    metadata=PodSandboxMetadata(
                        name=f'{domain}-name',
                        uid=f'{domain}-uid',
                        namespace=f'{domain}-namespace',
                    ),
                    hostname='TODO',
                    labels=labels,
                ),
            ),
        )

        podSandboxId = response.pod_sandbox_id
        expectedPodPrefix = f'p-{domain}:{service}@{version}#'
        self.assertTrue(podSandboxId.startswith(expectedPodPrefix))
        try:
            int(podSandboxId[len(expectedPodPrefix) :])
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
                    image=imageSpec,
                    envs=[KeyValue(key='some-key', value='some-value')],
                    labels=labels,
                ),
            ),
        )

        # The pod should always have the same ID as its one container (modulo the prefix).
        containerId = response.container_id
        self.assertTrue(containerId.startswith('c-'))
        self.assertEqual(containerId[len('c-') :], podSandboxId[len('p-') :])

        self.runtimeService.StartContainer(
            StartContainerRequest(container_id=containerId),
        )

        # Finally, try exercising the data plane.
        client = AdderServiceStub(insecure_channel(f'{ipHostName(ipAddress)}:80'))
        response = client.AddFloats(AddFloatsRequest(x=3.5, y=-1.2))

        self.assertEqual(response, AddFloatsResponse(result=2.3))

        self.runtimeService.StopContainer(
            StopContainerRequest(container_id=containerId, timeout=1),
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
            RemoveContainerRequest(container_id=containerId),
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
        domain, service, version, componentName, labels, imageSpec = self.setupImage(
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
                runtime_handler=RUNTIME_NAME,
                config=PodSandboxConfig(
                    metadata=PodSandboxMetadata(
                        name=f'{domain}-name',
                        uid=f'{domain}-uid',
                        namespace=f'{domain}-namespace',
                        attempt=666,
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
        self.assertEqual(response.status.log_path, '/dev/null')
        self.assertEqual(response.status.resources, ContainerResources())
        self.assertEqual(response.status.image_id, 'TODO')
        self.assertEqual(response.status.user, ContainerUser())

    # TODO: Test a container that's stopped then re-started without stopping the pod.


if __name__ == '__main__':
    main()
