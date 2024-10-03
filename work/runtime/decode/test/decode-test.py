
from unittest import (TestCase, main)

from work.runtime.decode.test.messages_pb2 import ScalarTypes

class ScalarTypesTest(TestCase):
    def test_bytes(self):
        msg = ScalarTypes(
            bytes_implicit = b'my bytes!',
        )

if __name__ == '__main__':
    main()