"""
These tests confirm basic operation of the compiler "in isolation"
by verifying that compiling each Protobuf file under the `data/` subdirectory
results in the corresponding expected WIT and metadata files.

It does not involve any of the actual encoding / decoding logic from the runtime.
For a much more comprehensive test suite that also covers en-/de-coding,
see the conformance test.

The output metadata file is converted to Protobuf text format,
rather than the binary format produced by the compiler,
for easier manual editing and verification.
"""

from os import listdir
from os.path import exists, splitext
from os.path import join as joinPath
from typing import Callable
from unittest import TestCase, main

from python.runfiles import Runfiles

from compiler.tests.util import protoc

runfiles = Runfiles.Create()

DATA_PATH = runfiles.Rlocation('_main/compiler/tests/data')


# The test class is populated dynamically
# based on the content of the test data directory.
class ProtocPluginTest(TestCase):
    pass


def generateTestCase(rootName: str) -> Callable[[TestCase], None]:
    """Generate a test case based on a group of test data files that share a root name."""

    def testCase(self):
        witFile = joinPath(DATA_PATH, f'{rootName}.wit')
        protoFile = joinPath(DATA_PATH, f'{rootName}.proto')
        metadataFile = joinPath(DATA_PATH, f'{rootName}.txtpb')
        for path in [witFile, protoFile, metadataFile]:
            self.assertTrue(exists(path), f"File '{path}' is missing")

        result = protoc(protoFile, include=[DATA_PATH])

        # Show diffs even if they're big.
        self.maxDiff = None

        with open(witFile, 'r') as expectedWit:
            self.assertEqual(result.wit, expectedWit.read())
        with open(metadataFile, 'r') as expectedMetadata:
            self.assertEqual(result.metadata, expectedMetadata.read())

    return testCase


# Each test case is defined by a group of files in the data directory
# which all share a filename root but differ in their extension.
for rootName in set(splitext(path)[0] for path in listdir(DATA_PATH)):
    setattr(ProtocPluginTest, f'test_{rootName}', generateTestCase(rootName))


if __name__ == '__main__':
    main()
