from contextlib import contextmanager

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


def codedMessage(code: int | str, message: str) -> str:
    """
    Format a code-message pair in a standard way.

    These kinds of pairs are common in error handling,
    like HTTP response codes / messages, or many types of Python exceptions.
    """
    return f'[{code}] {message}'
