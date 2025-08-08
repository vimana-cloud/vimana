"""
Test that a Helm chart renders correctly against a set of resources using `helm template`.
The spec of one of the resources, referred to as the target,
is used as the input values of the templates.
The rest of the resources are made available via a fake K8s API server
to enable Helm's `lookup` function to work.
"""

from os import getcwd
from sys import path

# Remove the current working directory from the Python path.
# Otherwise, the `operator` directory would shadow the standard `operator` library.
# We don't need relative imports anyway.
path.remove(getcwd())

from argparse import ArgumentParser
from collections import defaultdict
from contextlib import closing
from difflib import context_diff
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, HTTPServer
from json import dumps as dumpJson
from json import loads as loadJson
from os import makedirs
from os.path import dirname
from os.path import join as joinPath
from random import randrange
from re import compile as compileRegex
from shutil import copy
from socket import AF_INET, SOCK_STREAM, socket
from subprocess import PIPE, Popen
from sys import exit, stderr
from tempfile import NamedTemporaryFile, TemporaryDirectory
from threading import Thread
from typing import Dict, Tuple
from urllib.parse import urlparse

from yaml import safe_load_all as loadAllYaml

LOCALHOST = 'localhost'


def main(
    helmPath: str,
    chartFiles: str,
    resourcesPath: str,
    targetName: str,
    expectedPath: str,
):
    # Organize all the resources in the resources file by kind then name.
    # Also, find whichever resource is identified as the target,
    # and extract its spec as the values object.
    values = None
    resourcesByKindThenName = defaultdict(dict)
    with open(resourcesPath, 'r') as resourcesFile:
        for resource in loadAllYaml(resourcesFile):
            name = resource['metadata']['name']
            if name == targetName:
                values = resource['spec']
            resourcesByKindThenName[resource['kind']][name] = resource
    if values is None:
        raise RuntimeError(
            f"Target name '{targetName}' not found in the resources file"
        )

    apiPort = _findAvailablePort()
    apiServer = FakeK8sApiServer((LOCALHOST, apiPort), resourcesByKindThenName)
    apiServerThread = Thread(target=apiServer.serve_forever, daemon=True)
    apiServerThread.start()

    try:
        # Save the target values to a temporary file that can be access by Helm.
        with NamedTemporaryFile('w+') as valuesFile:
            valuesFile.write(dumpJson(values))
            valuesFile.flush()

            # Copy the chart files into a temporary directory,
            # giving them their proper logical layout.
            chartFiles = loadJson(chartFiles)
            with TemporaryDirectory() as chartDir:
                for logicalPath, runfilePath in chartFiles.items():
                    chartPath = joinPath(chartDir, logicalPath)
                    makedirs(dirname(chartPath), exist_ok=True)
                    copy(runfilePath, chartPath, follow_symlinks=True)

                # Run `helm template` to render the chart.
                command = [
                    helmPath,
                    'template',
                    '--debug',
                    '--generate-name',
                    '--dry-run=server',
                    f'--kube-apiserver=http://{LOCALHOST}:{apiPort}',
                    '--values={}'.format(valuesFile.name),
                    chartDir,
                ]
                process = Popen(command, stdout=PIPE, stderr=PIPE, text=True)
                helmStdout, helmStderr = process.communicate()

        if process.returncode != 0:
            print(f"Error running 'helm template':\n{helmStderr}", file=stderr)
            exit(1)
        actual = helmStdout.splitlines(keepends=True)

        # If there are any differences between the rendered chart and the expected file,
        # print them to stderr and fail the test.
        with open(expectedPath, 'r') as expectFile:
            expect = [line for line in expectFile]
        diff = list(context_diff(actual, expect, fromfile='actual', tofile='expected'))
        if len(diff) > 0:
            stderr.writelines(diff)
            exit(1)

    finally:
        apiServer.shutdown()


class FakeK8sApiServer(HTTPServer):
    def __init__(
        self,
        address: Tuple[str, int],
        resourcesByKindThenName: Dict[str, Dict[str, Dict[str, object]]],
    ):
        self.resourcesByKindThenName = resourcesByKindThenName
        super().__init__(address, FakeK8sApiServerRequestHandler)

    def resource(self, kind: str, name: str) -> Dict[str, object]:
        """
        Return a single K8s resource with the given kind and name.
        Return `None` if the resource does not exist.
        """
        return self.resourcesByKindThenName[kind].get(name)

    def resourceList(self, kind: str) -> Dict[str, object]:
        """
        Return a K8s resource representing a list of resources of a particular kind.
        """
        return {
            'apiVersion': 'api.vimana.host/v1alpha1',
            'kind': f'{kind}List',
            'metadata': {'resourceVersion': '1'},
            'items': list(self.resourcesByKindThenName[kind].values()),
        }


_getResourcePattern = compileRegex(
    r'^/apis/api.vimana.host/v1alpha1/namespaces/default/(\w+)/(\w+)$'
)
_listResourcesPattern = compileRegex(
    r'^/apis/api.vimana.host/v1alpha1/namespaces/default/(\w+)$'
)
_kindByPlural = {
    'vimanas': 'Vimana',
    'domains': 'Domain',
    'servers': 'Server',
    'components': 'Component',
}


class FakeK8sApiServerRequestHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        url = urlparse(self.path)
        # Endpoint to discover the registered resource types for the API.
        if url.path == '/apis/api.vimana.host/v1alpha1':
            self.okJson(
                {
                    'resources': [
                        _apiResource('Vimana'),
                        _apiResource('Domain'),
                        _apiResource('Server'),
                        _apiResource('Component'),
                    ]
                }
            )
        # Endpoint to get a single resource by kind and name.
        elif match := _getResourcePattern.match(url.path):
            self.okJson(
                self.server.resource(_kindByPlural[match.group(1)], match.group(2))
            )
        # Endpoint to list all resources of a certain kind.
        elif match := _listResourcesPattern.match(url.path):
            self.okJson(self.server.resourceList(_kindByPlural[match.group(1)]))
        else:
            self.unexpected()

    def okJson(self, response: Dict[str, object]):
        """Respond with an OK status code and a JSON-encoded response body."""
        self.send_response(HTTPStatus.OK.value)
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(dumpJson(response).encode())

    def unexpected(self):
        """Respond with a NOT IMPLEMENTED status code."""
        self.send_response(HTTPStatus.NOT_IMPLEMENTED.value)
        self.end_headers()
        self.wfile.write(f'Fake K8s API got unexpected request: {self.path}'.encode())


def _apiResource(
    kind: str,
    name: str = None,
    singularName: str = None,
) -> Dict[str, object]:
    """
    Return a K8s `APIResource` object describing a Vimana API resource
    with the given kind, name, and singular name.
    The name and singular name are derived from the kind if omitted.
    """
    name = name or f'{kind.lower()}s'
    singularName = singularName or f'{kind.lower()}'
    return {
        'name': name,
        'singularName': singularName,
        'namespaced': True,
        'group': 'api.vimana.host',
        'version': 'v1alpha1',
        'kind': kind,
        'verbs': ['get', 'list'],
    }


def _findAvailablePort(attempts: int = 5) -> int:
    """Find an available TCP port by random probing."""
    for i in range(attempts):
        # Pick a random port in the ephemeral range: [49152–65536).
        port = randrange(49152, 65536)
        if _isPortAvailable(port):
            return port
    raise RuntimeError(f'Could not find an open port after {attempts} attempts.')


def _isPortAvailable(port: int) -> bool:
    with closing(socket(AF_INET, SOCK_STREAM)) as sock:
        errno = sock.connect_ex((LOCALHOST, port))
        # Error codes 111 (on Linux) and 61 (on Mac)
        # seem to indicate connection refused (the port is available).
        if errno == 111 or errno == 61:
            return True
    return False


if __name__ == '__main__':
    parser = ArgumentParser(description=__doc__)
    parser.add_argument('--helm', help='Path to the Helm executable')
    parser.add_argument(
        '--chart-files',
        help='JSON-encoded object mapping logical chart paths to actual runfile paths',
    )
    parser.add_argument('--resources', help='Path to the custom resource(s) YAML file')
    parser.add_argument(
        '--target',
        help='Name of the custom resource whose spec defines the values for the chart',
    )
    parser.add_argument(
        '--expected', help='Path to the file containing the expected YAML output'
    )
    args = parser.parse_args()

    main(args.helm, args.chart_files, args.resources, args.target, args.expected)
