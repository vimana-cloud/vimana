"""Create a node image for a Vimana cluster."""

from argparse import ArgumentParser
from contextlib import ExitStack
from datetime import datetime, timedelta
from os import getenv
from subprocess import DEVNULL, PIPE, run
from time import sleep
from typing import Any, Optional

from fabric import Connection
from google.api_core.extended_operation import ExtendedOperation
from google.cloud.compute_v1 import (
    AccessConfig,
    AttachedDisk,
    AttachedDiskInitializeParams,
    DisksClient,
    Image,
    ImagesClient,
    Instance,
    InstancesClient,
    Items,
    Metadata,
    NetworkInterface,
    Snapshot,
    SnapshotsClient,
)
from google.cloud.oslogin_v1 import OsLoginServiceClient
from google.cloud.oslogin_v1.common import SshPublicKey
from paramiko import RSAKey
from rich.console import Console

# Path to the `vimanad` binary.
VIMANAD_PATH = 'vimanad-x86_64-linux'
# Path to SystemD service file for `vimanad`.
VIMANAD_SERVICE_PATH = 'vimanad.prod.service'
# Path to containerd configuration file.
CONTAINERD_CONFIG_PATH = 'containerd-config.toml'
# Timeout for any long-running operation, in seconds.
TIMEOUT = 300

console = Console(stderr=True, highlight=False, soft_wrap=True)


def main(gcpProject: Optional[str]):
    version, clean = imageVersion()

    # TODO: Also support other cloud platforms.
    if gcpProject is not None:
        gcp(version, clean, gcpProject)


def imageVersion() -> tuple[str, bool]:
    """
    Return an appropriate version string for the node image,
    along with a flag indicating whether the Git working directory is currently clean.
    """
    # If this script is run from an unmodified commit of the repository,
    # it is considered clean.
    repoDirectory = getenv('BUILD_WORKSPACE_DIRECTORY')
    result = run(
        ['git', '-C', repoDirectory, 'diff-index', '--quiet', 'HEAD'],
        stdout=DEVNULL,
        stderr=DEVNULL,
    )
    if result.returncode == 0:
        # During clean builds,
        # the version of the image is the short form of the current commit hash.
        result = run(
            ['git', '-C', repoDirectory, 'rev-parse', '--short', 'HEAD'],
            stdout=PIPE,
            stderr=DEVNULL,
            text=True,
        )
        return (result.stdout.strip(), True)
    else:
        # During dirty builds,
        # the version is just the current Unix time in seconds.
        return (str(int(datetime.now().timestamp())), False)


def gcp(version: str, clean: bool, project: str):
    """Create a node image on, and for, Google Cloud Platform."""
    stockProject = 'debian-cloud'
    stockFamily = 'debian-12'
    instanceName = f'image-dummy-{version}'
    instanceZone = 'us-west1-a'
    instanceType = 'e2-medium'
    snapshotName = f'{instanceName}-snapshot'
    name = f'node-{version}'
    family = 'vimana' if clean else 'vimana-dirty'

    console.print(f'Image name: [bold]{name}[/bold]')
    console.print(f'Image family: [bold]{family}[/bold]')

    instances = InstancesClient()
    osLogin = OsLoginServiceClient()
    disks = DisksClient()
    snapshots = SnapshotsClient()
    images = ImagesClient()

    with ExitStack() as exitStack:
        # Create a dummy compute instance to host the node image creation process.
        console.print(
            f'Creating instance [bold]{instanceName}[/bold]'
            f' from [bold]{stockProject}/{stockFamily}[/bold]',
        )
        poll(
            instances.insert(
                project=project,
                zone=instanceZone,
                instance_resource=Instance(
                    name=instanceName,
                    machine_type=f'zones/{instanceZone}/machineTypes/{instanceType}',
                    disks=[
                        AttachedDisk(
                            boot=True,
                            auto_delete=True,
                            initialize_params=AttachedDiskInitializeParams(
                                source_image=f'projects/{stockProject}/global/images/family/{stockFamily}',
                            ),
                        ),
                    ],
                    network_interfaces=[
                        NetworkInterface(
                            # Use the default access configuration
                            # to set up a public IP address for the instance.
                            access_configs=[AccessConfig()],
                        ),
                    ],
                    metadata=Metadata(
                        items=[
                            # Enable OS login so we can import RSA keys and SSH in.
                            Items(key='enable-oslogin', value='TRUE'),
                        ],
                    ),
                ),
            )
        )

        # From here on, always delete the dummy instance before exiting.
        @exitStack.callback
        def cleanupDummyInstance():
            console.print(f'Deleting instance [bold]{instanceName}[/bold]')
            poll(
                instances.delete(
                    project=project, zone=instanceZone, instance=instanceName
                )
            )

        # Get the public IP address of the newly-created instance.
        instance = instances.get(
            project=project, zone=instanceZone, instance=instanceName
        )
        instanceIp = instance.network_interfaces[0].access_configs[0].nat_ip

        # Create an RSA key pair and import it into the instance.
        rsaKey = RSAKey.generate(bits=2048)
        expiry = datetime.utcnow() + timedelta(seconds=60)

        response = osLogin.import_ssh_public_key(
            ssh_public_key=SshPublicKey(
                key=f'ssh-rsa {rsaKey.get_base64()} bootstrap',
                expiration_time_usec=int(expiry.timestamp() * 1_000_000),
            ),
        )
        username = response.login_profile.posix_accounts[0].username

        ssh = Connection(
            host=instanceIp,
            user=username,
            connect_kwargs={'pkey': rsaKey},
        )

        console.print(f'Uploading artifacts to [bold]{instanceName}[/bold]')
        ssh.put(VIMANAD_PATH, '/usr/bin/vimanad')
        ssh.put(VIMANAD_SERVICE_PATH, '/etc/systemd/system/vimanad.service')
        ssh.put(CONTAINERD_CONFIG_PATH, '/etc/containerd/config.toml')

        console.print(f'Configuring [bold]{instanceName}[/bold]')
        ssh.run(
            '\n'.join(
                [
                    'set -e',
                    'sudo apt-get update',
                    'sudo apt-get install -y cloud-init containerd',
                    'sudo systemctl enable vimanad',
                ],
            )
        )

        raise RuntimeError('here')

        # Wait for the instance to become SSH-available.
        timeout = 60
        console.print(
            f'Giving [bold]{instanceName}[/bold] up to {timeout} seconds to become SSH-available',
        )
        startTime = datetime.now()
        while True:
            result = run(
                [
                    'gcloud',
                    'compute',
                    'ssh',
                    instanceName,
                    f'--project={project}',
                    f'--zone={instanceZone}',
                    '--quiet',
                ],
                input='exit\n',
                stdout=DEVNULL,
                stderr=DEVNULL,
                text=True,
            )
            if result.returncode == 0:
                break

            sleep(1)
            if (datetime.now() - startTime).total_seconds() > timeout:
                raise RuntimeError(
                    'Timed out waiting for dummy instance to become available'
                )

        console.print(
            f'Stopping [bold]{instanceName}[/bold] to preserve disk integrity during snapshot',
        )
        poll(
            instances.stop(
                project=project,
                zone=instanceZone,
                instance=instanceName,
            )
        )

        console.print(f'Creating snapshot [bold]{snapshotName}[/bold]')
        poll(
            disks.create_snapshot(
                project=project,
                zone=instanceZone,
                disk=instanceName,
                snapshot_resource=Snapshot(
                    name=snapshotName,
                ),
            )
        )

        # From here on, always delete the snapshot before exiting.
        @exitStack.callback
        def cleanupSnapshot():
            console.print(f'Deleting snapshot [bold]{snapshotName}[/bold]')
            poll(
                snapshots.delete(
                    project=project,
                    snapshot=snapshotName,
                )
            )

        console.print(f'Creating image [bold]{name}[/bold] from the snapshot')
        poll(
            images.insert(
                project=project,
                image_resource=Image(
                    name=name,
                    family=family,
                    source_snapshot=f'projects/{project}/global/snapshots/{snapshotName}',
                ),
            )
        )

        console.print(
            f'Successfully created image [bold]{name}[/bold] under family [bold]{family}[/bold] 🙂',
        )


def poll(operation: ExtendedOperation) -> Any:
    """
    Wait for a long-running operation to complete, returning the result.

    On error, raise an exception.
    If there are warnings, print them to stderr in yellow.
    """
    # https://docs.cloud.google.com/compute/docs/samples/compute-operation-extended-wait
    result = operation.result(timeout=TIMEOUT)

    if operation.error_code:
        raise operation.exception() or RuntimeError(
            codedMessage(operation.error_code, operation.error_message)
        )

    if operation.warnings:
        for warning in operation.warnings:
            console.print(codedMessage(warning.code, warning.message), style='yellow')

    return result


def codedMessage(code, message: str) -> str:
    """Format a code-message pair in a standard way."""
    return f'[{code}] {message}'


if __name__ == '__main__':
    parser = ArgumentParser(description=__doc__)
    parser.add_argument(
        '--gcp-project', help='ID of the GCP project in which to create the node image'
    )
    args = parser.parse_args()

    main(args.gcp_project)
