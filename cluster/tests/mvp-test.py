"""A basic E2E test that walks through some of Vimana's functionality."""

from os import environ
from unittest import TestCase, main

import grpc
from cluster.tests.components.mvp_pb2 import HelloRequest, HelloResponse
from cluster.tests.components.mvp_pb2_grpc import ThisOldTropeStub

# https://github.com/grpc/grpc/blob/v1.76.0/doc/environment_variables.md
environ['GRPC_DEFAULT_SSL_ROOTS_FILE_PATH'] = (
    'cluster/tests/mvp-test.certificates.root.cert'
)


class Walkthough(TestCase):
    def test_WIP(self):
        mvpClient = ThisOldTropeStub(
            grpc.secure_channel('mvp.test', grpc.ssl_channel_credentials()),
        )

        response = mvpClient.HelloWorld(HelloRequest(name='World'))

        self.assertEqual(response, HelloResponse(message='Hello, World!'))


if __name__ == '__main__':
    main()
