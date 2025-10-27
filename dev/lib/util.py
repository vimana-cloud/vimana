"""
A library of useful, general values and functions for Python scripts.
"""

from contextlib import contextmanager
from datetime import datetime, timedelta
from subprocess import DEVNULL, PIPE, Popen
from time import sleep
from typing import Callable, Optional

from rich.console import Console

console = Console(stderr=True, highlight=False, soft_wrap=True)


@contextmanager
def step(status: str):
    """
    Display a status message with an animated spinner for the life of this context.

    On success, keep the status message with a check mark.

    On failure, keep the status message with an X mark, and immediately print the exception text.
    The exception is also re-raised, but there may be cleanup actions that occur first.
    """
    try:
        with console.status(status):
            yield
    except Exception as e:
        console.print(f'[red]✘[/red] {status}')
        console.print(e, style='red')
        raise e
    console.print(f'[green]✔[/green] {status}')


def waitFor(
    condition: Callable[[], bool],
    description: str,
    timeout: timedelta,
    interval: timedelta,
    minimum: Optional[timedelta] = None,
):
    """
    Wait for some condition to become true.

    After waiting for a minimum wait time,
    test the condition predicate at regular intervals until it returns true,
    or a timeout expires.
    """
    with step(
        f'Waiting up to [bold]{int(timeout.total_seconds())}[/bold] seconds for {description}'
    ):
        if minimum is not None:
            sleep(min(minimum, timeout).total_seconds())
        start = datetime.now()
        end = start + timeout
        interval = interval.total_seconds()
        while (now := datetime.now()) < end:
            if condition():
                break
            else:
                sleep(interval)
        else:
            raise RuntimeError(f'Timed out waiting for {description}')

    console.print(f'Ready after {int((now - start).total_seconds())} seconds')


def runWithStderr(*args) -> int:
    """
    Run the command defined by the specified arguments.

    Any output written to stdout is discarded.
    Any output written to stderr is displayed in yellow.
    If the command exits with a non-zero status, an exception is raised.
    """
    process = Popen(args, stderr=PIPE, stdout=DEVNULL, text=True)

    for line in process.stderr:
        console.print(line.rstrip(), style='yellow')

    status = process.wait()
    if status != 0:
        raise RuntimeError(codeMessage(status, 'Command failed'))


def codeMessage(code: int | str, message: str) -> str:
    """
    Format a code-message pair in a standard way.

    These kinds of pairs are common in error handling,
    like HTTP response codes / messages,
    command exit status / outputs,
    or many types of Python exceptions which include "status code" fields.
    """
    return f'[{code}] {message}'
