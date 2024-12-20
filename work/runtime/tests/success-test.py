from unittest import TestCase, main

from work.runtime.tests.util import WorkdTester
from work.runtime.tests.api_pb2 import RunPodSandboxRequest, VersionRequest

class SuccessTest(TestCase):

    @classmethod
    def setUpClass(cls):
        # Use a single, long-running `workd` instance for every test.
        cls.workd = WorkdTester()
        # Convenience shorthands.
        cls.runtimeService = cls.workd.runtimeService
        cls.imageService = cls.workd.imageService

    @classmethod
    def tearDownClass(cls):
        # Shut down the various servers and subprocesses.
        del cls.workd

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

if __name__ == '__main__':
    main()