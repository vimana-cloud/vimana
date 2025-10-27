"""Create a node image for a Vimana cluster."""

from argparse import ArgumentParser
from contextlib import ExitStack
from datetime import datetime, timedelta
from os import getenv
from os.path import basename
from os.path import join as joinPath
from subprocess import DEVNULL, PIPE, run
from time import sleep
from typing import Any, Optional

from fabric import Connection
from google.api_core.extended_operation import ExtendedOperation
from google.auth import default
from google.auth.transport.requests import Request
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
from paramiko.ssh_exception import NoValidConnectionsError
from requests import get

from dev.lib.util import codedMessage, console, step

# Path to the `vimanad` binary.
VIMANAD_PATH = joinPath('cluster', 'node', 'vimanad-x86_64-linux')
# Path to SystemD service file for `vimanad`.
VIMANAD_SERVICE_PATH = joinPath('cluster', 'node', 'vimanad.prod.service')
# Path to containerd configuration file.
CONTAINERD_CONFIG_PATH = joinPath('cluster', 'node', 'containerd-config.toml')
# Timeout for any long-running operation, in seconds.
TIMEOUT = 300


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

    with ExitStack() as exitStack:
        # Create a dummy compute instance to host the node image creation process.
        with step(
            f'Creating instance [bold]{instanceName}[/bold]'
            f' from [bold]{stockProject}/{stockFamily}[/bold]'
        ):
            instances = InstancesClient()
            pollGcpOperation(
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
                ),
            )

        # From here on, always delete the dummy instance before exiting.
        @exitStack.callback
        def cleanupInstance():
            with step(f'Deleting instance [bold]{instanceName}[/bold]'):
                pollGcpOperation(
                    instances.delete(
                        project=project, zone=instanceZone, instance=instanceName
                    )
                )

        sshTimeout = 60
        sshStart = datetime.now()
        with step(
            f'Giving [bold]{instanceName}[/bold] up to [bold]{sshTimeout}[/bold] seconds'
            ' to become SSH-available'
        ):
            # Get the public IP address of the newly-created instance.
            instance = instances.get(
                project=project, zone=instanceZone, instance=instanceName
            )
            instanceIp = instance.network_interfaces[0].access_configs[0].nat_i_p

            # Use the email address from the application default credentials for OS login.
            email = gcpAdcEmail()

            rsaKey = RSAKey.generate(bits=2048)
            expiry = datetime.now() + timedelta(seconds=sshTimeout)

            osLogin = OsLoginServiceClient()
            response = osLogin.import_ssh_public_key(
                parent=f'users/{email}',
                ssh_public_key=SshPublicKey(
                    key=f'ssh-rsa {rsaKey.get_base64()} bootstrap',
                    expiration_time_usec=int(expiry.timestamp() * 1_000_000),
                ),
            )
            username = response.login_profile.posix_accounts[0].username

            ssh = Connection(
                host=instanceIp, user=username, connect_kwargs={'pkey': rsaKey}
            )

            # Wait for the instance to become SSH-available.
            while True:
                try:
                    ssh.open()
                    break
                except NoValidConnectionsError:
                    if (datetime.now() - sshStart).total_seconds() > sshTimeout:
                        raise RuntimeError(
                            'Timed out waiting for dummy instance to become available'
                        )
                    sleep(1)
                    continue

        with step(f'Uploading artifacts to [bold]{instanceName}[/bold]'):
            # These are all uploaded in the user's home directory by default.
            # We lack the root filesystem privileges necessary
            # to upload them directly to their proper destinations.
            # Follow up with `sudo mv` commands over SSH.
            ssh.put(VIMANAD_PATH)
            ssh.put(VIMANAD_SERVICE_PATH)
            ssh.put(CONTAINERD_CONFIG_PATH)

        with step(f'Configuring [bold]{instanceName}[/bold]'):
            ssh.run(
                '\n'.join(
                    [
                        'set -e',
                        'sudo apt-get update',
                        'sudo apt-get install -y cloud-init containerd',
                        f"sudo mv ~/'{basename(VIMANAD_PATH)}' /usr/bin/vimanad",
                        f"sudo mv ~/'{basename(VIMANAD_SERVICE_PATH)}' /etc/systemd/system/vimanad.service",
                        f"sudo mv ~/'{basename(CONTAINERD_CONFIG_PATH)}' /etc/containerd/config.toml",
                        'sudo systemctl enable vimanad',
                    ],
                ),
                # Only show stdout and stderr if there is a problem.
                hide=True,
            )

        with step(
            f'Stopping [bold]{instanceName}[/bold] to preserve disk integrity during snapshot'
        ):
            pollGcpOperation(
                instances.stop(
                    project=project,
                    zone=instanceZone,
                    instance=instanceName,
                ),
            )

        with step(f'Creating snapshot [bold]{snapshotName}[/bold]'):
            disks = DisksClient()
            pollGcpOperation(
                disks.create_snapshot(
                    project=project,
                    zone=instanceZone,
                    disk=instanceName,
                    snapshot_resource=Snapshot(
                        name=snapshotName,
                    ),
                ),
            )

        # From here on, always delete the snapshot before exiting.
        @exitStack.callback
        def cleanupSnapshot():
            with step(f'Deleting snapshot [bold]{snapshotName}[/bold]'):
                snapshots = SnapshotsClient()
                pollGcpOperation(
                    snapshots.delete(project=project, snapshot=snapshotName)
                )

        with step(f'Creating image [bold]{name}[/bold] from the snapshot'):
            images = ImagesClient()
            pollGcpOperation(
                images.insert(
                    project=project,
                    image_resource=Image(
                        name=name,
                        family=family,
                        source_snapshot=f'projects/{project}/global/snapshots/{snapshotName}',
                    ),
                ),
            )

        console.print(
            f'Successfully created image [bold]{name}[/bold] under family [bold]{family}[/bold] 🙂',
        )


def gcpAdcEmail():
    """
    Return the email address of the currently-signed-in account
    for GCP's application default credentials (ADC).
    """
    credentials, _project = default()
    if not credentials.valid:
        credentials.refresh(Request())

    response = get(
        'https://www.googleapis.com/oauth2/v1/userinfo',
        headers={'Authorization': f'Bearer {credentials.token}'},
    )

    return response.json()['email']


def pollGcpOperation(operation: ExtendedOperation) -> Any:
    """
    Wait for a long-running GCP operation to complete, returning the result.

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


if __name__ == '__main__':
    parser = ArgumentParser(description=__doc__)
    parser.add_argument(
        '--gcp-project', help='ID of the GCP project in which to create the node image'
    )
    args = parser.parse_args()

    main(args.gcp_project)
