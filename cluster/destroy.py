"""Shut down a Vimana cluster."""

from datetime import datetime

from python.runfiles import Runfiles
from rich.prompt import Confirm

from cluster.profile.loader import load as loadProfile
from cluster.profile.loader import name as profileName
from dev.lib.util import console, runLoggingStderr, step, truncateTimedelta

runfiles = Runfiles.Create()

# Path to the `kops` binary.
KOPS_PATH = runfiles.Rlocation('rules_k8s/kops.exe')


def main():
    profile = loadProfile()
    name = profileName()

    if not Confirm.ask(f'Destroy [bold]{name}[/bold]?'):
        exit(1)

    start = datetime.now()

    with step('Destroying cluster using [bold]kops[/bold]'):
        runLoggingStderr(
            KOPS_PATH,
            'delete',
            'cluster',
            name,
            f'--state={profile["state-store"]}',
            '--yes',
        )

    elapsed = truncateTimedelta(datetime.now() - start)
    console.print(
        f'[bold]{name}[/bold] successfully destroyed after [bold]{elapsed}[/bold] 💀',
    )


if __name__ == '__main__':
    main()
