"""Tests for `ListPodSandbox` and `ListContainers`."""

from enum import Enum, auto
from functools import partial
from os import getenv
from time import time_ns
from unittest import main

from work.runtime.tests.api_pb2 import (
    Container,
    ContainerConfig,
    ContainerFilter,
    ContainerMetadata,
    ContainerState,
    ContainerStateValue,
    CreateContainerRequest,
    ImageSpec,
    ListContainersRequest,
    ListPodSandboxRequest,
    PodSandbox,
    PodSandboxConfig,
    PodSandboxFilter,
    PodSandboxMetadata,
    PodSandboxState,
    PodSandboxStateValue,
    RemoveContainerRequest,
    RunPodSandboxRequest,
    StartContainerRequest,
    StopContainerRequest,
    StopPodSandboxRequest,
)
from work.runtime.tests.util import WorkdTestCase, hexUuid

# The number of nanoseconds it takes for this test to time out.
# Used for very rough upper / lower bounds when checking reasonableness of recent timestamps.
# https://bazel.build/reference/test-encyclopedia#initial-conditions
TEST_TIMEOUT_NANOSECONDS = int(getenv('TEST_TIMEOUT')) * 1000 * 1000 * 1000


class Phase(Enum):
    """Each possible phase of the lifecycle of a pod / container pair,
    denoted by the most recently called CRI API method.
    """

    RunPodSandbox = auto()
    CreateContainer = auto()
    StartContainer = auto()
    StopContainer = auto()
    RemoveContainer = auto()
    StopPodSandbox = auto()


class ListTest(WorkdTestCase):
    @classmethod
    def setUpClass(cls):
        """Create a bunch of pods in various phases of the lifecycle and with various labels.

        6 pods are created with the same domain, service, and version (1 in each lifecycle phase).
        1 pod is created with a different domain, service and version (in the `Created` phase).
        """
        super().setUpClass()

        (
            cls.fooDomain,
            cls.fooService,
            cls.fooVersion,
            cls.fooComponentName,
            cls.fooLabels,
        ) = cls.setupImage(
            service='foo.AdderService',
            version='1.2.3',
            module='work/runtime/tests/components/adder-c.component.wasm',
            metadata='work/runtime/tests/components/adder.binpb',
        )
        (
            cls.barDomain,
            cls.barService,
            cls.barVersion,
            cls.barComponentName,
            cls.barLabels,
        ) = cls.setupImage(
            service='bar.AdderService',
            version='0.0.0',
            module='work/runtime/tests/components/adder-c.component.wasm',
            metadata='work/runtime/tests/components/adder.binpb',
        )

        # Set up a pod / container in every possible state for the 'foo' labels.
        cls.fooPodMetadata = randomPodMetadata()
        cls.fooContainerMetadata = randomContainerMetadata()
        setupFooPod = partial(
            cls.setupPod,
            cls.fooPodMetadata,
            cls.fooContainerMetadata,
            cls.fooComponentName,
            cls.fooLabels,
        )
        cls.initiatedFooId = setupFooPod(Phase.RunPodSandbox)
        cls.createdFooId = setupFooPod(Phase.CreateContainer)
        cls.runningFooId = setupFooPod(Phase.StartContainer)
        cls.stoppedFooId = setupFooPod(Phase.StopContainer)
        cls.removedFooId = setupFooPod(Phase.RemoveContainer)
        cls.killedFooId = setupFooPod(Phase.StopPodSandbox)

        # We only need one pod / container with the 'bar' labels.
        cls.barPodMetadata = randomPodMetadata()
        cls.barContainerMetadata = randomContainerMetadata()
        setupBarPod = partial(
            cls.setupPod,
            cls.barPodMetadata,
            cls.barContainerMetadata,
            cls.barComponentName,
            cls.barLabels,
        )
        cls.createdBarId = setupBarPod(Phase.CreateContainer)

    @classmethod
    def setupPod(
        cls,
        podMetadata: PodSandboxMetadata,
        containerMetadata: ContainerMetadata,
        componentName: str,
        labels: dict[str, str],
        until: Phase,
    ) -> str:
        """
        Create a single pod with specified metadata,
        in the lifecycle phase specified by `until`.
        """
        podSandboxId = cls.runtimeService.RunPodSandbox(
            RunPodSandboxRequest(
                runtime_handler='workd',
                config=PodSandboxConfig(
                    metadata=podMetadata,
                    hostname='foobar',
                    labels=labels,
                ),
            ),
        ).pod_sandbox_id
        if until == Phase.RunPodSandbox:
            return podSandboxId

        cls.runtimeService.CreateContainer(
            CreateContainerRequest(
                pod_sandbox_id=podSandboxId,
                config=ContainerConfig(
                    metadata=containerMetadata,
                    image=ImageSpec(
                        image=componentName,
                        runtime_handler='workd',
                    ),
                    labels=labels,
                ),
            ),
        )
        if until == Phase.CreateContainer:
            return podSandboxId

        cls.runtimeService.StartContainer(
            StartContainerRequest(container_id=podSandboxId),
        )
        if until == Phase.StartContainer:
            return podSandboxId

        cls.runtimeService.StopContainer(
            StopContainerRequest(container_id=podSandboxId),
        )
        if until == Phase.StopContainer:
            return podSandboxId

        cls.runtimeService.RemoveContainer(
            RemoveContainerRequest(container_id=podSandboxId),
        )
        if until == Phase.RemoveContainer:
            return podSandboxId

        cls.runtimeService.StopPodSandbox(
            StopPodSandboxRequest(pod_sandbox_id=podSandboxId),
        )
        if until == Phase.StopPodSandbox:
            return podSandboxId

        cls.deletePod(podSandboxId)
        raise ValueError(f'Unexpected phase: {until}')

    def assertPodSandbox(
        self,
        podSandbox: PodSandbox,
        metadata: PodSandboxMetadata,
        state: PodSandboxState,
        labels: dict[str, str],
    ):
        """Assert on every detail return by `ListPodSandbox`."""
        self.assertEqual(podSandbox.metadata, metadata)
        self.assertEqual(podSandbox.state, state)
        now = time_ns()
        self.assertTrue(
            now - TEST_TIMEOUT_NANOSECONDS
            < podSandbox.created_at
            < now + TEST_TIMEOUT_NANOSECONDS
        )
        self.assertEqual(podSandbox.labels, labels)
        self.assertEqual(podSandbox.annotations, {})
        self.assertEqual(podSandbox.runtime_handler, 'workd')

    def assertContainer(
        self,
        container: Container,
        metadata: ContainerMetadata,
        state: ContainerState,
        labels: dict[str, str],
    ):
        """Assert on every detail return by `ListPodContainers`."""
        self.assertEqual(container.metadata, metadata)
        self.assertEqual(container.image, ImageSpec())
        self.assertEqual(container.image_ref, 'TODO')
        self.assertEqual(container.state, state)
        now = time_ns()
        self.assertTrue(
            now - TEST_TIMEOUT_NANOSECONDS
            < container.created_at
            < now + TEST_TIMEOUT_NANOSECONDS
        )
        self.assertEqual(container.labels, labels)
        self.assertEqual(container.annotations, {})
        self.assertEqual(container.image_id, 'TODO')

    def test_ListPodSandbox_NoFilter(self):
        response = self.runtimeService.ListPodSandbox(ListPodSandboxRequest())

        self.assertEqual(len(response.items), 7)

        self.assertPodSandbox(
            findById(response.items, self.initiatedFooId),
            self.fooPodMetadata,
            PodSandboxState.SANDBOX_READY,
            self.fooLabels,
        )
        self.assertPodSandbox(
            findById(response.items, self.createdFooId),
            self.fooPodMetadata,
            PodSandboxState.SANDBOX_READY,
            self.fooLabels,
        )
        self.assertPodSandbox(
            findById(response.items, self.runningFooId),
            self.fooPodMetadata,
            PodSandboxState.SANDBOX_READY,
            self.fooLabels,
        )
        self.assertPodSandbox(
            findById(response.items, self.stoppedFooId),
            self.fooPodMetadata,
            PodSandboxState.SANDBOX_READY,
            self.fooLabels,
        )
        self.assertPodSandbox(
            findById(response.items, self.removedFooId),
            self.fooPodMetadata,
            PodSandboxState.SANDBOX_READY,
            self.fooLabels,
        )
        self.assertPodSandbox(
            findById(response.items, self.killedFooId),
            self.fooPodMetadata,
            PodSandboxState.SANDBOX_NOTREADY,
            self.fooLabels,
        )
        self.assertPodSandbox(
            findById(response.items, self.createdBarId),
            self.barPodMetadata,
            PodSandboxState.SANDBOX_READY,
            self.barLabels,
        )

    def test_ListContainers_NoFilter(self):
        response = self.runtimeService.ListContainers(ListContainersRequest())

        self.assertEqual(len(response.containers), 4)

        self.assertContainer(
            findById(response.containers, self.createdFooId),
            self.fooContainerMetadata,
            ContainerState.CONTAINER_CREATED,
            self.fooLabels,
        )
        self.assertContainer(
            findById(response.containers, self.runningFooId),
            self.fooContainerMetadata,
            ContainerState.CONTAINER_RUNNING,
            self.fooLabels,
        )
        self.assertContainer(
            findById(response.containers, self.stoppedFooId),
            self.fooContainerMetadata,
            ContainerState.CONTAINER_EXITED,
            self.fooLabels,
        )
        self.assertContainer(
            findById(response.containers, self.createdBarId),
            self.barContainerMetadata,
            ContainerState.CONTAINER_CREATED,
            self.barLabels,
        )

    def test_ListPodSandbox_FilterById(self):
        response = self.runtimeService.ListPodSandbox(
            ListPodSandboxRequest(filter=PodSandboxFilter(id=self.removedFooId))
        )

        self.assertEqual(len(response.items), 1)
        findById(response.items, self.removedFooId)

    def test_ListContainers_FilterById(self):
        response = self.runtimeService.ListContainers(
            ListContainersRequest(filter=ContainerFilter(id=self.runningFooId))
        )

        self.assertEqual(len(response.containers), 1)
        findById(response.containers, self.runningFooId)

    def test_ListPodSandbox_FilterByStateReady(self):
        response = self.runtimeService.ListPodSandbox(
            ListPodSandboxRequest(
                filter=PodSandboxFilter(
                    state=PodSandboxStateValue(state=PodSandboxState.SANDBOX_READY)
                )
            )
        )

        self.assertEqual(len(response.items), 6)
        findById(response.items, self.initiatedFooId)
        findById(response.items, self.createdFooId)
        findById(response.items, self.runningFooId)
        findById(response.items, self.stoppedFooId)
        findById(response.items, self.removedFooId)
        findById(response.items, self.createdBarId)

    def test_ListPodSandbox_FilterByStateNotready(self):
        response = self.runtimeService.ListPodSandbox(
            ListPodSandboxRequest(
                filter=PodSandboxFilter(
                    state=PodSandboxStateValue(state=PodSandboxState.SANDBOX_NOTREADY)
                )
            )
        )

        self.assertEqual(len(response.items), 1)
        findById(response.items, self.killedFooId)

    def test_ListContainers_FilterByStateCreated(self):
        response = self.runtimeService.ListContainers(
            ListContainersRequest(
                filter=ContainerFilter(
                    state=ContainerStateValue(state=ContainerState.CONTAINER_CREATED)
                )
            )
        )

        self.assertEqual(len(response.containers), 2)
        findById(response.containers, self.createdFooId)
        findById(response.containers, self.createdBarId)

    def test_ListContainers_FilterByStateRunning(self):
        response = self.runtimeService.ListContainers(
            ListContainersRequest(
                filter=ContainerFilter(
                    state=ContainerStateValue(state=ContainerState.CONTAINER_RUNNING)
                )
            )
        )

        self.assertEqual(len(response.containers), 1)
        findById(response.containers, self.runningFooId)

    def test_ListContainers_FilterByStateExited(self):
        response = self.runtimeService.ListContainers(
            ListContainersRequest(
                filter=ContainerFilter(
                    state=ContainerStateValue(state=ContainerState.CONTAINER_EXITED)
                )
            )
        )

        self.assertEqual(len(response.containers), 3)
        findById(response.containers, self.stoppedFooId)
        findById(response.containers, self.removedFooId)
        findById(response.containers, self.killedFooId)

    def test_ListContainers_FilterByStateUnknown(self):
        response = self.runtimeService.ListContainers(
            ListContainersRequest(
                filter=ContainerFilter(
                    state=ContainerStateValue(state=ContainerState.CONTAINER_UNKNOWN)
                )
            )
        )

        self.assertEqual(len(response.containers), 0)

    def test_ListPodSandbox_FilterByLabels(self):
        response = self.runtimeService.ListPodSandbox(
            ListPodSandboxRequest(
                filter=PodSandboxFilter(
                    label_selector={
                        'vimana.host/domain': self.fooDomain,
                        'vimana.host/service': self.fooService,
                        'vimana.host/version': self.fooVersion,
                    }
                )
            )
        )

        self.assertEqual(len(response.items), 6)
        findById(response.items, self.initiatedFooId)
        findById(response.items, self.createdFooId)
        findById(response.items, self.runningFooId)
        findById(response.items, self.stoppedFooId)
        findById(response.items, self.removedFooId)
        findById(response.items, self.killedFooId)

    def test_ListContainers_FilterByLabels(self):
        response = self.runtimeService.ListContainers(
            ListContainersRequest(
                filter=ContainerFilter(
                    label_selector={
                        'vimana.host/domain': self.barDomain,
                        'vimana.host/service': self.barService,
                        'vimana.host/version': self.barVersion,
                    }
                )
            )
        )

        self.assertEqual(len(response.containers), 1)
        findById(response.containers, self.createdBarId)

    def test_ListPodSandbox_FilterByStateAndLabels(self):
        response = self.runtimeService.ListPodSandbox(
            ListPodSandboxRequest(
                filter=PodSandboxFilter(
                    state=PodSandboxStateValue(state=PodSandboxState.SANDBOX_READY),
                    label_selector={'vimana.host/domain': self.fooDomain},
                )
            )
        )

        self.assertEqual(len(response.items), 5)
        findById(response.items, self.initiatedFooId)
        findById(response.items, self.createdFooId)
        findById(response.items, self.runningFooId)
        findById(response.items, self.stoppedFooId)
        findById(response.items, self.removedFooId)

    def test_ListContainers_FilterByStateAndLabels(self):
        response = self.runtimeService.ListContainers(
            ListContainersRequest(
                filter=ContainerFilter(
                    state=ContainerStateValue(state=ContainerState.CONTAINER_CREATED),
                    label_selector={'vimana.host/domain': self.fooDomain},
                )
            )
        )

        self.assertEqual(len(response.containers), 1)
        findById(response.containers, self.createdFooId)


def findById(items: list[PodSandbox | Container], id: str) -> PodSandbox | Container:
    for item in items:
        if item.id == id:
            return item
    raise AssertionError(f"No result found with id '{id}'.")


def randomPodMetadata():
    return PodSandboxMetadata(
        name=hexUuid(),
        uid=hexUuid(),
        namespace='default',
        attempt=1,
    )


def randomContainerMetadata():
    return ContainerMetadata(
        name=hexUuid(),
        attempt=1,
    )


if __name__ == '__main__':
    main()
