"""
A library of useful, general values and functions for Python scripts.
"""

from contextlib import contextmanager
from datetime import datetime, timedelta
from shlex import quote
from subprocess import PIPE, CompletedProcess, Popen, run
from time import sleep
from typing import Callable, List, Optional, TextIO

from requests import Response, request
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
        f'Waiting up to [bold]{truncateTimedelta(timeout)}[/bold] for {description}'
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

    console.print(f'Done after [bold]{truncateTimedelta(now - start)}[/bold]')


def runLoggingStderr(*command) -> TextIO:
    """
    Run the command defined by the specified arguments.

    Any output written to stderr is immediately logged to the console in yellow.
    If the command exits with a non-zero status, an exception is raised.
    Otherwise, all output written to stdout is returned as a text stream.
    """
    process = Popen(command, stderr=PIPE, stdout=PIPE, text=True)

    for line in process.stderr:
        console.print(line.rstrip(), style='yellow')

    status = process.wait()
    if status != 0:
        stdout = process.stdout.read()
        raise RuntimeError(stdout if stdout else codeMessage(status, 'Command failed'))

    return process.stdout


def runOrDie(
    command: List[str], *args, stdout=PIPE, stderr=PIPE, text=True, **kwargs
) -> CompletedProcess:
    """
    Drop-in replacement for `subprocess.run`
    that raises with a helpful message if the status code is non-zero.

    Runs quietly by default.
    """
    kwargs['stdout'] = stdout
    kwargs['stderr'] = stderr
    kwargs['text'] = text
    result = run(command, *args, **kwargs)
    if result.returncode != 0:
        raise RuntimeError(
            f'Error running {" ".join(quote(token) for token in command)}:\n'
            + codeMessage(result.returncode, result.stderr)
        )
    return result


def requestOrDie(method: str, url: str, *args, **kwargs) -> Response:
    """
    Drop-in replacement for `requests.request`
    that raises with a helpful message if the response status is non-successful.
    """
    response = request(method, url, *args, **kwargs)
    if not response.ok:
        raise RuntimeError(
            f"Error requesting '{url}' with {method}:\n"
            + codeMessage(response.status_code, response.text)
        )
    return response


def codeMessage(code: int | str, message: str) -> str:
    """
    Format a code-message pair in a standard way.

    These kinds of pairs are common in error handling,
    like HTTP response codes / messages,
    command exit status / outputs,
    or many types of Python exceptions which include "status code" fields.
    """
    return f'[{code}] {message}'


def truncateTimedelta(span: timedelta) -> timedelta:
    """
    Return a new `timedelta` with the same number of total seconds as the input
    rounded down to the nearest integer.
    """
    return timedelta(seconds=int(span.total_seconds()))
