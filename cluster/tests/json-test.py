"""Test gRPC-JSON transcoding by making HTTP/JSON requests to a gRPC service."""

from unittest import main

import requests
from python.runfiles import Runfiles

from cluster.tests.util import E2eTestCase

runfiles = Runfiles.Create()

ROOT_CERTIFICATE_PATH = runfiles.Rlocation(
    '_main/cluster/tests/json-test.certificates.root.cert'
)


class MvpTest(E2eTestCase(ROOT_CERTIFICATE_PATH)):
    def test_helloWorld_json(self):
        # Make HTTP/JSON request (the transcoder converts this to gRPC).
        response = requests.get(
            url='https://json.test/echo',
            json={'message': 'This is the message.'},
            headers={'Content-Type': 'application/json'},
            verify=ROOT_CERTIFICATE_PATH,
            timeout=15,
        )

        self.assertEqual(response.status_code, 200)
        self.assertEqual(
            response.json(),
            {'message': 'This is the message. This is the message.'},
        )


if __name__ == '__main__':
    main()
