from dataclasses import dataclass
from os.path import abspath, dirname
from os.path import join as joinPath
from subprocess import PIPE, run
from tempfile import TemporaryDirectory

from python.runfiles import Runfiles

runfiles = Runfiles.Create()

PROTOC_PATH = runfiles.Rlocation('_main/compiler/tests/protoc')
PLUGIN_PATH = runfiles.Rlocation('_main/compiler/protoc-gen-vimana')
METADATA_PROTO_PATH = runfiles.Rlocation('_main/runtime/metadata.proto')

WIT_FILENAME = joinPath('wit', 'server.wit')
METADATA_FILENAME = 'metadata.binpb'
METADATA_MESSAGE_NAME = 'vimana.runtime.Metadata'


@dataclass(kw_only=True)
class ProtocOutput:
    # Verbatim contents of the WIT output file.
    wit: str
    # Contents of the metadata output file converted to Protobuf text format.
    metadata: str


def protoc(*files, include=None) -> ProtocOutput:
    """
    Helper method to invoke `protoc` with the Vimana plugin.

    Also converts the binary metadata output to text format,
    so it can be more easily inspected and compared to an expected value
    using the standard line-based diff tool.
    """
    includeOptions = [f'--proto_path={path}' for path in (include or [])]

    with TemporaryDirectory() as output:
        # Generate the actual results by invoking `protoc`.
        run(
            [
                PROTOC_PATH,
                f'--plugin={abspath(PLUGIN_PATH)}',
                f'--vimana_out={output}',
                *includeOptions,
                *files,
            ],
            check=True,
        )

        # Read the results from the output files.
        with open(joinPath(output, WIT_FILENAME), 'r') as witFile:
            wit = witFile.read()
        with open(joinPath(output, METADATA_FILENAME), 'rb') as metadataFile:
            metadata = metadataFile.read()

    # The metadata file is in binary format.
    # For the purpose of this test, convert it to text format by invoking `protoc` again.
    metadata = run(
        [
            PROTOC_PATH,
            f'--decode={METADATA_MESSAGE_NAME}',
            f'--proto_path={dirname(METADATA_PROTO_PATH)}',
            *includeOptions,
            METADATA_PROTO_PATH,
        ],
        input=metadata,
        stdout=PIPE,
        check=True,
    ).stdout.decode()

    return ProtocOutput(wit=wit, metadata=metadata)
