"""Just some tests."""

import time

# import timeit
from pathlib import Path

import craft_cli
from craft_cli import emit
from craft_cli.messages import Emitter


def example_01() -> None:
    """Show a simple message, the expected command result."""
    value = 42
    emit.message(f"The meaning of life is {value}.")


def example_02() -> None:
    """Show some progress, then the result."""
    emit.message("We need to know!")
    emit.progress("Building computer...")
    # time.sleep(1.5)
    emit.progress("Asking question...")
    # time.sleep(1.5)
    emit.message("The meaning of life is 42.")


def example_03() -> None:
    """Show some progress, with one long delay message, then the result."""
    emit.message("We need to know!")
    emit.progress("Building computer...")
    time.sleep(1.4)
    emit.progress("Asking question...")
    time.sleep(5)
    emit.message("The meaning of life is 42.")


# elapsed = timeit.timeit(example_02, number=10000)

for _ in range(10000):
    emit.init(craft_cli.EmitterMode.VERBOSE, "testing", "hello", Path("log.txt"))
    example_02()
    emit.ended_ok()
    emit = Emitter()

# emit.init(craft_cli.EmitterMode.VERBOSE, "testing", "hello", Path("log.txt"))
# example_02()
# emit.ended_ok()


# print(elapsed)
