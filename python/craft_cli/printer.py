from craft_cli._rs.printer import Message, Printer as RPrinter, MessageType, Mode

class Printer:
    def __init__(self, mode: Mode):
        self._rprinter = RPrinter()
        self._rprinter.start(mode)

    def info(self, msg: str):
        message = Message(msg, MessageType.INFO)
        self._rprinter.send(message)

    def progress(self, msg: str, *, permanent=False):
        model = MessageType.PROG_PERSISTENT if permanent else MessageType.PROG_EPHEMERAL
        message = Message(msg, model)
        self._rprinter.send(message)
