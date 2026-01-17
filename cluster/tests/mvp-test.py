"""A basic E2E test that exercises the simplest possible service."""

from datetime import timedelta
from unittest import TestCase, main

import grpc
from cluster.tests.components.mvp_pb2 import HelloRequest, HelloResponse
from cluster.tests.components.mvp_pb2_grpc import ThisOldTropeStub
from python.runfiles import Runfiles

runfiles = Runfiles.Create()

ROOT_CERTIFICATE_PATH = runfiles.Rlocation(
    '_main/cluster/tests/mvp-test.certificates.root.cert'
)


class MvpTest(TestCase):
    @classmethod
    def setUpClass(cls):
        with open(ROOT_CERTIFICATE_PATH, 'rb') as rootCertificateFile:
            cls.rootCertificate = rootCertificateFile.read()

    def test_mvp(self):
        client = ThisOldTropeStub(self.secureChannel('mvp.test'))

        response = client.HelloWorld(HelloRequest(name='World'))

        self.assertEqual(response, HelloResponse(message='Hello, World!'))

    @classmethod
    def secureChannel(cls, domain: str, timeout: timedelta = timedelta(seconds=15)):
        channel = grpc.secure_channel(
            domain,
            grpc.ssl_channel_credentials(root_certificates=cls.rootCertificate),
        )
        # Sometimes there is a delay between when the gateway is programmed
        # and when it has finished setting up its listeners, TLS certificates, and routes.
        grpc.channel_ready_future(channel).result(timeout=timeout.total_seconds())
        return channel


if __name__ == '__main__':
    main()
