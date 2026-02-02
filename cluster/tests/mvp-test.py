"""A basic E2E test that exercises the simplest possible service."""

from unittest import main

from cluster.tests.components.mvp_pb2 import HelloRequest, HelloResponse
from cluster.tests.components.mvp_pb2_grpc import ThisOldTropeStub
from python.runfiles import Runfiles

from cluster.tests.util import E2eTestCase

runfiles = Runfiles.Create()

ROOT_CERTIFICATE_PATH = runfiles.Rlocation(
    '_main/cluster/tests/mvp-test.certificates.root.cert'
)


class MvpTest(E2eTestCase(ROOT_CERTIFICATE_PATH)):
    def test_mvp(self):
        client = ThisOldTropeStub(self.secureChannel('mvp.test'))

        response = client.HelloWorld(HelloRequest(name='World'))

        self.assertEqual(response, HelloResponse(message='Hello, World!'))


if __name__ == '__main__':
    main()
