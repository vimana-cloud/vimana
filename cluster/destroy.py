"""Shut down a Vimana cluster."""

from argparse import ArgumentParser
from datetime import datetime
from os import getenv
from os.path import join as joinPath

from rich.prompt import Confirm

from cluster.profiles.load import load as loadProfile
from dev.lib.util import console, runWithStderr, step

# Path to the `kops` binary.
# `RUNFILES_DIR` is set when invoked via `bazel build`.
# `..` is the parent for external repo data dependencies when invoked via `bazel run`.
RUNFILES_DIR = getenv('RUNFILES_DIR', '..')
KOPS_PATH = joinPath(RUNFILES_DIR, 'rules_k8s+', 'kops.exe')


def main(name: str):
    profile = loadProfile(name)

    if not Confirm.ask(f'Destroy [bold]{name}[/bold]?'):
        exit(1)

    start = datetime.now()

    with step('Destroying cluster using [bold]kops[/bold]'):
        runWithStderr(
            KOPS_PATH,
            'delete',
            'cluster',
            name,
            f'--state={profile["state-store"]}',
            '--yes',
        )

    elapsed = datetime.now() - start
    console.print(
        f'[bold]{name}[/bold] successfully destroyed after [bold]{elapsed}[/bold] 💀',
    )


if __name__ == '__main__':
    parser = ArgumentParser(description=__doc__)
    parser.add_argument(
        'profile',
        help="Name of the profile defined in 'profiles.yaml'",
    )
    args = parser.parse_args()

    main(args.profile)
