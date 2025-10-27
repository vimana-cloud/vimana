"""Create a new Vimana cluster."""

from argparse import ArgumentParser
from contextlib import ExitStack
from datetime import datetime, timedelta
from os import getenv
from os.path import join as joinPath
from typing import Dict

from google.cloud.compute_v1 import ImagesClient
from kubernetes.client import CoreV1Api
from kubernetes.config import load_kube_config
from rich.prompt import Confirm
from urllib3.exceptions import MaxRetryError

from cluster.profiles.load import load as loadProfile
from dev.lib.util import console, runWithStderr, step, waitFor

# Paths to tool binaries.
# `RUNFILES_DIR` is set when invoked via `bazel build`.
# `..` is the parent for external repo data dependencies when invoked via `bazel run`.
RUNFILES_DIR = getenv('RUNFILES_DIR', '..')
KOPS_PATH = joinPath(RUNFILES_DIR, 'rules_k8s+', 'kops.exe')
HELM_PATH = joinPath(RUNFILES_DIR, 'rules_k8s+', 'helm.exe')

# Path to the executable to deploy the Vimana operator in a cluster.
OPERATOR_DEPLOY_PATH = joinPath('operator', 'deploy')


def main(name: str):
    profile = loadProfile(name)

    # TODO: Also support other cloud platforms.
    if 'gcp' in profile:
        gcp(name, profile)


def gcp(name: str, profile: Dict[str, object]):
    profileGcp = profile['gcp']
    project = profileGcp['project']
    imageProject = profileGcp['image-project']
    imageFamily = profileGcp['image-family']

    # TODO: Flesh out the list of necessary APIs.
    # gcloud services enable --project="$gcp_project" \
    #  iam.googleapis.com \
    #  cloudresourcemanager.googleapis.com

    # Make sure `<project-number>@cloudservices.gserviceaccount.com` ($gcp_project)
    # has `roles/compute.imageUser` on `projects/vimana-node-images`.

    # Get the latest image from the family.
    with step(
        f'Looking up latest image from [bold]{imageProject}/{imageFamily}[/bold]'
    ):
        images = ImagesClient()
        image = images.get_from_family(project=imageProject, family=imageFamily)
        imageName = f'{imageProject}/{image.name}'
        imageCreationTime = datetime.fromisoformat(image.creation_timestamp)

    console.print(
        f'Using image [bold]{imageName}[/bold] created at [bold]{imageCreationTime}[/bold]',
    )

    create(name, profile, '--cloud=gce', f'--project={project}', f'--image={imageName}')


def create(name: str, profile: Dict[str, object], *args):
    start = datetime.now()

    with ExitStack() as exitStack:
        # Offer to clean up the cluster if anything fails
        # between starting to create it and successfully finishing.
        @exitStack.callback
        def cleanup():
            if Confirm.ask('Clean up partially-initialized cluster?'):
                with step(
                    f'Deleting cluster [bold]{name}[/bold] using [bold]kops[/bold]'
                ):
                    runWithStderr(
                        KOPS_PATH,
                        'delete',
                        'cluster',
                        name,
                        f'--state={profile["state-store"]}',
                        '--yes',
                    )

        with step(f'Provisioning cluster [bold]{name}[/bold] using [bold]kops[/bold]'):
            runWithStderr(
                KOPS_PATH,
                'create',
                'cluster',
                name,
                *args,
                f'--state={profile["state-store"]}',
                f'--zones={profile["zone"]}',
                '--control-plane-count=1',
                f'--control-plane-size={profile["machine-type"]}',
                '--node-count=1',
                f'--node-size={profile["machine-type"]}',
                '--networking=kube-router',
                '--kubernetes-feature-gates=+RuntimeClassInImageCriApi',
                '--set=spec.containerd.skipInstall=true',
                '--set=spec.containerd.address=/run/vimana/vimanad.sock',
                # '--topology=private',
                # '--bastion',
                '--yes',
            )

        load_kube_config()
        coreApi = CoreV1Api()

        def controlPlaneReady():
            try:
                coreApi.get_api_resources()
                return True
            except MaxRetryError:
                return False

        waitFor(
            controlPlaneReady,
            'the control plane to become ready',
            timeout=timedelta(seconds=300),
            interval=timedelta(seconds=10),
            minimum=timedelta(seconds=60),
        )

        with step('Installing Envoy Gateway using [bold]helm[/bold]'):
            runWithStderr(
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

        with step('Installing the Vimana operator'):
            runWithStderr(OPERATOR_DEPLOY_PATH)

        # Cluster creation succeeded. No cleanup necessary.
        exitStack.pop_all()

        elapsed = datetime.now() - start
        console.print(
            f'[bold]{name}[/bold] successfully created after [bold]{elapsed}[/bold] 🐣',
        )


if __name__ == '__main__':
    parser = ArgumentParser(description=__doc__)
    parser.add_argument(
        'profile',
        help="Name of the profile defined in 'profiles.yaml'",
    )
    args = parser.parse_args()

    main(args.profile)
