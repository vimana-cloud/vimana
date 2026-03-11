"""A more-or-less comprehensive set of E2E test cases."""

from time import time
from unittest import main

from cluster.tests.components.wasi_pb2 import Empty, StringRequest, U64Request
from cluster.tests.components.wasi_pb2_grpc import WasiExerciseStub
from grpc import RpcError, StatusCode

from cluster.tests.util import E2eTestCase


class WasiTest(E2eTestCase):
    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.client = WasiExerciseStub(cls.secureChannel('wasi.test'))

    def test_noFilesystem(self):
        request = StringRequest(input='/usr/bin/bash')

        response = self.client.AssertNoFilesystem(request)

        self.assertEqual(response, Empty())

    def test_internetAccess(self):
        request = StringRequest(input='https://www.google.com/')

        response = self.client.AssertInternetAccess(request)

        self.assertEqual(response, Empty())

    def test_internetAccess_domainNotFound(self):
        request = StringRequest(input='https://theresnowaythisexists.wikipedia.org')

        with self.assertRaises(RpcError) as cm:
            self.client.AssertInternetAccess(request)

        self.assertEqual(cm.exception.code(), StatusCode.INTERNAL)
        self.assertEqual(
            cm.exception.details(),
            'Got error response: ErrorCode::DnsError({})'.format(
                'DnsErrorPayload { rcode: Some("address not available"), info-code: Some(0) }'
            ),
        )

    def test_clockAccess(self):
        request = U64Request(input=int(time()))

        response = self.client.AssertClockAccess(request)

        self.assertEqual(response, Empty())

    def test_clockAccess_wrongTime(self):
        request = U64Request(input=int(time() - 5))

        with self.assertRaises(RpcError) as cm:
            self.client.AssertClockAccess(request)

        self.assertEqual(cm.exception.code(), StatusCode.INTERNAL)
        self.assertIn('Wrong time', cm.exception.details())


if __name__ == '__main__':
    main()
