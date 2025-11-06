from dataclasses import dataclass
from os.path import abspath
from os.path import join as joinPath
from subprocess import Popen
from tempfile import TemporaryDirectory

PROTOC_PATH = joinPath('..', 'protobuf+', 'protoc')
PLUGIN_PATH = joinPath('compiler', 'protoc-gen-vimana')


@dataclass(kw_only=True)
class ProtocOutput:
    wit: str


def protoc(*files, include=None) -> ProtocOutput:
    """
    Helper method to invoke `protoc` with the Vimana plugin.
    """
    with TemporaryDirectory() as output:
        args = (
            [
                PROTOC_PATH,
                f'--plugin={abspath(PLUGIN_PATH)}',
                f'--vimana_out={output}',
            ]
            + [f'--proto_path={path}' for path in (include or [])]
            + list(files)
        )
        if (status := Popen(args).wait()) != 0:
            raise RuntimeError(f'Failed executing protoc (status={status})')

        with open(joinPath(output, 'server.wit'), 'r') as witFile:
            wit = witFile.read()
        return ProtocOutput(wit=wit)
