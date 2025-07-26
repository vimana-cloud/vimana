"""Create a new Vimana cluster."""

from argparse import ArgumentParser
from datetime import datetime, timedelta
from os import getenv
from os.path import join as joinPath
from subprocess import DEVNULL, PIPE, Popen
from time import sleep
from typing import Dict

from google.cloud.compute_v1 import ImagesClient
from kubernetes.client import CoreV1Api
from kubernetes.config import load_kube_config
from rich.console import Console
from rich.prompt import Confirm
from urllib3.exceptions import MaxRetryError

from cluster.profiles.load import load as loadProfile

# Paths to tool binaries.
# `RUNFILES_DIR` is set when invoked via `bazel build`.
# `..` is the parent for external repo data dependencies when invoked via `bazel run`.
RUNFILES_DIR = getenv('RUNFILES_DIR', '..')
KOPS_PATH = joinPath(RUNFILES_DIR, 'rules_k8s+', 'kops.exe')
HELM_PATH = joinPath(RUNFILES_DIR, 'rules_k8s+', 'helm.exe')
KUBECTL_PATH = joinPath(RUNFILES_DIR, 'rules_k8s+', 'kubectl.exe')

# Path to the cluster-wide K8s resources to create during initialization.
CLUSTER_RESOURCES = joinPath('gateway', 'cluster.yaml')

console = Console(stderr=True, highlight=False, soft_wrap=True)


def main(name: str):
    profile = loadProfile(name)
    # TODO: Also support other cloud platforms.
    if 'gcp' in profile:
        _gcp(name, profile)


def _gcp(name: str, profile: Dict[str, object]):
    profileGcp = profile['gcp']

    # TODO: Flesh out the list of necessary APIs.
    # gcloud services enable --project="$gcp_project" \
    #  iam.googleapis.com \
    #  cloudresourcemanager.googleapis.com

    # Make sure `<project-number>@cloudservices.gserviceaccount.com` ($gcp_project)
    # has `roles/compute.imageUser` on `projects/vimana-node-images`.

    # Get the latest image from the family.
    imageProject = profileGcp['image-project']
    imageFamily = profileGcp['image-family']
    image = ImagesClient().get_from_family(project=imageProject, family=imageFamily)
    imageName = f'{imageProject}/{image.name}'
    imageCreationTime = datetime.fromisoformat(image.creation_timestamp)
    console.print(
        f'Using image [bold]{imageName}[/bold] created at [bold]{imageCreationTime}[/bold]',
    )

    _create(name, profile, f'--project={profileGcp["project"]}', f'--image={imageName}')


def _create(name: str, profile: Dict[str, object], *args):
    start = datetime.now()

    console.print('Provisioning cluster with [bold]kops[/bold]')
    if (
        _command(
            KOPS_PATH,
            'create',
            'cluster',
            name,
            '--cloud=gce',
            f'--state={profile["state-store"]}',
            f'--zones={profile["zone"]}',
            '--control-plane-count=1',
            f'--control-plane-size={profile["machine-type"]}',
            '--node-count=1',
            f'--node-size={profile["machine-type"]}',
            '--networking=kube-router',
            '--kubernetes-feature-gates=+RuntimeClassInImageCriApi',
            '--set=spec.containerd.skipInstall=true',
            '--set=spec.containerd.address=/run/vimana/workd.sock',
            # '--topology=private',
            # '--bastion',
            *args,
            '--yes',
        )
        != 0
    ):
        console.print(f'[red]Failed to provision [bold]{name}[/bold][/red]')
        _cleanup(name, profile)
        return

    load_kube_config()
    coreApi = CoreV1Api()
    if not _waitForControlPlane(coreApi):
        console.print(
            '[red]Timed out waiting for the control plane to become available[/red]',
        )
        _cleanup(name, profile)
        return

    console.print('Installing Envoy Gateway with [bold]helm[/bold]')
    if (
        _command(
            HELM_PATH,
            'install',
            'envoy-gateway',
            'oci://docker.io/envoyproxy/gateway-helm',
            '--version=v1.4.2',
            '--namespace=envoy-gateway-system',
            '--create-namespace',
            # Use gateway namespace mode
            # to create load balancer services in the same namespace as the Gateway resource.
            # https://gateway.envoyproxy.io/docs/tasks/operations/gateway-namespace-mode/
            '--set=config.envoyGateway.provider.kubernetes.deploy.type=GatewayNamespace',
        )
        != 0
    ):
        console.print('[red]Failed installing Envoy Gateway[/red]')
        _cleanup(name, profile)
        return

    # TODO:
    #   This should probably be done using the Python client, but ATTOW that's not as convenient.
    #   https://github.com/kubernetes-client/python/issues/740
    console.print('Creating cluster-wide resources with [bold]kubectl[/bold]')
    if (
        _command(
            KUBECTL_PATH,
            'apply',
            f'--filename={CLUSTER_RESOURCES}',
        )
        != 0
    ):
        console.print('[red]Failed creating Vimana cluster-wide resources[/red]')
        _cleanup(name, profile)
        return

    elapsed = datetime.now() - start
    console.print(
        f'[bold]{name}[/bold] successfully created after {elapsed} 🐣',
    )


def _command(*args) -> int:
    process = Popen(args, stderr=PIPE, stdout=DEVNULL, text=True)
    for line in process.stderr:
        console.print(line.rstrip(), style='yellow')
    return process.wait()


def _waitForControlPlane(
    coreApi: CoreV1Api,
    timeout=timedelta(seconds=300),
    interval=timedelta(seconds=10),
    minimum=timedelta(seconds=60),
):
    """
    Wait for the Kubernetes API to become available.

    After waiting for a minimum wait time,
    poll the API at regular intervals until a timeout expires.
    Return true iff the API is available.
    """
    console.print(
        f'Waiting up to {timeout.seconds} seconds for the control plane to become ready...',
    )
    start = datetime.now()
    sleep(min(minimum, timeout).seconds)
    while (now := datetime.now()) < start + timeout:
        try:
            coreApi.get_api_resources()
        except MaxRetryError:
            # This is expected if the control plane is not yet available.
            sleep(interval.seconds)
        else:
            console.print(f'Ready after {(now - start).seconds} seconds')
            return True
    return False


def _cleanup(name: str, profile: Dict[str, object]):
    if Confirm.ask('Clean up partially-initialized cluster?'):
        if (
            _command(
                KOPS_PATH,
                'delete',
                'cluster',
                name,
                f'--state={profile["state-store"]}',
                '--yes',
            )
            != 0
        ):
            console.print('[red]Failed to clean up the cluster![/red] 🧟')


if __name__ == '__main__':
    parser = ArgumentParser(description=__doc__)
    parser.add_argument(
        'profile',
        help="Name of the profile defined in 'profiles.yaml'",
    )
    args = parser.parse_args()

    main(args.profile)
