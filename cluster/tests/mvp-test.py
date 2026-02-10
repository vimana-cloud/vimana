"""A basic E2E test that exercises the simplest possible service."""

from unittest import main

from cluster.tests.components.mvp_pb2 import HelloRequest, HelloResponse
from cluster.tests.components.mvp_pb2_grpc import ThisOldTropeStub

from cluster.tests.util import E2eTestCase


class MvpTest(E2eTestCase):
    def test_mvp(self):
        client = ThisOldTropeStub(self.secureChannel('mvp.test'))

        response = client.HelloWorld(HelloRequest(name='World'))

        self.assertEqual(response, HelloResponse(message='Hello, World!'))


if __name__ == '__main__':
    main()
