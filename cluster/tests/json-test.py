"""Test gRPC-JSON transcoding by making HTTP/JSON requests to a gRPC service."""

from contextlib import contextmanager
from tempfile import NamedTemporaryFile
from time import sleep, time
from unittest import main

import requests

from cluster.tests.util import E2eTestCase


@contextmanager
def tempFile(content: bytes):
    """Write PEM bytes to a temporary file for use with requests' verify parameter."""
    with NamedTemporaryFile() as tempFile:
        tempFile.write(content)
        tempFile.flush()
        yield tempFile.name


class MvpTest(E2eTestCase):
    def test_helloWorld_json(self):
        with tempFile(self.additionalCas()) as bundle:
            # Retry until the gateway accepts HTTPS connections.
            # There is a delay between when the gateway is programmed
            # and when it has obtained its TLS certificate from the ACME server.
            deadline = time() + 300
            while True:
                try:
                    response = requests.get(
                        url='https://json.test/echo',
                        json={'message': 'This is the message.'},
                        headers={'Content-Type': 'application/json'},
                        verify=bundle,
                        timeout=15,
                    )
                    break
                except requests.ConnectionError:
                    if time() >= deadline:
                        raise
                    sleep(5)

        self.assertEqual(response.status_code, 200)
        self.assertEqual(
            response.json(),
            {'message': 'This is the message. This is the message.'},
        )


if __name__ == '__main__':
    main()
