"""Test harness and helper functions for E2E tests."""

from datetime import timedelta
from unittest import TestCase

import grpc


def E2eTestCase(rootCertificatePath: str) -> type[TestCase]:
    class E2eTestCase(TestCase):
        @classmethod
        def setUpClass(cls):
            super().setUpClass()
            with open(rootCertificatePath, 'rb') as rootCertificateFile:
                cls.rootCertificate = rootCertificateFile.read()

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

    return E2eTestCase
