import typing
from datetime import datetime
from pathlib import Path

StreamHandle: typing.TypeAlias = int | None

class _MessageInfo:
    stream: StreamHandle
    text: str
    ephemeral: bool
    bar_progress: float | None
    bar_total: float | None
    use_timestamp: bool
    end_line: bool
    created_at: datetime
    terminal_prefix: str = ""

class Printer:
    stopped: bool
    prv_msg: _MessageInfo | None
    log: Path
    terminal_prefix: str = ""
    secrets: list[str]

    def __init__(self, log_filepath: Path) -> None: ...
    def set_terminal_prefix(self, prefix: str) -> None:
        """Set the prefix to be added to every line printed."""

    def show(
        self,
        stream: StreamHandle,
        text: str,
        *,
        ephemeral: bool = False,
        use_timestamp: bool = False,
        end_line: bool = False,
        avoid_logging: bool = False,
    ) -> None:
        """Show a text to a given stream if not stopped."""

    def stop(self) -> None:
        """Stop the printing infrastructure.

        In detail:
        - stop the spinner
        - show the cursor
        - add a new line to the screen (if needed)
        - close the log file
        """
