"""A basic E2E test that walks through most of Vimana's functionality."""

from os import environ
from unittest import TestCase, main

import grpc

from work.runtime.tests.components.adder_pb2 import AddFloatsRequest, AddFloatsResponse
from work.runtime.tests.components.adder_pb2_grpc import AdderServiceStub

# https://github.com/grpc/grpc/blob/v1.71.0/doc/environment_variables.md
environ['GRPC_DEFAULT_SSL_ROOTS_FILE_PATH'] = 'e2e/walkthrough-bootstrap.root.cert'


class Walkthough(TestCase):
    def test_WIP(self):
        adderClient = AdderServiceStub(
            grpc.secure_channel('api.vimana.host', grpc.ssl_channel_credentials()),
        )

        response = adderClient.AddFloats(AddFloatsRequest(x=3.5, y=-1.2))

        self.assertEqual(response, AddFloatsResponse(result=2.3))


if __name__ == '__main__':
    main()
