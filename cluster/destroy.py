"""Shut down a Vimana cluster."""

from argparse import ArgumentParser
from datetime import datetime
from os import getenv
from os.path import join as joinPath
from subprocess import DEVNULL, PIPE, Popen

from rich.console import Console
from rich.prompt import Confirm

from cluster.profiles.load import load as loadProfile

# Path to the `kops` binary.
# `RUNFILES_DIR` is set when invoked via `bazel build`.
# `..` is the parent for external repo data dependencies when invoked via `bazel run`.
RUNFILES_DIR = getenv('RUNFILES_DIR', '..')
KOPS_PATH = joinPath(RUNFILES_DIR, 'rules_k8s+', 'kops.exe')

console = Console(stderr=True, highlight=False, soft_wrap=True)


def main(name: str):
    profile = loadProfile(name)
    if not Confirm.ask(f'Destroy [bold]{name}[/bold]?'):
        exit(1)
    start = datetime.now()

    console.print('Destroying cluster with [bold]kops[/bold]')
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
        raise RuntimeError(f"Failed to delete cluster '{name}'")

    elapsed = datetime.now() - start
    console.print(
        f'[bold]{name}[/bold] successfully destroyed after {elapsed.total_seconds()} seconds 💀',
    )


def _command(*args) -> int:
    process = Popen(args, stderr=PIPE, stdout=DEVNULL, text=True)
    for line in process.stderr:
        console.print(line.rstrip(), style='yellow')
    return process.wait()


if __name__ == '__main__':
    parser = ArgumentParser(description=__doc__)
    parser.add_argument(
        'profile',
        help="Name of the profile defined in 'profiles.yaml'",
    )
    args = parser.parse_args()

    main(args.profile)
