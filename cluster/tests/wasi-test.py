"""A more-or-less comprehensive set of E2E test cases."""

from time import time
from unittest import main

from cluster.tests.components.wasi_pb2 import ResultResponse, StringRequest, U64Request
from cluster.tests.components.wasi_pb2_grpc import WasiExerciseStub

from cluster.tests.util import E2eTestCase


class WasiTest(E2eTestCase):
    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.client = WasiExerciseStub(cls.secureChannel('wasi.test'))

    def test_noFilesystem(self):
        request = StringRequest(input='/usr/bin/bash')

        response = self.client.AssertNoFilesystem(request)

        self.assertEqual(response, ResultResponse(passed=True))

    def test_internetAccess(self):
        request = StringRequest(input='https://www.google.com/')

        response = self.client.AssertInternetAccess(request)

        self.assertEqual(response, ResultResponse(passed=True))

    def test_internetAccess_domainNotFound(self):
        request = StringRequest(input='https://theresnowaythisexists.wikipedia.org')

        response = self.client.AssertInternetAccess(request)

        self.assertEqual(
            response,
            ResultResponse(
                reason='Got error response: ErrorCode::DnsError({})'.format(
                    'DnsErrorPayload { rcode: Some("address not available"), info-code: Some(0) }'
                )
            ),
        )

    def test_clockAccess(self):
        request = U64Request(input=int(time()))

        response = self.client.AssertClockAccess(request)

        self.assertEqual(response, ResultResponse(passed=True))

    def test_clockAccess_wrongTime(self):
        request = U64Request(input=int(time() - 5))

        response = self.client.AssertClockAccess(request)

        self.assertFalse(response.passed)
        self.assertIn('Wrong time', response.reason)


if __name__ == '__main__':
    main()
