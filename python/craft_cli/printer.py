"""The `Printer` module for handling messages to a terminal."""

from craft_cli._rs.printer import Message, MessageType, Mode
from craft_cli._rs.printer import Printer as RPrinter


class Printer:
    """Print status messages to a terminal with spinners for long operations."""

    def __init__(self, mode: Mode) -> None:
        self._rprinter = RPrinter()
        self._rprinter.start(mode)

    def info(self, msg: str) -> None:
        """Send an info-level message."""
        message = Message(msg, MessageType.Info)
        self._rprinter.send(message)

    def progress(self, msg: str, *, permanent: bool = False) -> None:
        """Send a progress message."""
        model = MessageType.ProgPersistent if permanent else MessageType.ProgEphemeral
        message = Message(msg, model)
        self._rprinter.send(message)
