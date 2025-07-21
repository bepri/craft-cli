"""Just some tests."""

from time import sleep

from craft_cli.printer import Mode, Printer

printer = Printer(Mode.Verbose)

printer.progress("hey!", permanent=True)
printer.progress("starting", permanent=False)
sleep(5)
printer.progress("processing", permanent=False)
sleep(2)
printer.progress("end", permanent=True)
