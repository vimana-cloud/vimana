"""Tests for `ListPodSandbox` and `ListContainers`."""

from enum import Enum, auto
from functools import partial
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
        setupFooPod = partial(cls.setupPod, cls.fooComponentName, cls.fooLabels)
        cls.initiatedFooId = setupFooPod(Phase.RunPodSandbox)
        cls.createdFooId = setupFooPod(Phase.CreateContainer)
        cls.runningFooId = setupFooPod(Phase.StartContainer)
        cls.stoppedFooId = setupFooPod(Phase.StopContainer)
        cls.removedFooId = setupFooPod(Phase.RemoveContainer)
        cls.killedFooId = setupFooPod(Phase.StopPodSandbox)

        # We only need one pod / container with the 'bar' labels.
        setupBarPod = partial(cls.setupPod, cls.barComponentName, cls.barLabels)
        cls.createdBarId = setupBarPod(Phase.CreateContainer)

    @classmethod
    def setupPod(cls, componentName: str, labels: dict[str, str], until: Phase) -> str:
        podSandboxId = cls.runtimeService.RunPodSandbox(
            RunPodSandboxRequest(
                runtime_handler='workd',
                config=PodSandboxConfig(
                    metadata=randomPodMetadata(),
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
                    metadata=randomContainerMetadata(),
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

    def test_ListPodSandbox_NoFilter(self):
        response = self.runtimeService.ListPodSandbox(ListPodSandboxRequest())

        # TODO: Should this return 7 pods?
        self.assertEqual(len(response.items), 6)

        initiatedFooPod = findById(response.items, self.initiatedFooId)
        createdFooPod = findById(response.items, self.createdFooId)
        runningFooPod = findById(response.items, self.runningFooId)
        stoppedFooPod = findById(response.items, self.stoppedFooId)
        removedFooPod = findById(response.items, self.removedFooId)
        # killedFooPod = findById(response.items, self.killedFooId)
        createdBarPod = findById(response.items, self.createdBarId)

        # TODO: Assert details.

    def test_ListContainers_NoFilter(self):
        response = self.runtimeService.ListContainers(ListContainersRequest())

        # TODO: Should this return 4 containers?
        self.assertEqual(len(response.containers), 7)

        initiatedContainer = findById(response.containers, self.initiatedFooId)
        createdContainer = findById(response.containers, self.createdFooId)
        runningContainer = findById(response.containers, self.runningFooId)
        stoppedContainer = findById(response.containers, self.stoppedFooId)
        removedContainer = findById(response.containers, self.removedFooId)
        killedContainer = findById(response.containers, self.killedFooId)
        createdBarPod = findById(response.containers, self.createdBarId)

        # TODO: Assert details.

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

        # TODO: Should this return 1 pod?
        self.assertEqual(len(response.items), 0)
        # findById(response.items, self.killedFooId)

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

        # TODO: Should this return 0 pods?
        self.assertEqual(len(response.containers), 1)

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

        # TODO: Should this return 6 pods?
        self.assertEqual(len(response.items), 5)
        findById(response.items, self.initiatedFooId)
        findById(response.items, self.createdFooId)
        findById(response.items, self.runningFooId)
        findById(response.items, self.stoppedFooId)
        findById(response.items, self.removedFooId)
        # findById(response.items, self.killedFooId)

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
