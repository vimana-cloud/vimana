from os import listdir
from os.path import exists, splitext
from os.path import join as joinPath
from typing import Callable
from unittest import TestCase, main

from cli.protoc.tests.util import protoc

DATA_PATH = joinPath('cli', 'protoc', 'tests', 'data')


# The test class is populated dynamically
# based on the content of the test data directory.
class ProtocPluginTest(TestCase):
    pass


def generateCase(rootName: str) -> Callable[[TestCase], None]:
    """Generate a test case based on a group of test data files that share a root name."""

    def case(self):
        protoFile = joinPath(DATA_PATH, f'{rootName}.proto')
        witFile = joinPath(DATA_PATH, f'{rootName}.wit')
        self.assertTrue(exists(protoFile), f"File '{protoFile}' is missing")
        self.assertTrue(exists(witFile), f"File '{witFile}' is missing")

        result = protoc(protoFile)

        self.maxDiff = None
        with open(witFile, 'r') as expectedWit:
            self.assertEqual(result.wit, expectedWit.read())

    return case


# Each test case is defined by a group of files in the data directory
# which all share a filename root but differ in their extension.
for rootName in set(splitext(path)[0] for path in listdir(DATA_PATH)):
    setattr(ProtocPluginTest, f'test_{rootName}', generateCase(rootName))


if __name__ == '__main__':
    main()
